use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

// A stalled transfer otherwise hangs forever: ssh sends no keepalives by
// default, so a dead connection is never noticed, and git accepts an
// arbitrarily slow one. Both are capped to about a minute of silence.
const SSH_COMMAND: &str =
    "ssh -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3";
const STALL_CONFIG: &[&str] = &[
    "-c",
    "http.lowSpeedLimit=1000",
    "-c",
    "http.lowSpeedTime=60",
];

// Fallback for callers that pass no cwd: the process's own cwd may have been
// deleted under it (e.g. removing the context it sits in), which breaks git.
// All call sites use absolute paths, so any directory that always exists does.
const SAFE_CWD: &str = "/";

/// A failed git call whose message is git's own error when it was captured.
#[derive(Debug)]
pub struct GitError {
    pub argv: Vec<String>,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: Option<String>,
}

impl std::error::Error for GitError {}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = self.stderr.as_deref().unwrap_or("").trim();
        if detail.is_empty() {
            write!(f, "command failed ({})", self.argv.join(" "))
        } else {
            write!(f, "{detail}")
        }
    }
}

/// A Command for any program, with the test environment applied under tests.
pub(crate) fn new_command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(test)]
    crate::testutil::apply_test_env(&mut cmd);
    cmd
}

fn command_with(
    args: &[&str],
    cwd: Option<&Path>,
    ssh_configured: bool,
    prompt_configured: bool,
) -> Command {
    let mut cmd = new_command("git");
    cmd.args(STALL_CONFIG)
        .args(args)
        .current_dir(cwd.unwrap_or(Path::new(SAFE_CWD)));
    if !ssh_configured {
        cmd.env("GIT_SSH_COMMAND", SSH_COMMAND);
    }
    // A prompt would block on a terminal the caller may not be showing.
    if !prompt_configured {
        cmd.env("GIT_TERMINAL_PROMPT", "0");
    }
    cmd
}

fn command(args: &[&str], cwd: Option<&Path>) -> Command {
    command_with(
        args,
        cwd,
        std::env::var_os("GIT_SSH_COMMAND").is_some(),
        std::env::var_os("GIT_TERMINAL_PROMPT").is_some(),
    )
}

fn argv(args: &[&str]) -> Vec<String> {
    ["git"]
        .iter()
        .copied()
        .chain(STALL_CONFIG.iter().copied())
        .chain(args.iter().copied())
        .map(str::to_string)
        .collect()
}

fn spawn_error(args: &[&str], err: std::io::Error) -> GitError {
    GitError {
        argv: argv(args),
        code: None,
        stdout: String::new(),
        stderr: Some(err.to_string()),
    }
}

// In-flight git process groups, so cancellation can end a transfer and its
// ssh child instead of orphaning them (the asyncio version killed the group
// on task cancellation; here quit and SIGINT do the equivalent).
static INFLIGHT: Mutex<Vec<i32>> = Mutex::new(Vec::new());
static INTERRUPTED: LazyLock<Arc<AtomicBool>> = LazyLock::new(|| Arc::new(AtomicBool::new(false)));

/// Route SIGINT through the interrupted flag: in-flight git calls kill their
/// transfers and fail, so cleanup paths run instead of the process dying
/// mid-write with the transfer orphaned.
pub fn install_interrupt_handler() {
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, INTERRUPTED.clone());
}

/// Kill every in-flight git call's process group (quit with a transfer running).
///
/// Only the calls running right now are killed; the interrupt flag stays
/// untouched so the process can keep using git — the TUI kills a stray
/// transfer on exit and then creates the context the user asked for.
pub fn kill_inflight() {
    for pgid in INFLIGHT.lock().expect("registry lock").iter() {
        unsafe {
            libc::killpg(*pgid, libc::SIGKILL);
        }
    }
}

fn drain(stream: Option<impl std::io::Read + Send + 'static>) -> impl FnOnce() -> String {
    let handle = stream.map(|mut stream| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf);
            buf
        })
    });
    move || {
        handle
            .and_then(|handle| handle.join().ok())
            .map(|buf| String::from_utf8_lossy(&buf).into_owned())
            .unwrap_or_default()
    }
}

