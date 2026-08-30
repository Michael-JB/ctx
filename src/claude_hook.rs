//! Claude Code hook adapter: maps hook events onto the agent-status file.
//!
//! The `agent` status builtin reads a state word from `.git/agent-status`;
//! this module is the Claude Code side of that contract. Other agent systems
//! can feed the same file directly.

use std::path::Path;

use serde_json::Value;

// Tools that watch or sleep rather than act.
const MONITORING_TOOLS: &[&str] = &["Monitor", "ScheduleWakeup"];

fn state(payload: &serde_json::Map<String, Value>) -> Option<&'static str> {
    match payload.get("hook_event_name").and_then(Value::as_str) {
        Some("UserPromptSubmit") => Some("working"),
        Some("PreToolUse") => {
            let tool = payload.get("tool_name").and_then(Value::as_str);
            match tool {
                Some(tool) if MONITORING_TOOLS.contains(&tool) => Some("monitoring"),
                _ => Some("working"),
            }
        }
        Some("Notification") => Some("blocked"),
        Some("Stop") => Some("idle"),
        _ => None,
    }
}

/// Apply one hook event (the JSON Claude Code pipes to hooks) in `cwd`.
///
/// Anything unusable — no `.git` directory, malformed JSON, an unknown
/// event — is ignored: a status hook must never break the agent driving it.
pub fn handle(raw: &str, cwd: &Path) {
    let git_dir = cwd.join(".git");
    if !git_dir.is_dir() {
        return;
    }
    let Ok(Value::Object(payload)) = serde_json::from_str::<Value>(raw) else {
        return;
    };
    let path = git_dir.join("agent-status");
    if payload.get("hook_event_name").and_then(Value::as_str) == Some("SessionEnd") {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let Some(state) = state(&payload) else {
        return;
    };
    let current = std::fs::read_to_string(&path)
        .ok()
        .map(|text| text.trim().to_string());
    // Rewrite only on change: the file's mtime is the state's start.
    if current.as_deref() != Some(state) {
        let _ = std::fs::write(&path, format!("{state}\n"));
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn checkout() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    fn event(name: &str) -> String {
        format!("{{\"hook_event_name\": \"{name}\"}}")
    }

    fn tool_event(name: &str, tool: &str) -> String {
        format!("{{\"hook_event_name\": \"{name}\", \"tool_name\": \"{tool}\"}}")
    }

    fn status_of(checkout: &Path) -> String {
        std::fs::read_to_string(checkout.join(".git").join("agent-status")).unwrap()
    }

    #[test]
    fn events_map_to_states() {
        for (payload, state) in [
            (event("UserPromptSubmit"), "working"),
            (tool_event("PreToolUse", "Bash"), "working"),
            (tool_event("PreToolUse", "Monitor"), "monitoring"),
            (tool_event("PreToolUse", "ScheduleWakeup"), "monitoring"),
            (event("Notification"), "blocked"),
            (event("Stop"), "idle"),
        ] {
            let (_dir, checkout) = checkout();

            handle(&payload, &checkout);

            assert_eq!(status_of(&checkout), format!("{state}\n"));
        }
    }

    #[test]
    fn session_end_removes_the_file() {
        let (_dir, checkout) = checkout();
        handle(&event("Stop"), &checkout);

        handle(&event("SessionEnd"), &checkout);

        assert!(!checkout.join(".git").join("agent-status").exists());
        handle(&event("SessionEnd"), &checkout); // idempotent
    }

    #[test]
    fn unchanged_state_keeps_the_mtime() {
        // The file's mtime is the state's start; rewrites would reset the clock.
        let (_dir, checkout) = checkout();
        handle(&event("UserPromptSubmit"), &checkout);
        let path = checkout.join(".git").join("agent-status");
        let started = std::fs::metadata(&path).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        handle(&tool_event("PreToolUse", "Bash"), &checkout);

        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            started
        );
        handle(&event("Stop"), &checkout);
        assert_ne!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            started
        );
    }

    #[test]
    fn outside_a_checkout_does_nothing() {
        let dir = tempfile::tempdir().unwrap();

        handle(&event("Stop"), dir.path());

        assert!(!dir.path().join(".git").exists());
    }

    #[test]
    fn unusable_input_is_ignored() {
        let (_dir, checkout) = checkout();

        handle("not json", &checkout);
        handle("[1, 2]", &checkout);
        handle(&event("SomethingNew"), &checkout);

        assert!(!checkout.join(".git").join("agent-status").exists());
    }
}
