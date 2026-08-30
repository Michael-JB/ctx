use std::path::Path;
use std::process::{Command, Stdio};

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

fn command(args: &[&str], cwd: Option<&Path>) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(STALL_CONFIG)
        .args(args)
        .current_dir(cwd.unwrap_or(Path::new(SAFE_CWD)));
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", SSH_COMMAND);
    }
    // A prompt would block on a terminal the caller may not be showing.
    if std::env::var_os("GIT_TERMINAL_PROMPT").is_none() {
        cmd.env("GIT_TERMINAL_PROMPT", "0");
    }
    cmd
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

/// Run git, letting stderr (progress, errors) stream to the terminal.
///
/// Every call gets the timeouts, not just the ones that reach a remote: they
/// are inert for local work, and marking each remote call by hand is a thing
/// to get wrong.
pub fn git(args: &[&str], cwd: Option<&Path>) -> Result<String, GitError> {
    let output = command(args, cwd)
        .stdout(Stdio::piped())
        .output()
        .map_err(|err| spawn_error(args, err))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        return Err(GitError {
            argv: argv(args),
            code: output.status.code(),
            stdout,
            stderr: None,
        });
    }
    Ok(stdout)
}

/// Like `git`, but quiet: stderr is captured into the error rather than
/// streamed, for callers that are UIs which must not be written over.
pub fn git_quiet(args: &[&str], cwd: Option<&Path>) -> Result<String, GitError> {
    let output = command(args, cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| spawn_error(args, err))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        return Err(GitError {
            argv: argv(args),
            code: output.status.code(),
            stdout,
            stderr: Some(String::from_utf8_lossy(&output.stderr).into_owned()),
        });
    }
    Ok(stdout)
}
