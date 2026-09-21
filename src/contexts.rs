use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::config::Config;
use crate::errors::{CtxError, Result, msg};
use crate::git::{git, git_quiet, new_command};
use crate::repos;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub repo: String,
    pub name: String,
    pub path: PathBuf,
}

impl Context {
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.repo, self.name)
    }
}

// Checkouts are renamed to this suffix before removal, so an interrupted
// delete leaves a marked corpse rather than a live-looking, half-gutted
// context. `sweep_deleting` finishes such leftovers.
const DELETING_SUFFIX: &str = ".deleting";

pub fn context_path(cfg: &Config, repo: &str, name: &str) -> PathBuf {
    cfg.contexts_dir.join(repo).join(name)
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

fn touched_marker(ctx: &Context) -> PathBuf {
    ctx.path.join(".git").join("ctx-touched")
}

/// Record the context being brought into play, by creation or by opening;
/// the marker's mtime is the timestamp.
pub fn touch(ctx: &Context) {
    // Listing order is not worth failing the operation over.
    let _ = std::fs::File::create(touched_marker(ctx))
        .and_then(|file| file.set_modified(SystemTime::now()));
}

/// When the context was last created or opened, if known.
fn last_touched(ctx: &Context) -> Option<SystemTime> {
    mtime(&touched_marker(ctx))
}

/// Whether the context has gone a day or more untouched.
pub fn is_stale(ctx: &Context, now: SystemTime) -> bool {
    const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
    last_touched(ctx).is_none_or(|at| now.duration_since(at).unwrap_or_default() >= STALE_AFTER)
}

/// Move the context's last touch `ago` into the past.
#[cfg(test)]
pub fn backdate_touch(ctx: &Context, ago: std::time::Duration) {
    let file = std::fs::File::open(touched_marker(ctx)).unwrap();
    file.set_modified(SystemTime::now() - ago).unwrap();
}

fn archived_marker(ctx: &Context) -> PathBuf {
    ctx.path.join(".git").join("ctx-archived")
}

/// Record the context being archived; the marker's mtime is the timestamp.
fn mark_archived(ctx: &Context) {
    // The date is informational; not worth failing the archive over.
    let _ = std::fs::File::create(archived_marker(ctx))
        .and_then(|file| file.set_modified(SystemTime::now()));
}

/// When the context was archived, if known.
pub fn archived_at(ctx: &Context) -> Option<SystemTime> {
    mtime(&archived_marker(ctx))
}

/// Whether a directory is a full clone, the only thing a context can be.
///
/// A `.git` directory means a full clone. A `.git` file marks a linked
/// worktree (e.g. one an agent created next to a context); those are not
/// contexts and nothing downstream can handle them.
fn is_clone(path: &Path) -> bool {
    path.join(".git").is_dir()
}

fn sorted_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    paths
}

/// Contexts under a <root>/<repo>/<name> tree, latest `recency` first;
/// ones without a recency follow by name.
fn scan(root: &Path, recency: fn(&Context) -> Option<SystemTime>) -> Vec<Context> {
    if !root.is_dir() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for repo_dir in sorted_dirs(root) {
        if !repo_dir.is_dir() {
            continue;
        }
        let repo = repo_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for ctx_dir in sorted_dirs(&repo_dir) {
            let name = ctx_dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if name.ends_with(DELETING_SUFFIX) {
                continue;
            }
            if is_clone(&ctx_dir) {
                found.push(Context {
                    repo: repo.clone(),
                    name,
                    path: ctx_dir,
                });
            }
        }
    }
    let mut keyed: Vec<(Option<SystemTime>, String, Context)> = found
        .into_iter()
        .map(|ctx| (recency(&ctx), ctx.qualified(), ctx))
        .collect();
    keyed.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    keyed.into_iter().map(|(_, _, ctx)| ctx).collect()
}

/// All contexts, most recently touched first.
pub fn list_contexts(cfg: &Config) -> Vec<Context> {
    scan(&cfg.contexts_dir, last_touched)
}

/// Resolve a context name; names are globally unique.
pub fn find_context(cfg: &Config, name: &str) -> Result<Context> {
    list_contexts(cfg)
        .into_iter()
        .find(|ctx| ctx.name == name)
        .ok_or_else(|| CtxError::Msg(format!("no context '{name}'")))
}

/// Names are unique across live and archived contexts alike.
///
/// Sharing a name with an archived context would leave that archive
/// unrestorable, so the two pools compete for the same names.
fn check_name_free(cfg: &Config, name: &str, exclude: Option<&Path>) -> Result<()> {
    for ctx in list_contexts(cfg) {
        if ctx.name == name && Some(ctx.path.as_path()) != exclude {
            return msg(format!(
                "context name '{name}' is already used by {}",
                ctx.qualified()
            ));
        }
    }
    for ctx in list_archived(cfg) {
        if ctx.name == name && Some(ctx.path.as_path()) != exclude {
            return msg(format!(
                "context name '{name}' is already used by archived {}",
                ctx.qualified()
            ));
        }
    }
    Ok(())
}

/// The branch a context of this name works on.
fn branch_for(cfg: &Config, name: &str) -> String {
    // Spaces are welcome in context names but not in branch names; dash them
    // out. Anything else unfit for a branch is rejected, not rewritten.
    format!("{}{}", cfg.branch_prefix, name.replace(' ', "-"))
}