fn run_command(mut cmd: Command, args: &[&str], quiet: bool) -> Result<String, GitError> {
    use std::os::unix::process::CommandExt;
    use wait_timeout::ChildExt;

    cmd.stdout(Stdio::piped());
    if quiet {
        cmd.stderr(Stdio::piped());
    }
    // Own process group, so cancellation can kill git and its ssh child
    // together, and a terminal SIGINT reaches this process alone.
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|err| spawn_error(args, err))?;
    let pgid = child.id() as i32;
    INFLIGHT.lock().expect("registry lock").push(pgid);
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let mut killed = false;
    let status = loop {
        if INTERRUPTED.load(Ordering::SeqCst) && !killed {
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
            killed = true;
        }
        match child.wait_timeout(std::time::Duration::from_millis(50)) {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => continue,
            Err(err) => break Err(err),
        }
    };
    INFLIGHT
        .lock()
        .expect("registry lock")
        .retain(|p| *p != pgid);
    let status = status.map_err(|err| spawn_error(args, err))?;
    let stdout = stdout().trim().to_string();
    if !status.success() {
        return Err(GitError {
            argv: argv(args),
            code: status.code(),
            stdout,
            stderr: quiet.then(stderr),
        });
    }
    Ok(stdout)
}

/// Run git, letting stderr (progress, errors) stream to the terminal.
///
/// Every call gets the timeouts, not just the ones that reach a remote: they
/// are inert for local work, and marking each remote call by hand is a thing
/// to get wrong.
pub fn git(args: &[&str], cwd: Option<&Path>) -> Result<String, GitError> {
    run_command(command(args, cwd), args, false)
}

/// Like `git`, but quiet: stderr is captured into the error rather than
/// streamed, for callers that are UIs which must not be written over.
pub fn git_quiet(args: &[&str], cwd: Option<&Path>) -> Result<String, GitError> {
    run_command(command(args, cwd), args, true)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;
    use crate::testutil::{git as fixture_git, test_env};

    fn env_of(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| *k == OsStr::new(key))
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    #[test]
    fn calls_cap_stalled_transfers() {
        let cmd = command_with(&["fetch", "origin"], None, false, false);

        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(
            args[..2],
            [OsStr::new("-c"), OsStr::new("http.lowSpeedLimit=1000")]
        );
        assert!(
            env_of(&cmd, "GIT_SSH_COMMAND")
                .unwrap()
                .contains("ServerAliveInterval")
        );
        assert_eq!(env_of(&cmd, "GIT_TERMINAL_PROMPT").unwrap(), "0");
    }

    #[test]
    fn a_configured_ssh_command_wins() {
        let cmd = command_with(&["fetch", "origin"], None, true, true);

        assert_eq!(env_of(&cmd, "GIT_SSH_COMMAND"), None);
        assert_eq!(env_of(&cmd, "GIT_TERMINAL_PROMPT"), None);
    }

    #[test]
    fn the_stall_config_reaches_git() {
        // Unmocked: proves the -c options sit where git accepts them.
        let env = test_env();
        let origin = env.origin();

        assert_eq!(
            git(&["config", "--get", "http.lowSpeedLimit"], Some(&origin)).unwrap(),
            "1000"
        );
    }

    #[test]
    fn git_still_reports_failure() {
        let env = test_env();
        let origin = env.origin();

        // Quiet here: streaming would print git's error over the test output.
        assert!(git_quiet(&["rev-parse", "--verify", "no-such-ref"], Some(&origin)).is_err());
    }

    #[test]
    fn git_quiet_returns_output_and_reports_failure() {
        let env = test_env();
        let origin = env.origin();

        assert_eq!(
            git_quiet(&["rev-parse", "--abbrev-ref", "HEAD"], Some(&origin)).unwrap(),
            "main"
        );

        let err = git_quiet(&["rev-parse", "--verify", "no-such-ref"], Some(&origin))
            .expect_err("bad ref must fail");
        assert!(err.stderr.as_deref().unwrap().contains("fatal"));
        assert!(
            err.to_string().contains("fatal"),
            "the message must carry git's stderr"
        );
    }

    #[test]
    fn calls_survive_a_deleted_working_directory() {
        // Sitting in a deleted directory (e.g. a removed context) must not
        // break git. The library never depends on the process cwd (every
        // call passes one, or defaults to /), so a doomed cwd is simulated
        // per call rather than by moving the whole test process there.
        let env = test_env();
        let origin = env.origin();

        let clone = env.root().join("clone");
        git_quiet(
            &["clone", &origin.to_string_lossy(), &clone.to_string_lossy()],
            None,
        )
        .unwrap();

        assert_eq!(
            git_quiet(&["rev-parse", "--abbrev-ref", "HEAD"], Some(&clone)).unwrap(),
            "main"
        );
    }

    #[test]
    fn fixture_git_commits_with_the_isolated_identity() {
        let env = test_env();
        let origin = env.origin();

        assert_eq!(fixture_git(&["log", "-1", "--format=%an"], &origin), "Test");
    }
}
