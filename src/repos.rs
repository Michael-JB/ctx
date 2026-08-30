use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::errors::{Result, msg};
use crate::git::{git, git_quiet};

pub fn repo_path(cfg: &Config, name: &str) -> PathBuf {
    cfg.repos_dir.join(format!("{name}.git"))
}

pub fn repo_names(cfg: &Config) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(&cfg.repos_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            name.strip_suffix(".git").map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

pub fn name_from_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

/// Populate a mirror's LFS store, which bare fetches leave empty.
///
/// Clones smudge against the mirror, so a missing object there fails every
/// checkout. The .gitattributes probe keeps repos without LFS away from the
/// lfs command, which may not even be installed.
fn fetch_lfs(path: &Path, branch: &str) -> Result<()> {
    let probe = git_quiet(
        &[
            "grep",
            "--quiet",
            "filter=lfs",
            branch,
            "--",
            ".gitattributes",
            "*/.gitattributes",
        ],
        Some(path),
    );
    if probe.is_err() {
        return Ok(());
    }
    git_quiet(&["lfs", "fetch", "origin", branch], Some(path))?;
    Ok(())
}

pub fn add_repo(cfg: &Config, url: &str, name: Option<&str>) -> Result<String> {
    // An empty name falls back like Python's `name or name_from_url(url)`
    // (a '' name would clone into a hidden `.git` entry).
    let name = match name {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => name_from_url(url),
    };
    let path = repo_path(cfg, &name);
    if path.exists() {
        return msg(format!(
            "repo '{name}' already registered at {}",
            path.display()
        ));
    }
    std::fs::create_dir_all(&cfg.repos_dir)?;
    let mirror = || -> Result<()> {
        git_quiet(
            &[
                "clone",
                "--bare",
                "--single-branch",
                url,
                &path.to_string_lossy(),
            ],
            None,
        )?;
        // Bare clones get no fetch refspec; mirror only the default branch.
        let branch = git_quiet(&["symbolic-ref", "--short", "HEAD"], Some(&path))?;
        git_quiet(
            &[
                "config",
                "remote.origin.fetch",
                &format!("+refs/heads/{branch}:refs/heads/{branch}"),
            ],
            Some(&path),
        )?;
        fetch_lfs(&path, &branch)
    };
    if let Err(err) = mirror() {
        // A half-made mirror would squat on the name; leave it unregistered.
        let _ = std::fs::remove_dir_all(&path);
        return Err(err);
    }
    // The repo's contexts directory is part of its registration: users may
    // place files there (e.g. an .envrc) before any context exists.
    std::fs::create_dir_all(cfg.contexts_dir.join(&name))?;
    Ok(name)
}

pub fn remove_repo(cfg: &Config, name: &str) -> Result<()> {
    let path = repo_path(cfg, name);
    if !path.exists() {
        return msg(format!("repo '{name}' is not registered"));
    }
    if default_repo(cfg).as_deref() == Some(name) {
        set_default_repo(cfg, None)?;
    }
    std::fs::remove_dir_all(&path)?;
    Ok(())
}

fn default_repo_file(cfg: &Config) -> PathBuf {
    cfg.repos_dir.join("default-repo")
}

/// The repo new contexts are created in by default, if set and still registered.
pub fn default_repo(cfg: &Config) -> Option<String> {
    let name = std::fs::read_to_string(default_repo_file(cfg))
        .ok()?
        .trim()
        .to_string();
    repo_path(cfg, &name).exists().then_some(name)
}

/// Set the default repo, or clear it with None.
pub fn set_default_repo(cfg: &Config, name: Option<&str>) -> Result<()> {
    let file = default_repo_file(cfg);
    let Some(name) = name else {
        match std::fs::remove_file(&file) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
            _ => return Ok(()),
        }
    };
    if !repo_path(cfg, name).exists() {
        return msg(format!("repo '{name}' is not registered"));
    }
    std::fs::write(&file, format!("{name}\n"))?;
    Ok(())
}