/// Reject names that break the paths, branches, or commands they feed.
fn check_name(name: &str, branch: &str) -> Result<()> {
    if name.trim().is_empty() {
        return msg("context name must not be empty");
    }
    if name.contains('/') || name == "." || name == ".." {
        return msg(format!(
            "context name '{name}' must be a single path component"
        ));
    }
    if name.starts_with('-') {
        return msg(format!("context name '{name}' must not start with '-'"));
    }
    if name.ends_with(DELETING_SUFFIX) {
        return msg(format!(
            "context name '{name}' must not end with '{DELETING_SUFFIX}'"
        ));
    }
    // cwd="/": the process's own cwd may have been deleted under it.
    let check = new_command("git")
        .args(["check-ref-format", &format!("refs/heads/{branch}")])
        .current_dir("/")
        .output();
    if !check.map(|output| output.status.success()).unwrap_or(false) {
        return msg(format!(
            "context name '{name}' does not make a valid branch name ('{branch}')"
        ));
    }
    Ok(())
}

#[rustfmt::skip]
const ADJECTIVES: &[&str] = &[
    "amber", "bold", "brave", "breezy", "bright", "calm", "cheeky", "clever",
    "cosmic", "cozy", "curious", "daring", "dapper", "dusty", "eager", "fancy",
    "fluffy", "frosty", "fuzzy", "gentle", "golden", "happy", "hazy", "holy",
    "humble", "jolly", "keen", "lively", "lucky", "mellow", "merry", "mighty",
    "misty", "nimble", "perky", "plucky", "proud", "quiet", "rosy", "rusty",
    "shiny", "sleepy", "sly", "snappy", "snug", "stormy", "sunny", "swift",
    "vivid", "witty",
];

#[rustfmt::skip]
const ANIMALS: &[&str] = &[
    "badger", "bear", "beaver", "bison", "crane", "dingo", "dolphin", "eagle",
    "falcon", "ferret", "finch", "fox", "gecko", "goose", "hare", "hawk",
    "heron", "husky", "jaguar", "koala", "lemur", "llama", "lynx", "magpie",
    "marmot", "moose", "narwhal", "otter", "owl", "panda", "pelican", "penguin",
    "puffin", "quail", "raccoon", "raven", "robin", "seal", "sparrow", "stork",
    "swan", "tiger", "toucan", "trout", "turtle", "walrus", "weasel", "wombat",
    "wren", "yak",
];

/// A free adjective-animal name for when the user would rather not pick one.
pub fn random_name(cfg: &Config) -> Result<String> {
    random_name_from(cfg, ADJECTIVES, ANIMALS)
}

fn random_name_from(cfg: &Config, adjectives: &[&str], animals: &[&str]) -> Result<String> {
    use rand::RngExt;

    let taken: std::collections::HashSet<String> = list_contexts(cfg)
        .into_iter()
        .chain(list_archived(cfg))
        .map(|ctx| ctx.name)
        .collect();
    let free: Vec<String> = adjectives
        .iter()
        .flat_map(|adjective| {
            animals
                .iter()
                .map(move |animal| format!("{adjective}-{animal}"))
        })
        .filter(|name| !taken.contains(name))
        .collect();
    if free.is_empty() {
        return msg("all generated names are taken; pick one yourself");
    }
    let pick = rand::rng().random_range(0..free.len());
    Ok(free[pick].clone())
}

pub fn create_context(cfg: &Config, repo: &str, name: &str, base: Option<&str>) -> Result<Context> {
    create_context_with(cfg, repo, name, base, None)
}

/// Like `create_context`, but a mirror refresh already under way is awaited
/// in place of fetching here (it is only consulted when no base is given).
pub fn create_context_with(
    cfg: &Config,
    repo: &str,
    name: &str,
    base: Option<&str>,
    refresh: Option<&repos::Refresh>,
) -> Result<Context> {
    let branch = branch_for(cfg, name);
    check_name(name, &branch)?;
    let mirror = repos::repo_path(cfg, repo);
    if !mirror.exists() {
        return msg(format!(
            "repo '{repo}' is not registered (ctx repo add <url>)"
        ));
    }
    check_name_free(cfg, name, None)?;
    let path = context_path(cfg, repo, name);

    let (base, fetch_base) = match base {
        None => {
            match refresh {
                Some(refresh) => refresh.wait()?,
                None => repos::update_repo(cfg, repo)?,
            }
            (repos::default_branch(cfg, repo)?, false)
        }
        // The mirror only carries the default branch; fetch the base into the context.
        Some(base) => (base.to_string(), true),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A directory that predates the clone is not ours to delete on failure.
    let preexisting = path.exists();
    let ctx = Context {
        repo: repo.to_string(),
        name: name.to_string(),
        path: path.clone(),
    };
    let checkout = || -> Result<()> {
        git_quiet(
            &["clone", &mirror.to_string_lossy(), &path.to_string_lossy()],
            None,
        )?;
        let url = repos::repo_url(cfg, repo)?;
        git_quiet(&["remote", "set-url", "origin", &url], Some(&path))?;
        if fetch_base && git_quiet(&["fetch", "origin", &base], Some(&path)).is_err() {
            remove_context(&ctx)?;
            return msg(format!("branch '{base}' not found on origin of '{repo}'"));
        }
        if !fetch_base
            && !git_quiet(
                &["for-each-ref", &format!("refs/heads/{branch}")],
                Some(&path),
            )?
            .is_empty()
        {
            // The clone's default branch may already bear the requested name
            // (e.g. a context named after it); adopt it instead of forking it.
            git_quiet(&["checkout", &branch], Some(&path))?;
        } else if !git_quiet(
            &["for-each-ref", &format!("refs/remotes/origin/{base}")],
            Some(&path),
        )?
        .is_empty()
        {
            git_quiet(
                &[
                    "checkout",
                    "--no-track",
                    "-b",
                    &branch,
                    &format!("origin/{base}"),
                ],
                Some(&path),
            )?;
        } else {
            // An empty repo has no commit to branch from; start the work branch unborn.
            git_quiet(&["checkout", "--no-track", "-b", &branch], Some(&path))?;
        }
        Ok(())
    };
    if let Err(err) = checkout() {
        // A half-made checkout would squat on the name; leave nothing behind.
        if !preexisting {
            let _ = std::fs::remove_dir_all(&path);
        }
        return Err(err);
    }
    touch(&ctx);
    Ok(ctx)
}

pub fn archive_path(cfg: &Config, repo: &str, name: &str) -> PathBuf {
    cfg.archive_dir.join(repo).join(name)
}

/// All archived contexts, most recently archived first.
pub fn list_archived(cfg: &Config) -> Vec<Context> {
    scan(&cfg.archive_dir, archived_at)
}

/// Resolve an archived context by name.
pub fn find_archived(cfg: &Config, name: &str) -> Result<Context> {
    list_archived(cfg)
        .into_iter()
        .find(|ctx| ctx.name == name)
        .ok_or_else(|| CtxError::Msg(format!("no archived context '{name}'")))
}

/// Resolve a context name among live and archived contexts alike.
pub fn find_any(cfg: &Config, name: &str) -> Result<Context> {
    list_contexts(cfg)
        .into_iter()
        .chain(list_archived(cfg))
        .find(|ctx| ctx.name == name)
        .ok_or_else(|| CtxError::Msg(format!("no context '{name}'")))
}

pub fn is_archived(cfg: &Config, ctx: &Context) -> bool {
    ctx.path.starts_with(&cfg.archive_dir)
}

/// Move a directory, falling back to copy-and-delete across filesystems.
fn move_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    match std::fs::rename(src, dest) {
        Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_tree(src, dest)?;
            std::fs::remove_dir_all(src)
        }
        result => result,
    }
}

fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            // Recreate symlinks (checkouts carry them, and a dangling or
            // directory link would fail a dereferencing copy).
            std::os::unix::fs::symlink(std::fs::read_link(entry.path())?, &target)?;
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Move a context's checkout into the archive.
pub fn archive_context(cfg: &Config, ctx: &Context) -> Result<Context> {
    check_name_free(cfg, &ctx.name, Some(&ctx.path))?;
    let dest = archive_path(cfg, &ctx.repo, &ctx.name);
    if dest.exists() {
        return msg(format!("'{}' is already archived", ctx.qualified()));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    move_dir(&ctx.path, &dest)?;
    let archived = Context {
        repo: ctx.repo.clone(),
        name: ctx.name.clone(),
        path: dest,
    };
    mark_archived(&archived);
    Ok(archived)
}

/// Move an archived checkout back among the live contexts.
pub fn unarchive_context(cfg: &Config, ctx: &Context) -> Result<Context> {
    check_name_free(cfg, &ctx.name, Some(&ctx.path))?;
    let dest = context_path(cfg, &ctx.repo, &ctx.name);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    move_dir(&ctx.path, &dest)?;
    let restored = Context {
        repo: ctx.repo.clone(),
        name: ctx.name.clone(),
        path: dest,
    };
    // Only archived contexts carry a stamp.
    let _ = std::fs::remove_file(archived_marker(&restored));
    Ok(restored)
}

/// Give a context, live or archived, a new name.
///
/// The new name must pass the create rules, so a renamed context could
/// have been created as such.
pub fn rename_context(cfg: &Config, ctx: &Context, name: &str) -> Result<Context> {
    check_name(name, &branch_for(cfg, name))?;
    if name == ctx.name {
        return msg(format!("'{}' already has that name", ctx.qualified()));
    }
    check_name_free(cfg, name, None)?;
    // Same parent directory, so a plain rename; it must not replace a
    // (non-clone) directory already there, as renaming onto an empty one would.
    let dest = ctx.path.with_file_name(name);
    if dest.exists() {
        return msg(format!("'{}' already exists", dest.display()));
    }
    std::fs::rename(&ctx.path, &dest)?;
    // Agents key their per-checkout state by the live path, archived or not.
    crate::builtins::checkout_moved(
        &context_path(cfg, &ctx.repo, &ctx.name),
        &context_path(cfg, &ctx.repo, name),
    );
    Ok(Context {
        repo: ctx.repo.clone(),
        name: name.to_string(),
        path: dest,
    })
}

/// The checkout's branch, read from `.git/HEAD` to spare a subprocess.
///
/// Anything but a symbolic ref to a branch (e.g. a detached HEAD's raw
/// hash) reads as no branch, like `git branch --show-current`. So does an
/// unreadable HEAD: a checkout damaged on disk must not break listings.
pub fn current_branch(ctx: &Context) -> String {
    let Ok(head) = std::fs::read_to_string(ctx.path.join(".git").join("HEAD")) else {
        return String::new();
    };
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .unwrap_or("")
        .to_string()
}

pub fn is_dirty(ctx: &Context) -> Result<bool> {
    Ok(!git(&["status", "--porcelain"], Some(&ctx.path))?.is_empty())
}

pub fn unpushed_commits(ctx: &Context) -> Result<Vec<String>> {
    let out = git(
        &["log", "--branches", "--not", "--remotes", "--oneline"],
        Some(&ctx.path),
    )?;
    if out.is_empty() {
        return Ok(Vec::new());
    }
    Ok(out.lines().map(str::to_string).collect())
}

/// Take a context off its name with one rename; the returned corpse is
/// hidden from listings and costs `finish_removal` (or the startup sweep)
/// the actual delete.
///
/// A checkout can hold millions of files (virtualenvs, build output), so
/// the delete takes long where the rename is instant.
pub fn stage_removal(ctx: &Context) -> Result<PathBuf> {
    let name = ctx.path.file_name().unwrap_or_default().to_string_lossy();
    let doomed = ctx.path.with_file_name(format!("{name}{DELETING_SUFFIX}"));
    if doomed.exists() {
        let _ = std::fs::remove_dir_all(&doomed);
    }
    std::fs::rename(&ctx.path, &doomed)?;
    Ok(doomed)
}

pub fn finish_removal(doomed: &Path) -> Result<()> {
    std::fs::remove_dir_all(doomed)?;
    Ok(())
}

pub fn remove_context(ctx: &Context) -> Result<()> {
    finish_removal(&stage_removal(ctx)?)
}

/// Finish removals that a crash or kill interrupted mid-delete.
pub fn sweep_deleting(cfg: &Config) {
    for root in [&cfg.contexts_dir, &cfg.archive_dir] {
        if !root.is_dir() {
            continue;
        }
        for repo_dir in sorted_dirs(root) {
            if !repo_dir.is_dir() {
                continue;
            }
            for entry in sorted_dirs(&repo_dir) {
                if entry.to_string_lossy().ends_with(DELETING_SUFFIX) && entry.is_dir() {
                    let _ = std::fs::remove_dir_all(&entry);
                }
            }
        }
    }
}

/// Rename every archived context away, see `stage_removal`.
pub fn stage_empty_archive(cfg: &Config) -> Result<Vec<PathBuf>> {
    list_archived(cfg).iter().map(stage_removal).collect()
}

/// Permanently delete every archived context.
pub fn empty_archive(cfg: &Config) -> Result<()> {
    stage_empty_archive(cfg)?
        .iter()
        .try_for_each(|doomed| finish_removal(doomed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude_transcripts::{project_key, resolved};
    use crate::testutil::{TestEnv, commit_file, commit_lfs_file, git, lfs_available, test_env};

    fn registered() -> (TestEnv, PathBuf) {
        let env = test_env();
        let origin = env.origin();
        repos::add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();
        (env, origin)
    }

    fn create(env: &TestEnv, repo: &str, name: &str) -> Context {
        create_context(&env.cfg, repo, name, None).unwrap()
    }

    #[test]
    fn create_checks_out_the_repo() {
        let (env, _origin) = registered();

        let ctx = create(&env, "origin", "feat");

        assert_eq!(ctx.path, env.cfg.contexts_dir.join("origin").join("feat"));
        assert!(ctx.path.join("README.md").exists());
    }

    #[test]
    fn create_starts_a_branch_named_after_the_context() {
        let (env, _origin) = registered();

        let ctx = create(&env, "origin", "feat");

        assert_eq!(current_branch(&ctx), "feat");
    }

    #[test]
    fn create_adopts_an_existing_local_branch() {
        // "main" already exists in the fresh clone as its default branch. The
        // context checks it out instead of failing to create a branch of that name.
        let (env, _origin) = registered();

        let ctx = create(&env, "origin", "main");

        assert_eq!(current_branch(&ctx), "main");
        // No second branch was forked; the existing one is all there is.
        assert_eq!(
            git(&["branch", "--format=%(refname:short)"], &ctx.path),
            "main"
        );
    }

    #[test]
    fn create_applies_the_branch_prefix() {
        let (mut env, _origin) = registered();
        env.cfg.branch_prefix = "mb/".to_string();

        let ctx = create(&env, "origin", "feat");

        assert_eq!(current_branch(&ctx), "mb/feat");
    }

    #[test]
    fn create_maps_name_spaces_to_branch_dashes() {
        let (env, _origin) = registered();

        let ctx = create(&env, "origin", "two words");

        assert_eq!(current_branch(&ctx), "two-words");
        assert_eq!(find_context(&env.cfg, "two words").unwrap(), ctx);
    }

    #[test]
    fn create_smudges_lfs_files_from_the_mirror() {
        if !lfs_available() {
            return;
        }
        let env = test_env();
        let origin = env.origin();
        commit_lfs_file(&origin, "data.bin", "payload\n");
        repos::add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();
        // Only the mirror's LFS store may serve the checkout.
        std::fs::remove_dir_all(origin.join(".git").join("lfs")).unwrap();

        let ctx = create(&env, "origin", "feat");

        assert_eq!(
            std::fs::read_to_string(ctx.path.join("data.bin")).unwrap(),
            "payload\n"
        );
    }

    #[test]
    fn failed_clone_leaves_nothing_behind() {
        if !lfs_available() {
            return;
        }
        let env = test_env();
        let origin = env.origin();
        commit_lfs_file(&origin, "data.bin", "x\n");
        repos::add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();
        // A mirror missing LFS objects fails the clone's checkout; the explicit
        // base skips the mirror update that would repopulate the store.
        std::fs::remove_dir_all(repos::repo_path(&env.cfg, "origin").join("lfs")).unwrap();

        let result = create_context(&env.cfg, "origin", "feat", Some("main"));

        assert!(matches!(result, Err(CtxError::Git(_))));
        assert!(!env.cfg.contexts_dir.join("origin").join("feat").exists());
    }

    #[test]
    fn failed_clone_spares_a_preexisting_directory() {
        let (env, _origin) = registered();
        let path = env.cfg.contexts_dir.join("origin").join("feat");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("keep.txt"), "x\n").unwrap();

        let result = create_context(&env.cfg, "origin", "feat", None);

        assert!(matches!(result, Err(CtxError::Git(_))));
        assert!(path.join("keep.txt").exists());
    }

    #[test]
    fn create_rejects_an_empty_name() {
        let (env, _origin) = registered();

        for name in ["", "   "] {
            let err = create_context(&env.cfg, "origin", name, None).expect_err("must reject");
            assert!(err.to_string().contains("must not be empty"));
        }
    }

    #[test]
    fn create_rejects_path_like_names() {
        let (env, _origin) = registered();

        for name in ["a/b", ".", ".."] {
            let err = create_context(&env.cfg, "origin", name, None).expect_err("must reject");
            assert!(err.to_string().contains("single path component"));
        }
        assert_eq!(list_contexts(&env.cfg), vec![]);
    }

    #[test]
    fn create_rejects_an_option_like_name() {
        let (env, _origin) = registered();

        let err = create_context(&env.cfg, "origin", "-feat", None).expect_err("must reject");

        assert!(err.to_string().contains("must not start with '-'"));
    }

    #[test]
    fn create_rejects_names_unfit_for_branches() {
        let (env, _origin) = registered();

        for name in [
            "feat~1",
            "a..b",
            "what?",
            "tab\there",
            ".hidden",
            "feat.lock",
        ] {
            let err = create_context(&env.cfg, "origin", name, None).expect_err("must reject");
            assert!(err.to_string().contains("valid branch name"));
        }
        assert_eq!(list_contexts(&env.cfg), vec![]);
    }

    #[test]
    fn create_points_the_remote_at_the_registered_url() {
        let (env, origin) = registered();

        let ctx = create(&env, "origin", "feat");

        assert_eq!(
            git(&["remote", "get-url", "origin"], &ctx.path),
            origin.to_string_lossy()
        );
    }

    #[test]
    fn create_includes_the_latest_origin_commits() {
        let (env, origin) = registered();
        commit_file(&origin, "new.txt", "x\n");

        let ctx = create(&env, "origin", "feat");

        assert!(ctx.path.join("new.txt").exists());
    }

    #[test]
    fn create_with_a_finished_refresh_does_not_fetch() {
        let (env, origin) = registered();
        commit_file(&origin, "new.txt", "x\n");
        let refresh = repos::Refresh::default();
        refresh.finish(Ok(()));

        let ctx = create_context_with(&env.cfg, "origin", "feat", None, Some(&refresh)).unwrap();

        assert!(!ctx.path.join("new.txt").exists());
    }

    #[test]
    fn create_reports_a_failed_refresh() {
        let (env, _origin) = registered();
        let refresh = repos::Refresh::default();
        refresh.finish(msg("boom"));

        let err = create_context_with(&env.cfg, "origin", "feat", None, Some(&refresh))
            .expect_err("a failed refresh must fail the create");

        assert_eq!(err.to_string(), "boom");
        assert!(!context_path(&env.cfg, "origin", "feat").exists());
    }

    #[test]
    fn create_from_a_base_branch() {
        let (env, origin) = registered();
        git(&["branch", "other"], &origin);
        commit_file(&origin, "on-main.txt", "x\n");

        let ctx = create_context(&env.cfg, "origin", "feat", Some("other")).unwrap();

        assert_eq!(current_branch(&ctx), "feat");
        assert!(!ctx.path.join("on-main.txt").exists());
    }

    #[test]
    fn create_with_a_missing_base_fails_cleanly() {
        let (env, _origin) = registered();

        let err =
            create_context(&env.cfg, "origin", "feat", Some("nope")).expect_err("must reject");

        assert!(err.to_string().contains("branch 'nope' not found"));
        assert_eq!(list_contexts(&env.cfg), vec![]);
    }

    #[test]
    fn create_rejects_a_taken_name() {
        let (env, _origin) = registered();
        create(&env, "origin", "feat");

        let err = create_context(&env.cfg, "origin", "feat", None).expect_err("must reject");

        assert!(err.to_string().contains("already used by origin/feat"));
    }

    #[test]
    fn create_rejects_an_unregistered_repo() {
        let env = test_env();

        let err = create_context(&env.cfg, "nope", "feat", None).expect_err("must reject");

        assert!(err.to_string().contains("not registered"));
    }

    #[test]
    fn create_from_an_empty_repo() {
        let env = test_env();
        let origin = env.make_origin("empty", true);
        repos::add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();

        let ctx = create(&env, "empty", "feat");

        assert_eq!(current_branch(&ctx), "feat");
        // Listing must not choke on the missing reflog/index of an unborn branch.
        assert_eq!(list_contexts(&env.cfg), vec![ctx]);
    }

    #[test]
    fn random_name_pairs_an_adjective_with_an_animal() {
        let env = test_env();

        let name = random_name(&env.cfg).unwrap();
        let (adjective, animal) = name.split_once('-').unwrap();

        assert!(ADJECTIVES.contains(&adjective));
        assert!(ANIMALS.contains(&animal));
    }

    #[test]
    fn random_name_avoids_live_and_archived_names() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "holy-tiger");
        archive_context(&env.cfg, &ctx).unwrap();

        let name = random_name_from(&env.cfg, &["holy"], &["tiger", "otter"]).unwrap();

        assert_eq!(name, "holy-otter");
    }

    #[test]
    fn random_name_fails_when_all_names_are_taken() {
        let (env, _origin) = registered();
        create(&env, "origin", "holy-tiger");

        let err = random_name_from(&env.cfg, &["holy"], &["tiger"]).expect_err("must fail");

        assert!(err.to_string().contains("all generated names are taken"));
    }

    #[test]
    fn list_contexts_returns_created_contexts() {
        let (env, _origin) = registered();

        let created = create(&env, "origin", "feat");

        assert_eq!(list_contexts(&env.cfg), vec![created]);
    }

    #[test]
    fn list_contexts_ignores_agent_created_worktrees() {
        // A `git worktree add` from inside a checkout lands a sibling
        // directory whose `.git` is a file, not a clone; it is not a context.
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "task");
        let phantom = env.cfg.contexts_dir.join("origin").join("phantom");
        git(&["worktree", "add", &phantom.to_string_lossy()], &ctx.path);

        assert_eq!(list_contexts(&env.cfg), vec![ctx]);
    }

    fn set_mtime(path: &Path, when: u64) {
        let file = std::fs::File::open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(when))
            .unwrap();
    }

    fn set_touched(ctx: &Context, when: u64) {
        set_mtime(&touched_marker(ctx), when);
    }

    #[test]
    fn list_contexts_sorts_last_touched_first_then_by_name() {
        let (env, _origin) = registered();
        let older = create(&env, "origin", "older");
        let newer = create(&env, "origin", "newer");
        let b = create(&env, "origin", "b");
        let a = create(&env, "origin", "a");
        set_touched(&older, 1_000);
        set_touched(&newer, 2_000);
        set_touched(&b, 3_000);
        set_touched(&a, 3_000);

        assert_eq!(
            list_contexts(&env.cfg),
            vec![a.clone(), b.clone(), newer.clone(), older.clone()]
        );

        touch(&older);

        assert_eq!(list_contexts(&env.cfg), vec![older, a, b, newer]);
    }

    #[test]
    fn creating_a_context_counts_as_touching_it() {
        let (env, _origin) = registered();
        let old = create(&env, "origin", "old");
        set_touched(&old, 1_000);
        let fresh = create(&env, "origin", "fresh");

        assert_eq!(list_contexts(&env.cfg), vec![fresh, old]);
    }

    #[test]
    fn is_stale_after_a_day_untouched() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");
        let now = SystemTime::now();
        let day = std::time::Duration::from_secs(86_400);

        assert!(!is_stale(&ctx, now), "just created");
        backdate_touch(&ctx, day - std::time::Duration::from_secs(60));
        assert!(!is_stale(&ctx, now));
        backdate_touch(&ctx, day + std::time::Duration::from_secs(60));
        assert!(is_stale(&ctx, now));
    }

    #[test]
    fn find_context_resolves_by_name() {
        let (env, _origin) = registered();

        let created = create(&env, "origin", "feat");

        assert_eq!(find_context(&env.cfg, "feat").unwrap(), created);
    }

    #[test]
    fn find_context_rejects_unknown_names() {
        let env = test_env();

        let err = find_context(&env.cfg, "feat").expect_err("must reject");

        assert!(err.to_string().contains("no context 'feat'"));
    }

    #[test]
    fn fresh_context_is_clean() {
        let (env, _origin) = registered();

        let ctx = create(&env, "origin", "feat");

        assert!(!is_dirty(&ctx).unwrap());
        assert_eq!(unpushed_commits(&ctx).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn uncommitted_file_makes_context_dirty() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");

        std::fs::write(ctx.path.join("scratch.txt"), "x\n").unwrap();

        assert!(is_dirty(&ctx).unwrap());
    }

    #[test]
    fn local_commit_counts_as_unpushed() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");

        commit_file(&ctx.path, "work.txt", "x\n");

        assert_eq!(unpushed_commits(&ctx).unwrap().len(), 1);
    }

    #[test]
    fn archive_moves_the_checkout_out_of_the_contexts() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");

        let archived = archive_context(&env.cfg, &ctx).unwrap();

        assert_eq!(
            archived.path,
            env.cfg.archive_dir.join("origin").join("feat")
        );
        assert!(archived.path.join("README.md").exists());
        assert_eq!(list_contexts(&env.cfg), vec![]);
        assert_eq!(list_archived(&env.cfg), vec![archived]);
    }

    #[test]
    fn archive_stamps_when_it_happened() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");
        assert_eq!(archived_at(&ctx), None);

        let before = SystemTime::now();
        let archived = archive_context(&env.cfg, &ctx).unwrap();

        assert!(archived_at(&archived).is_some_and(|at| at >= before));
    }

    #[test]
    fn list_archived_sorts_most_recently_archived_first() {
        let (env, _origin) = registered();
        let older = archive_context(&env.cfg, &create(&env, "origin", "older")).unwrap();
        let newer = archive_context(&env.cfg, &create(&env, "origin", "newer")).unwrap();
        let unstamped = archive_context(&env.cfg, &create(&env, "origin", "a")).unwrap();
        set_mtime(&archived_marker(&older), 1_000);
        set_mtime(&archived_marker(&newer), 2_000);
        // Archives predating the stamp carry none and sort last, by name.
        std::fs::remove_file(archived_marker(&unstamped)).unwrap();

        assert_eq!(list_archived(&env.cfg), vec![newer, older, unstamped]);
    }

    #[test]
    fn archive_keeps_the_context_name_reserved() {
        let (env, _origin) = registered();
        let archived = archive_context(&env.cfg, &create(&env, "origin", "feat")).unwrap();

        let err = create_context(&env.cfg, "origin", "feat", None).expect_err("must reject");

        assert!(
            err.to_string()
                .contains("already used by archived origin/feat")
        );
        assert_eq!(list_archived(&env.cfg), vec![archived]);
    }

    #[test]
    fn archive_refuses_to_move_onto_an_occupied_path() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");
        std::fs::create_dir_all(env.cfg.archive_dir.join("origin").join("feat")).unwrap();

        let err = archive_context(&env.cfg, &ctx).expect_err("must reject");

        assert!(err.to_string().contains("already archived"));
        assert_eq!(list_contexts(&env.cfg), vec![ctx]);
    }

    #[test]
    fn find_archived_resolves_by_name() {
        let (env, _origin) = registered();
        let archived = archive_context(&env.cfg, &create(&env, "origin", "feat")).unwrap();

        assert_eq!(find_archived(&env.cfg, "feat").unwrap(), archived);
    }

    #[test]
    fn find_any_resolves_live_and_archived_contexts() {
        let (env, _origin) = registered();
        let archived = archive_context(&env.cfg, &create(&env, "origin", "cold")).unwrap();
        let live = create(&env, "origin", "hot");

        assert_eq!(find_any(&env.cfg, "cold").unwrap(), archived);
        assert_eq!(find_any(&env.cfg, "hot").unwrap(), live);
    }

    #[test]
    fn find_any_rejects_unknown_names() {
        let env = test_env();

        let err = find_any(&env.cfg, "feat").expect_err("must reject");

        assert!(err.to_string().contains("no context 'feat'"));
    }

    #[test]
    fn find_archived_rejects_unknown_names() {
        let env = test_env();

        let err = find_archived(&env.cfg, "feat").expect_err("must reject");

        assert!(err.to_string().contains("no archived context 'feat'"));
    }

    #[test]
    fn unarchive_restores_the_context() {
        let (env, _origin) = registered();
        let created = create(&env, "origin", "feat");
        let archived = archive_context(&env.cfg, &created).unwrap();

        let restored = unarchive_context(&env.cfg, &archived).unwrap();

        assert_eq!(archived_at(&restored), None);

        assert_eq!(restored, created);
        assert_eq!(list_contexts(&env.cfg), vec![created]);
        assert_eq!(list_archived(&env.cfg), vec![]);
    }

    #[test]
    fn unarchive_rejects_a_name_taken_by_a_live_context() {
        let (env, _origin) = registered();
        let stale = archive_context(&env.cfg, &create(&env, "origin", "old")).unwrap();
        let live = create(&env, "origin", "feat");
        // Archives predating global uniqueness can still clash with a live name.
        let clash = stale.path.with_file_name("feat");
        std::fs::rename(&stale.path, &clash).unwrap();

        let err = unarchive_context(
            &env.cfg,
            &Context {
                repo: "origin".to_string(),
                name: "feat".to_string(),
                path: clash.clone(),
            },
        )
        .expect_err("must reject");

        assert!(err.to_string().contains("already used by origin/feat"));
        assert!(live.path.exists());
        assert!(clash.exists());
    }

    #[test]
    fn rename_moves_the_checkout_and_keeps_the_branch() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");

        let renamed = rename_context(&env.cfg, &ctx, "better").unwrap();

        assert_eq!(
            renamed.path,
            env.cfg.contexts_dir.join("origin").join("better")
        );
        assert_eq!(current_branch(&renamed), "feat");
        assert!(!ctx.path.exists());
        assert_eq!(list_contexts(&env.cfg), vec![renamed]);
    }

    #[test]
    fn rename_keeps_an_archived_context_archived() {
        let (env, _origin) = registered();
        let archived = archive_context(&env.cfg, &create(&env, "origin", "feat")).unwrap();

        let renamed = rename_context(&env.cfg, &archived, "better").unwrap();

        assert_eq!(
            renamed.path,
            env.cfg.archive_dir.join("origin").join("better")
        );
        assert_eq!(list_archived(&env.cfg), vec![renamed]);
        assert_eq!(list_contexts(&env.cfg), vec![]);
    }

    #[test]
    fn rename_rejects_a_taken_name() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");
        create(&env, "origin", "live");
        archive_context(&env.cfg, &create(&env, "origin", "cold")).unwrap();

        let err = rename_context(&env.cfg, &ctx, "live").expect_err("must reject");
        assert!(err.to_string().contains("already used by origin/live"));
        let err = rename_context(&env.cfg, &ctx, "cold").expect_err("must reject");
        assert!(
            err.to_string()
                .contains("already used by archived origin/cold")
        );
        assert!(ctx.path.exists());
    }

    #[test]
    fn rename_rejects_the_current_name() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");

        let err = rename_context(&env.cfg, &ctx, "feat").expect_err("must reject");

        assert!(err.to_string().contains("already has that name"));
    }

    #[test]
    fn rename_rejects_names_unfit_for_contexts() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");

        for name in ["", "a/b", "-x", "feat~1", "x.deleting"] {
            rename_context(&env.cfg, &ctx, name).expect_err("must reject");
        }
        assert_eq!(list_contexts(&env.cfg), vec![ctx]);
    }

    #[test]
    fn rename_carries_the_agent_state_along() {
        // Claude Code keeps a checkout's transcripts under a directory named
        // after its path; a rename must move it or `--continue` finds nothing.
        let (env, _origin) = registered();
        let claude_dir = env.root().join("claude");
        let _config_dir =
            crate::testutil::push_env("CLAUDE_CONFIG_DIR", &claude_dir.to_string_lossy());
        let ctx = create(&env, "origin", "feat");
        let projects = claude_dir.join("projects");
        let old = projects.join(project_key(&resolved(&ctx.path)));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("s.jsonl"), "{}\n").unwrap();

        let renamed = rename_context(&env.cfg, &ctx, "better").unwrap();

        let new = projects.join(project_key(&resolved(&renamed.path)));
        assert!(new.join("s.jsonl").exists());
        assert!(!old.exists());
    }

    #[test]
    fn rename_of_an_archived_context_relocates_by_its_live_path() {
        let (env, _origin) = registered();
        let claude_dir = env.root().join("claude");
        let _config_dir =
            crate::testutil::push_env("CLAUDE_CONFIG_DIR", &claude_dir.to_string_lossy());
        let live = create(&env, "origin", "feat");
        let archived = archive_context(&env.cfg, &live).unwrap();
        let projects = claude_dir.join("projects");
        let old = projects.join(project_key(&resolved(&live.path)));
        std::fs::create_dir_all(&old).unwrap();

        rename_context(&env.cfg, &archived, "better").unwrap();

        let restored = context_path(&env.cfg, "origin", "better");
        assert!(projects.join(project_key(&resolved(&restored))).is_dir());
        assert!(!old.exists());
    }

    #[test]
    fn rename_refuses_to_replace_a_directory_in_the_way() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");
        let squatter = ctx.path.with_file_name("better");
        std::fs::create_dir(&squatter).unwrap();

        let err = rename_context(&env.cfg, &ctx, "better").expect_err("must reject");

        assert!(err.to_string().contains("already exists"));
        assert!(ctx.path.exists());
    }

    #[test]
    fn copy_tree_preserves_symlinks() {
        // The cross-filesystem fallback must move symlinks as symlinks:
        // dereferencing breaks relative links, and dangling ones error.
        let env = test_env();
        let src = env.root().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub").join("real.txt"), "x\n").unwrap();
        std::os::unix::fs::symlink("sub/real.txt", src.join("file-link")).unwrap();
        std::os::unix::fs::symlink("sub", src.join("dir-link")).unwrap();
        std::os::unix::fs::symlink("nowhere", src.join("dangling")).unwrap();

        let dest = env.root().join("dest");
        copy_tree(&src, &dest).unwrap();

        for (link, points_to) in [
            ("file-link", "sub/real.txt"),
            ("dir-link", "sub"),
            ("dangling", "nowhere"),
        ] {
            assert_eq!(
                std::fs::read_link(dest.join(link)).unwrap(),
                PathBuf::from(points_to)
            );
        }
    }

    #[test]
    fn current_branch_tolerates_a_damaged_checkout() {
        // A context whose removal was interrupted must not break listings.
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");
        std::fs::remove_file(ctx.path.join(".git").join("HEAD")).unwrap();

        assert_eq!(current_branch(&ctx), "");
    }

    #[test]
    fn remove_context_deletes_the_checkout() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");

        remove_context(&ctx).unwrap();

        assert_eq!(list_contexts(&env.cfg), vec![]);
        assert_eq!(
            sorted_dirs(&env.cfg.contexts_dir.join("origin")),
            vec![] as Vec<PathBuf>
        );
    }

    #[test]
    fn interrupted_removals_are_hidden_and_swept() {
        let (env, _origin) = registered();
        let kept = create(&env, "origin", "live");
        let doomed = create(&env, "origin", "feat");
        // An interrupted remove_context leaves the renamed checkout behind.
        let leftover = doomed.path.with_file_name("feat.deleting");
        std::fs::rename(&doomed.path, &leftover).unwrap();

        assert_eq!(list_contexts(&env.cfg), vec![kept.clone()]);

        sweep_deleting(&env.cfg);

        assert!(!leftover.exists());
        assert_eq!(list_contexts(&env.cfg), vec![kept]);
    }

    #[test]
    fn remove_context_replaces_a_stale_leftover() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "feat");
        let stale = ctx.path.with_file_name("feat.deleting");
        std::fs::create_dir(&stale).unwrap();
        std::fs::write(stale.join("junk"), "x\n").unwrap();

        remove_context(&ctx).unwrap();

        assert!(!ctx.path.exists());
        assert!(!stale.exists());
    }

    #[test]
    fn create_rejects_names_reserved_for_deletion() {
        let (env, _origin) = registered();

        let err =
            create_context(&env.cfg, "origin", "feat.deleting", None).expect_err("must reject");

        assert!(err.to_string().contains(".deleting"));
    }

    #[test]
    fn staged_removal_hides_the_context_until_finished() {
        let (env, _origin) = registered();
        let ctx = create(&env, "origin", "one");

        let doomed = stage_removal(&ctx).unwrap();

        assert!(doomed.is_dir());
        assert_eq!(list_contexts(&env.cfg), vec![]);
        finish_removal(&doomed).unwrap();
        assert!(!doomed.exists());
    }

    #[test]
    fn empty_archive_deletes_all_archived_contexts() {
        let (env, _origin) = registered();
        archive_context(&env.cfg, &create(&env, "origin", "one")).unwrap();
        archive_context(&env.cfg, &create(&env, "origin", "two")).unwrap();
        let kept = create(&env, "origin", "live");

        empty_archive(&env.cfg).unwrap();

        assert_eq!(list_archived(&env.cfg), vec![]);
        assert_eq!(list_contexts(&env.cfg), vec![kept]);
    }
}
