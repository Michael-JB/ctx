use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use super::CmdError;
use crate::contexts::Context;
use crate::git::new_command;
use crate::layout::{Node, Pane, SplitDirection, resolve_layout};
use crate::multiplexer::{Multiplexer, MultiplexerError, env_truthy};
use crate::shellrun::via_shell;

fn session_name(ctx: &Context) -> String {
    let raw = format!("{}--{}", ctx.repo, ctx.name);
    // tmux forbids '.' and ':' in session names.
    raw.replace(['.', ':'], "-")
}

fn tmux(args: &[&str]) -> Result<String, CmdError> {
    let argv = || {
        ["tmux"]
            .iter()
            .copied()
            .chain(args.iter().copied())
            .map(str::to_string)
            .collect()
    };
    let output = new_command("tmux")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| CmdError {
            argv: argv(),
            stderr: err.to_string(),
        })?;
    if !output.status.success() {
        return Err(CmdError {
            argv: argv(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The leaf a split hands its original region to.
fn first_pane(node: &Node) -> &Pane {
    match node {
        Node::Pane(pane) => pane,
        Node::Split(split) => first_pane(&split.panes[0]),
    }
}

/// Subdivide pane_id according to the layout, returning (pane_id, pane) leaves.
fn build<'a>(
    node: &'a Node,
    pane_id: &str,
    cwd: &Path,
) -> Result<Vec<(String, &'a Pane)>, CmdError> {
    let split = match node {
        Node::Pane(pane) => return Ok(vec![(pane_id.to_string(), pane)]),
        Node::Split(split) => split,
    };
    let flag = match split.direction {
        SplitDirection::Row => "-h",
        SplitDirection::Column => "-v",
    };
    let cwd_str = cwd.to_string_lossy();
    let mut regions = vec![pane_id.to_string()];
    for child in &split.panes[1..] {
        let target = regions.last().expect("regions starts non-empty").clone();
        let mut args = vec![
            "split-window",
            flag,
            "-t",
            &target,
            "-c",
            &cwd_str,
            "-P",
            "-F",
            "#{pane_id}",
        ];
        let command = &first_pane(child).command;
        if let Some(command) = command {
            args.push(command);
        }
        regions.push(tmux(&args)?);
    }
    let mut leaves = Vec::new();
    for (child, region) in split.panes.iter().zip(&regions) {
        leaves.extend(build(child, region, cwd)?);
    }
    Ok(leaves)
}

fn create_session(session: &str, cwd: &Path, layout: &Node) -> Result<(), MultiplexerError> {
    // Commands run as the panes' start commands: delivering them by typing
    // into a shell instead races its startup, and the kernel's canonical
    // line buffer truncates what arrives too early to 1024 bytes on macOS.
    let cwd_str = cwd.to_string_lossy();
    let mut args = vec![
        "new-session",
        "-d",
        "-s",
        session,
        "-c",
        &cwd_str,
        "-P",
        "-F",
        "#{pane_id}",
    ];
    let command = &first_pane(layout).command;
    if let Some(command) = command {
        args.push(command);
    }
    let built = tmux(&args).and_then(|first| build(layout, &first, cwd));
    let leaves = match built {
        Ok(leaves) => leaves,
        Err(err) => {
            let _ = tmux(&["kill-session", "-t", &format!("={session}")]);
            if err.stderr.contains("command too long") {
                return Err(MultiplexerError(
                    "a pane command exceeds tmux's ~16KB limit".to_string(),
                ));
            }
            return Err(err.into());
        }
    };
    let focused = leaves
        .iter()
        .find(|(_, pane)| pane.focus)
        .or_else(|| leaves.first())
        .map(|(pane_id, _)| pane_id.clone())
        .expect("a layout always has at least one pane");
    tmux(&["select-pane", "-t", &focused])?;
    Ok(())
}

pub struct TmuxMultiplexer {
    layout: Node,
}

impl TmuxMultiplexer {
    pub fn new(layout: Node) -> TmuxMultiplexer {
        TmuxMultiplexer { layout }
    }
}

impl Multiplexer for TmuxMultiplexer {
    fn can_open_in_place(&self) -> bool {
        // Inside tmux, open() switches the client and returns.
        env_truthy("TMUX")
    }

    fn exists(&self, ctx: &Context) -> bool {
        new_command("tmux")
            .args(["has-session", "-t", &format!("={}", session_name(ctx))])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn is_current(&self, ctx: &Context) -> bool {
        if !env_truthy("TMUX") {
            return false;
        }
        tmux(&["display-message", "-p", "#S"])
            .map(|session| session == session_name(ctx))
            .unwrap_or(false)
    }

    fn create(
        &self,
        ctx: &Context,
        values: Option<&HashMap<String, String>>,
    ) -> Result<(), MultiplexerError> {
        if !self.exists(ctx) {
            let layout = via_shell(&resolve_layout(&self.layout, values))
                .map_err(|err| MultiplexerError(err.to_string()))?;
            create_session(&session_name(ctx), &ctx.path, &layout)?;
        }
        Ok(())
    }

    fn open(
        &self,
        ctx: &Context,
        values: Option<&HashMap<String, String>>,
    ) -> Result<(), MultiplexerError> {
        let session = session_name(ctx);
        self.create(ctx, values)?;
        if env_truthy("TMUX") {
            tmux(&["switch-client", "-t", &format!("={session}")])?;
            Ok(())
        } else {
            use std::os::unix::process::CommandExt;

            let err = new_command("tmux")
                .args(["attach-session", "-t", &format!("={session}")])
                .exec();
            Err(MultiplexerError(format!("could not exec tmux: {err}")))
        }
    }

    fn kill(&self, ctx: &Context) -> Result<(), MultiplexerError> {
        tmux(&["kill-session", "-t", &format!("={}", session_name(ctx))])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::testutil::{push_env, test_env};

    fn ctx(repo: &str, name: &str) -> Context {
        Context {
            repo: repo.to_string(),
            name: name.to_string(),
            path: PathBuf::from("/w"),
        }
    }

    /// A tmux stub that logs every invocation (unit-separated args, one call
    /// per line) and answers -P queries with a fresh pane id.
    fn stub_tmux(
        env: &crate::testutil::TestEnv,
        extra: &str,
    ) -> (PathBuf, crate::testutil::EnvGuard) {
        let log = env.root().join("tmux.log");
        let script = format!(
            "{{ printf '%s\\037' \"$@\"; printf '\\n'; }} >> {log}\n{extra}\necho \"%$(grep -c '' {log})\"",
            log = log.display()
        );
        let guard = env.fake_cli("tmux", &script);
        (log, guard)
    }

    fn calls(log: &Path) -> Vec<Vec<String>> {
        let Ok(text) = std::fs::read_to_string(log) else {
            return Vec::new();
        };
        text.lines()
            .map(|line| {
                line.split('\x1f')
                    .filter(|part| !part.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn session_name_replaces_forbidden_characters() {
        assert_eq!(session_name(&ctx("my.repo", "a:b")), "my-repo--a-b");
    }

    #[test]
    fn is_current_is_false_outside_tmux() {
        let _tmux_unset = push_env("TMUX", "");

        assert!(!TmuxMultiplexer::new(Node::Pane(Pane::default())).is_current(&ctx("repo", "a")));
    }

    #[test]
    fn is_current_compares_the_attached_session() {
        let env = test_env();
        let _guard = env.fake_cli("tmux", "echo 'repo--a'");
        let _tmux = push_env("TMUX", "/tmp/tmux-1/default,1,0");
        let mux = TmuxMultiplexer::new(Node::Pane(Pane::default()));

        assert!(mux.is_current(&ctx("repo", "a")));
        assert!(!mux.is_current(&ctx("repo", "b")));
    }

    #[test]
    fn open_resolves_builtin_panes_on_session_creation() {
        let env = test_env();
        // has-session fails: the session does not exist yet.
        let (log, _guard) = stub_tmux(&env, "[ \"$1\" = has-session ] && exit 1");
        let _tmux = push_env("TMUX", "/tmp/tmux-1/default,1,0");
        let layout = Node::Pane(Pane {
            builtin: Some("claude".to_string()),
            ..Pane::default()
        });
        let mux = TmuxMultiplexer::new(layout);
        let values = HashMap::from([("prompt".to_string(), "hi".to_string())]);

        mux.open(&ctx("repo", "a"), Some(&values)).unwrap();

        let new_sessions: Vec<_> = calls(&log)
            .into_iter()
            .filter(|call| call[0] == "new-session")
            .collect();
        assert_eq!(new_sessions.len(), 1);
        let launcher = shlex::split(new_sessions[0].last().unwrap()).unwrap();
        assert_eq!(launcher[0], "sh");
        let script = std::fs::read_to_string(&launcher[1]).unwrap();
        assert!(script.contains("exec sh -c 'ctx builtin claude trust; exec claude hi' <&9 9<&-"));
    }

    #[test]
    fn split_panes_start_their_own_commands() {
        let env = test_env();
        let (log, _guard) = stub_tmux(&env, "");
        let layout = Node::Split(crate::layout::Split {
            direction: SplitDirection::Row,
            panes: vec![
                Node::Pane(Pane {
                    command: Some("nvim".to_string()),
                    ..Pane::default()
                }),
                Node::Pane(Pane::default()),
                Node::Pane(Pane {
                    command: Some("htop".to_string()),
                    ..Pane::default()
                }),
            ],
        });

        create_session("s", Path::new("/w"), &layout).unwrap();

        let calls = calls(&log);
        let new_sessions: Vec<_> = calls.iter().filter(|c| c[0] == "new-session").collect();
        assert_eq!(new_sessions.len(), 1);
        assert_eq!(new_sessions[0].last().unwrap(), "nvim");
        let splits: Vec<_> = calls.iter().filter(|c| c[0] == "split-window").collect();
        assert_eq!(splits.len(), 2);
        assert_eq!(splits[0].last().unwrap(), "#{pane_id}");
        assert_eq!(splits[1].last().unwrap(), "htop");
    }

    #[test]
    fn reports_an_over_long_pane_command() {
        let env = test_env();
        let log = env.root().join("tmux.log");
        let script = format!(
            "{{ printf '%s\\037' \"$@\"; printf '\\n'; }} >> {log}\n\
             [ \"$1\" = new-session ] && {{ echo 'command too long' >&2; exit 1; }}\n\
             exit 0",
            log = log.display()
        );
        let _guard = env.fake_cli("tmux", &script);
        let layout = Node::Pane(Pane {
            command: Some(format!("claude {}", "x".repeat(20_000))),
            ..Pane::default()
        });

        let err = create_session("s", Path::new("/w"), &layout).expect_err("must fail");

        assert!(err.to_string().contains("16KB"));
        let kills: Vec<_> = calls(&log)
            .into_iter()
            .filter(|call| call[0] == "kill-session")
            .collect();
        assert_eq!(kills, vec![vec!["kill-session", "-t", "=s"]]);
    }

    #[test]
    fn create_does_not_attach() {
        let env = test_env();
        let (log, _guard) = stub_tmux(&env, "[ \"$1\" = has-session ] && exit 1");
        let _tmux = push_env("TMUX", "/tmp/tmux-1/default,1,0");
        let mux = TmuxMultiplexer::new(Node::Pane(Pane::default()));

        mux.create(&ctx("repo", "a"), Some(&HashMap::new()))
            .unwrap();

        let calls = calls(&log);
        assert!(calls.iter().any(|call| call[0] == "new-session"));
        assert!(
            !calls
                .iter()
                .any(|call| call[0] == "switch-client" || call[0] == "attach-session")
        );
    }
}
