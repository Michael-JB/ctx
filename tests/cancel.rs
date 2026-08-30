//! Cancellation kills git's whole process group (its own test binary: the
//! in-flight registry is process-global, and the lib tests' parallel git
//! calls must not be caught in the kill).

use std::time::{Duration, Instant};

#[test]
fn kill_inflight_ends_the_transport() {
    // A transport that hangs forever; the trailing # swallows git's arguments.
    unsafe {
        std::env::set_var("GIT_SSH_COMMAND", "sleep 599 #");
    }
    let fetch = std::thread::spawn(|| {
        ctx_tui::git::git_quiet(
            &["fetch", "ssh://host.invalid/x"],
            Some(std::path::Path::new("/")),
        )
    });
    std::thread::sleep(Duration::from_millis(500));

    let start = Instant::now();
    ctx_tui::git::kill_inflight();
    let result = fetch.join().expect("fetch thread joins");

    assert!(result.is_err(), "the killed fetch must report failure");
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "cancellation waited for the transfer"
    );
    let lingering = std::process::Command::new("pgrep")
        .args(["-f", "sleep 599"])
        .output()
        .expect("pgrep runs");
    assert_eq!(
        String::from_utf8_lossy(&lingering.stdout).trim(),
        "",
        "the transport outlived the cancelled fetch"
    );
}
