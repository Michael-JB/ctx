//! Cancellation kills git's whole process group (its own test binary: the
//! in-flight registry is process-global, and the lib tests' parallel git
//! calls must not be caught in the kill).

use std::process::Command;
use std::time::{Duration, Instant};

/// Killing an in-flight fetch must also end its transport, promptly.
#[test]
fn kill_inflight_ends_the_transport() {
    // The fetch must run in a real repo, or git fails before the transport
    // ever spawns and the kill path silently goes unexercised.
    let dir = tempfile::tempdir().expect("temp repo dir");
    let repo = dir.path().to_path_buf();
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .expect("git init runs");
    assert!(init.success());
    // A transport that hangs forever; the trailing # swallows git's arguments.
    unsafe {
        std::env::set_var("GIT_SSH_COMMAND", "sleep 599 #");
    }
    let fetch = std::thread::spawn(move || {
        ctx_tui::git::git_quiet(&["fetch", "ssh://host.invalid/x"], Some(&repo))
    });
    // Wait for the transport to be up, then cancel.
    let hung = Instant::now();
    loop {
        let up = Command::new("pgrep")
            .args(["-f", "sleep 599"])
            .output()
            .expect("pgrep runs");
        if !up.stdout.is_empty() {
            break;
        }
        assert!(
            hung.elapsed() < Duration::from_secs(5),
            "the transport never spawned; the fetch failed too early"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let start = Instant::now();
    ctx_tui::git::kill_inflight();
    let result = fetch.join().expect("fetch thread joins");

    assert!(result.is_err(), "the killed fetch must report failure");
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "cancellation waited for the transfer"
    );
    let lingering = Command::new("pgrep")
        .args(["-f", "sleep 599"])
        .output()
        .expect("pgrep runs");
    assert_eq!(
        String::from_utf8_lossy(&lingering.stdout).trim(),
        "",
        "the transport outlived the cancelled fetch"
    );

    // The kill is scoped to what was in flight: a later git call still works.
    assert!(ctx_tui::git::git_quiet(&["--version"], None).is_ok());
}
