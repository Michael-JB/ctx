//! Claude Code trust seeder: pre-trusts a directory in Claude's user config.
//!
//! Claude Code gates each workspace behind a trust dialog and records the
//! answer per directory in its user config; recording the answer before launch
//! keeps a fresh checkout's session from stopping at the dialog. This module
//! is the Claude Code side of that contract.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

fn config_file_from(config_dir: Option<&str>, home: &Path) -> PathBuf {
    match config_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home.to_path_buf(),
    }
    .join(".claude.json")
}

fn config_file() -> PathBuf {
    let config_dir = std::env::var("CLAUDE_CONFIG_DIR").ok();
    config_file_from(config_dir.as_deref(), &crate::config::home())
}

/// Record `cwd` as trusted in Claude Code's user config.
///
/// Best effort: an unusable config is left alone, and an answer already
/// on record is respected, whichever way — worst case the trust dialog
/// shows, which must never break launching the agent.
pub fn trust(cwd: &Path) {
    trust_in(&config_file(), cwd);
}

fn trust_in(file: &Path, cwd: &Path) {
    let mut data = match std::fs::read_to_string(file) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(data)) => data,
            _ => return,
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(_) => return,
    };
    let Value::Object(projects) = data
        .entry("projects".to_string())
        .or_insert_with(|| json!({}))
    else {
        return;
    };
    let Value::Object(entry) = projects
        .entry(cwd.to_string_lossy().into_owned())
        .or_insert_with(|| json!({}))
    else {
        return;
    };
    if entry.contains_key("hasTrustDialogAccepted") {
        return;
    }
    entry.insert("hasTrustDialogAccepted".to_string(), Value::Bool(true));
    let Ok(text) = serde_json::to_string_pretty(&Value::Object(data)) else {
        return;
    };
    // Claude Code rewrites this file while running and keeps it private:
    // swap in a finished copy carrying the original's permissions.
    let tmp = file.with_file_name(match file.file_name() {
        Some(name) => format!("{}.ctx-tmp", name.to_string_lossy()),
        None => return,
    });
    let swap = || -> std::io::Result<()> {
        std::fs::write(&tmp, &text)?;
        if let Ok(meta) = std::fs::metadata(file) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, file)
    };
    if swap().is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".claude.json");
        (dir, file)
    }

    fn read(file: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
    }

    #[test]
    fn trust_adds_the_project_entry() {
        let (_dir, file) = config();
        std::fs::write(&file, r#"{"numStartups": 3, "projects": {}}"#).unwrap();

        trust_in(&file, Path::new("/w/repo"));

        let data = read(&file);
        assert_eq!(
            data["projects"]["/w/repo"],
            json!({"hasTrustDialogAccepted": true})
        );
        assert_eq!(data["numStartups"], 3);
    }

    #[test]
    fn trust_keeps_other_entry_fields() {
        let (_dir, file) = config();
        std::fs::write(
            &file,
            r#"{"projects": {"/w/repo": {"allowedTools": ["Bash"]}}}"#,
        )
        .unwrap();

        trust_in(&file, Path::new("/w/repo"));

        assert_eq!(
            read(&file)["projects"]["/w/repo"],
            json!({"allowedTools": ["Bash"], "hasTrustDialogAccepted": true})
        );
    }

    #[test]
    fn trust_creates_a_missing_config() {
        let (_dir, file) = config();

        trust_in(&file, Path::new("/w/repo"));

        assert_eq!(
            read(&file),
            json!({"projects": {"/w/repo": {"hasTrustDialogAccepted": true}}})
        );
    }

    #[test]
    fn config_file_defaults_to_the_home_config() {
        assert_eq!(
            config_file_from(None, Path::new("/home/me")),
            PathBuf::from("/home/me/.claude.json")
        );
        assert_eq!(
            config_file_from(Some(""), Path::new("/home/me")),
            PathBuf::from("/home/me/.claude.json")
        );
        assert_eq!(
            config_file_from(Some("/cfg"), Path::new("/home/me")),
            PathBuf::from("/cfg/.claude.json")
        );
    }

    #[test]
    fn trust_respects_a_recorded_answer() {
        let (_dir, file) = config();
        let original = r#"{"projects": {"/w/repo": {"hasTrustDialogAccepted": false}}}"#;
        std::fs::write(&file, original).unwrap();

        trust_in(&file, Path::new("/w/repo"));

        assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    }

    #[test]
    fn trust_keeps_the_config_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, file) = config();
        std::fs::write(&file, "{}").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();

        trust_in(&file, Path::new("/w/repo"));

        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn trust_leaves_an_unparseable_config_alone() {
        let (_dir, file) = config();
        std::fs::write(&file, "{not json").unwrap();

        trust_in(&file, Path::new("/w/repo"));

        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{not json");
    }
}
