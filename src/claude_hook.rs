//! Claude Code hook adapter: maps hook events onto the agent-status file.
//!
//! The `agent` status builtin reads a state word from `.git/agent-status`;
//! this module is the Claude Code side of that contract. Other agent systems
//! can feed the same file directly.

use std::path::Path;

use serde_json::Value;

fn state(payload: &serde_json::Map<String, Value>) -> Option<&'static str> {
    match payload.get("hook_event_name").and_then(Value::as_str) {
        Some("UserPromptSubmit" | "PreToolUse") => Some("working"),
        Some("Notification") => Some("blocked"),
        Some("Stop") if awaits_background_work(payload) => Some("monitoring"),
        Some("Stop") => Some("idle"),
        _ => None,
    }
}

/// Whether the turn ended with work Claude resumes on its own: a background
/// task still in flight (a shell command, a monitor, a subagent) or a
/// scheduled wakeup. Claude Code lists only running and pending tasks, so any
/// entry means the agent is waiting on them, not on the user.
fn awaits_background_work(payload: &serde_json::Map<String, Value>) -> bool {
    let pending = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|entries| !entries.is_empty())
    };
    pending("background_tasks") || pending("session_crons")
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

    fn stop_event(background_tasks: &str, session_crons: &str) -> String {
        format!(
            "{{\"hook_event_name\": \"Stop\", \"background_tasks\": {background_tasks}, \
             \"session_crons\": {session_crons}}}"
        )
    }

    #[test]
    fn events_map_to_states() {
        for (payload, state) in [
            (event("UserPromptSubmit"), "working"),
            (tool_event("PreToolUse", "Bash"), "working"),
            (tool_event("PreToolUse", "Monitor"), "working"),
            (event("Notification"), "blocked"),
            (event("Stop"), "idle"),
            (stop_event("[]", "[]"), "idle"),
        ] {
            let (_dir, checkout) = checkout();

            handle(&payload, &checkout);

            assert_eq!(status_of(&checkout), format!("{state}\n"));
        }
    }

    #[test]
    fn stopping_with_background_work_pending_is_monitoring() {
        for (payload, state) in [
            (
                stop_event(
                    r#"[{"id": "b1", "type": "shell", "status": "running"}]"#,
                    "[]",
                ),
                "monitoring",
            ),
            (
                stop_event(
                    r#"[{"id": "a1", "type": "subagent", "status": "running"}]"#,
                    "[]",
                ),
                "monitoring",
            ),
            (
                stop_event(
                    r#"[{"id": "a2", "type": "subagent", "status": "pending"}]"#,
                    "[]",
                ),
                "monitoring",
            ),
            (
                stop_event(
                    "[]",
                    r#"[{"id": "c1", "schedule": "57 18 * * *", "recurring": false}]"#,
                ),
                "monitoring",
            ),
        ] {
            let (_dir, checkout) = checkout();

            handle(&payload, &checkout);

            assert_eq!(
                status_of(&checkout),
                format!("{state}\n"),
                "payload: {payload}"
            );
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
