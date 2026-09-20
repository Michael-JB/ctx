//! Claude Code transcripts adapter: keeps a checkout's conversations with it.
//!
//! Claude Code stores each directory's transcripts under a directory named
//! after the path, and `--continue` looks there for the conversation to
//! resume. When ctx moves a checkout, it moves the transcripts along, so the
//! next session picks up where the last one left off. This module is the
//! Claude Code side of that contract.

use std::path::{Path, PathBuf};

use crate::multiplexer::env_var;

/// Where the transcripts live: one directory per project key, under the
/// config dir (`~/.claude` unless CLAUDE_CONFIG_DIR says otherwise).
fn projects_dir_from(config_dir: Option<&str>, home: &Path) -> PathBuf {
    match config_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home.join(".claude"),
    }
    .join("projects")
}

fn projects_dir() -> PathBuf {
    let config_dir = env_var("CLAUDE_CONFIG_DIR");
    projects_dir_from(config_dir.as_deref(), &crate::config::home())
}

// XXX: Undocumented Claude Code internals, like the trust seeding; they may
// change with any release.
/// The transcripts directory's name for a path: every character that is not
/// an ASCII letter or digit becomes a dash.
pub fn project_key(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

/// The path as a process's cwd reports it: symlinks in the parent resolved.
/// The parent, which survives the rename, stands in for the path itself,
/// which may be gone already or not exist yet.
pub(crate) fn resolved(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

/// Move the transcripts of the checkout at `from` over to `to`.
///
/// Best effort: transcripts already at `to` are left alone, and nothing
/// here may fail the rename it follows.
pub fn relocate(from: &Path, to: &Path) {
    relocate_in(&projects_dir(), from, to);
}

fn relocate_in(projects: &Path, from: &Path, to: &Path) {
    let old = projects.join(project_key(&resolved(from)));
    let new = projects.join(project_key(&resolved(to)));
    if old.is_dir() && !new.exists() {
        let _ = std::fs::rename(&old, &new);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_dir_defaults_to_the_home_dot_claude() {
        assert_eq!(
            projects_dir_from(None, Path::new("/home/me")),
            PathBuf::from("/home/me/.claude/projects")
        );
        assert_eq!(
            projects_dir_from(Some(""), Path::new("/home/me")),
            PathBuf::from("/home/me/.claude/projects")
        );
        assert_eq!(
            projects_dir_from(Some("/cfg"), Path::new("/home/me")),
            PathBuf::from("/cfg/projects")
        );
    }

    #[test]
    fn project_key_dashes_everything_but_ascii_alphanumerics() {
        assert_eq!(
            project_key(Path::new("/Users/j.doe/dev/two words")),
            "-Users-j-doe-dev-two-words"
        );
    }

    #[test]
    fn relocate_moves_the_transcripts() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        let old = projects.join(project_key(Path::new("/w/old")));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("s.jsonl"), "{}\n").unwrap();

        relocate_in(&projects, Path::new("/w/old"), Path::new("/w/new"));

        let new = projects.join(project_key(Path::new("/w/new")));
        assert!(new.join("s.jsonl").exists());
        assert!(!old.exists());
    }

    #[test]
    fn relocate_leaves_transcripts_already_at_the_destination_alone() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        let old = projects.join(project_key(Path::new("/w/old")));
        let new = projects.join(project_key(Path::new("/w/new")));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("keep"), "").unwrap();

        relocate_in(&projects, Path::new("/w/old"), Path::new("/w/new"));

        assert!(old.exists());
        assert!(new.join("keep").exists());
    }

    #[test]
    fn relocate_keys_by_the_resolved_path() {
        // A cwd never carries symlinks, so neither do Claude's keys.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let projects = dir.path().join("projects");
        let old = projects.join(project_key(&real.join("old")));
        std::fs::create_dir_all(&old).unwrap();

        relocate_in(&projects, &link.join("old"), &link.join("new"));

        assert!(projects.join(project_key(&real.join("new"))).is_dir());
        assert!(!old.exists());
    }

    #[test]
    fn relocate_without_transcripts_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");

        relocate_in(&projects, Path::new("/w/old"), Path::new("/w/new"));

        assert!(!projects.exists());
    }
}