/// Refresh only the default branch; contexts fetch other branches from origin on demand.
pub fn update_repo(cfg: &Config, name: &str) -> Result<()> {
    let path = repo_path(cfg, name);
    let branch = default_branch(cfg, name)?;
    // A branch unborn on both ends (empty repo) has nothing to fetch, and
    // fetching it would fail; the local check keeps the common case one roundtrip.
    if git_quiet(
        &["for-each-ref", &format!("refs/heads/{branch}")],
        Some(&path),
    )?
    .is_empty()
        && git_quiet(
            &[
                "ls-remote",
                "--heads",
                "origin",
                &format!("refs/heads/{branch}"),
            ],
            Some(&path),
        )?
        .is_empty()
    {
        return Ok(());
    }
    git_quiet(
        &[
            "fetch",
            "origin",
            &format!("+refs/heads/{branch}:refs/heads/{branch}"),
        ],
        Some(&path),
    )?;
    fetch_lfs(&path, &branch)
}

pub fn repo_url(cfg: &Config, name: &str) -> Result<String> {
    Ok(git(
        &["remote", "get-url", "origin"],
        Some(&repo_path(cfg, name)),
    )?)
}

pub fn default_branch(cfg: &Config, name: &str) -> Result<String> {
    Ok(git_quiet(
        &["symbolic-ref", "--short", "HEAD"],
        Some(&repo_path(cfg, name)),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{commit_file, commit_lfs_file, git, lfs_available, test_env};

    #[test]
    fn name_from_url_derives_the_repo_name() {
        for (url, name) in [
            ("https://github.com/foo/bar.git", "bar"),
            ("https://github.com/foo/bar/", "bar"),
            ("git@github.com:foo/bar.git", "bar"),
            ("/local/path/bar", "bar"),
        ] {
            assert_eq!(name_from_url(url), name);
        }
    }

    #[test]
    fn add_repo_registers_under_derived_name() {
        let env = test_env();
        let origin = env.origin();

        let name = add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();

        assert_eq!(name, "origin");
        assert_eq!(repo_names(&env.cfg), ["origin"]);
    }

    #[test]
    fn add_repo_registers_under_given_name() {
        let env = test_env();
        let origin = env.origin();

        let name = add_repo(&env.cfg, &origin.to_string_lossy(), Some("custom")).unwrap();

        assert_eq!(name, "custom");
        assert_eq!(repo_names(&env.cfg), ["custom"]);
    }

    #[test]
    fn add_repo_treats_an_empty_name_as_unset() {
        let env = test_env();
        let origin = env.origin();

        let name = add_repo(&env.cfg, &origin.to_string_lossy(), Some("")).unwrap();

        assert_eq!(name, "origin");
        assert_eq!(repo_names(&env.cfg), ["origin"]);
    }

    #[test]
    fn add_repo_creates_the_contexts_dir() {
        let env = test_env();
        let origin = env.origin();

        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();

        assert!(env.cfg.contexts_dir.join("origin").is_dir());
    }

    #[test]
    fn add_repo_mirrors_only_the_default_branch() {
        let env = test_env();
        let origin = env.origin();
        git(&["branch", "other"], &origin);

        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();

        let mirror = repo_path(&env.cfg, "origin");
        let branches = git(
            &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
            &mirror,
        );
        assert_eq!(branches, "main");
    }

    #[test]
    fn add_repo_rejects_duplicates() {
        let env = test_env();
        let origin = env.origin();
        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();

        let err = add_repo(&env.cfg, &origin.to_string_lossy(), None)
            .expect_err("duplicate must be rejected");

        assert!(err.to_string().contains("already registered"));
    }

    #[test]
    fn repo_names_empty_without_repos() {
        let env = test_env();

        assert_eq!(repo_names(&env.cfg), Vec::<String>::new());
    }

    #[test]
    fn repo_names_sorted() {
        let env = test_env();
        let origin = env.origin();
        add_repo(&env.cfg, &origin.to_string_lossy(), Some("beta")).unwrap();
        add_repo(&env.cfg, &origin.to_string_lossy(), Some("alpha")).unwrap();

        assert_eq!(repo_names(&env.cfg), ["alpha", "beta"]);
    }

    #[test]
    fn repo_url_is_the_registered_url() {
        let env = test_env();
        let origin = env.origin();
        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();

        assert_eq!(
            repo_url(&env.cfg, "origin").unwrap(),
            origin.to_string_lossy()
        );
    }

    #[test]
    fn update_repo_picks_up_new_origin_commits() {
        let env = test_env();
        let origin = env.origin();
        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();
        commit_file(&origin, "new.txt", "x\n");

        update_repo(&env.cfg, "origin").unwrap();

        let mirror = repo_path(&env.cfg, "origin");
        assert_eq!(
            git(&["rev-parse", "main"], &mirror),
            git(&["rev-parse", "main"], &origin)
        );
    }

    #[test]
    fn update_repo_tolerates_an_empty_repo() {
        let env = test_env();
        let origin = env.make_origin("empty", true);
        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();

        update_repo(&env.cfg, "empty").unwrap();
    }

    fn lfs_objects(cfg: &Config, name: &str) -> Vec<PathBuf> {
        fn files(dir: &Path, into: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files(&path, into);
                } else {
                    into.push(path);
                }
            }
        }
        let mut found = Vec::new();
        files(
            &repo_path(cfg, name).join("lfs").join("objects"),
            &mut found,
        );
        found
    }

    #[test]
    fn add_repo_populates_the_lfs_store() {
        if !lfs_available() {
            return;
        }
        let env = test_env();
        let origin = env.origin();
        commit_lfs_file(&origin, "data.bin", "payload\n");

        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();

        assert!(!lfs_objects(&env.cfg, "origin").is_empty());
    }

    #[test]
    fn update_repo_fetches_new_lfs_objects() {
        if !lfs_available() {
            return;
        }
        let env = test_env();
        let origin = env.origin();
        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();
        commit_lfs_file(&origin, "data.bin", "payload\n");

        update_repo(&env.cfg, "origin").unwrap();

        assert!(!lfs_objects(&env.cfg, "origin").is_empty());
    }

    #[test]
    fn add_repo_skips_lfs_for_repos_without_it() {
        let env = test_env();
        let origin = env.origin();

        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();

        assert!(!repo_path(&env.cfg, "origin").join("lfs").exists());
    }

    #[test]
    fn remove_repo_unregisters() {
        let env = test_env();
        let origin = env.origin();
        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();

        remove_repo(&env.cfg, "origin").unwrap();

        assert_eq!(repo_names(&env.cfg), Vec::<String>::new());
    }

    #[test]
    fn remove_repo_rejects_unregistered() {
        let env = test_env();

        let err = remove_repo(&env.cfg, "nope").expect_err("unregistered must be rejected");

        assert!(err.to_string().contains("not registered"));
    }

    #[test]
    fn default_repo_is_unset_initially() {
        let env = test_env();

        assert_eq!(default_repo(&env.cfg), None);
    }

    #[test]
    fn default_repo_round_trips() {
        let env = test_env();
        let origin = env.origin();
        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();

        set_default_repo(&env.cfg, Some("origin")).unwrap();
        assert_eq!(default_repo(&env.cfg).as_deref(), Some("origin"));

        set_default_repo(&env.cfg, None).unwrap();
        assert_eq!(default_repo(&env.cfg), None);
    }

    #[test]
    fn set_default_repo_rejects_unregistered() {
        let env = test_env();

        let err =
            set_default_repo(&env.cfg, Some("nope")).expect_err("unregistered must be rejected");

        assert!(err.to_string().contains("not registered"));
    }

    #[test]
    fn remove_repo_clears_the_default() {
        let env = test_env();
        let origin = env.origin();
        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();
        set_default_repo(&env.cfg, Some("origin")).unwrap();

        remove_repo(&env.cfg, "origin").unwrap();

        assert_eq!(default_repo(&env.cfg), None);
        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();
        assert_eq!(
            default_repo(&env.cfg),
            None,
            "a re-added repo must not resurrect the default"
        );
    }
}
