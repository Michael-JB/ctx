use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

use sha2::{Digest, Sha256};

use crate::contexts::Context;
use crate::git::new_command;
use crate::layout::{Node, SplitDirection, resolve_layout};
use crate::multiplexer::{Multiplexer, MultiplexerError, env_truthy, env_var};
use crate::shellrun::via_shell;

// macOS caps sockaddr_un paths at 104 bytes including the terminator, and
// zellij offers no working way to relocate its socket dir, so session names
// must be short enough for the socket path to fit.
// See https://github.com/zellij-org/zellij/issues/5081.
const SOCKET_PATH_MAX: usize = 103;

/// Longest session name whose zellij socket path still fits, if capped.
fn session_name_budget() -> Option<usize> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    // zellij 0.44 places sockets in <tmp>/zellij-<uid>/<contract version>/<name>.
    let uid = unsafe { libc::getuid() };
    let sock_dir = std::env::temp_dir()
        .join(format!("zellij-{uid}"))
        .join("contract_version_1");
    Some(SOCKET_PATH_MAX.saturating_sub(sock_dir.to_string_lossy().len()) - 1)
}

fn session_name(ctx: &Context) -> String {
    let name = format!("{}--{}", ctx.repo, ctx.name).replace(['.', ':'], "-");
    session_name_within(name, session_name_budget())
}

fn session_name_within(name: String, budget: Option<usize>) -> String {
    let Some(budget) = budget else {
        return name;
    };
    if name.len() <= budget {
        return name;
    }
    // Truncate over-budget names; a digest of the full name keeps them unique.
    let digest: String = Sha256::digest(name.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let keep = std::cmp::max(budget.saturating_sub(7), 1);
    format!("{}-{}", &name[..keep], &digest[..6])
}

fn kdl_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

fn render_node(node: &Node, cwd: &Path, indent: usize) -> Result<String, MultiplexerError> {
    let pad = "    ".repeat(indent);
    let split = match node {
        Node::Pane(pane) => {
            let argv = match &pane.command {
                Some(command) => shlex::split(command).ok_or_else(|| {
                    MultiplexerError(format!("pane command has unbalanced quoting: {command}"))
                })?,
                None => Vec::new(),
            };
            let mut line = format!("{pad}pane");
            if let Some(program) = argv.first() {
                line += &format!(" command={}", kdl_string(program));
            }
            line += &format!(" cwd={}", kdl_string(&cwd.to_string_lossy()));
            if pane.focus {
                line += " focus=true";
            }
            if argv.len() > 1 {
                let args = argv[1..]
                    .iter()
                    .map(|arg| kdl_string(arg))
                    .collect::<Vec<_>>()
                    .join(" ");
                line += &format!(" {{\n{pad}    args {args}\n{pad}}}");
            }
            return Ok(line);
        }
        Node::Split(split) => split,
    };
    // Zellij's split_direction names the split axis, not the arrangement:
    // "vertical" puts panes side by side, "horizontal" stacks them.
    let direction = match split.direction {
        SplitDirection::Row => "vertical",
        SplitDirection::Column => "horizontal",
    };
    let children = split
        .panes
        .iter()
        .map(|pane| render_node(pane, cwd, indent + 1))
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    Ok(format!(
        "{pad}pane split_direction=\"{direction}\" {{\n{children}\n{pad}}}"
    ))
}

fn render_layout(layout: &Node, cwd: &Path) -> Result<String, MultiplexerError> {
    Ok(format!(
        "layout {{\n    default_tab_template {{\n        pane size=1 borderless=true {{\n            plugin location=\"zellij:tab-bar\"\n        }}\n        children\n        pane size=2 borderless=true {{\n            plugin location=\"zellij:status-bar\"\n        }}\n    }}\n    tab {{\n{}\n    }}\n}}\n",
        render_node(layout, cwd, 2)?
    ))
}

/// The process environment with every ZELLIJ variable hidden.
///
/// Inside a session, zellij turns any --layout invocation into new tabs of
/// the current session and never reaches the attach subcommand; hiding the
/// session env makes the command run as if from outside.
fn env_without_zellij() -> Vec<(String, String)> {
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    #[cfg(test)]
    crate::testutil::overlay_env(&mut env);
    env.retain(|key, _| !key.starts_with("ZELLIJ"));
    env.into_iter().collect()
}

pub struct ZellijMultiplexer {
    layout: Node,
}

impl ZellijMultiplexer {
    pub fn new(layout: Node) -> ZellijMultiplexer {
        ZellijMultiplexer { layout }
    }

    fn write_layout_file(
        &self,
        ctx: &Context,
        values: Option<&HashMap<String, String>>,
    ) -> Result<String, MultiplexerError> {
        let to_mux_err = |err: std::io::Error| MultiplexerError(err.to_string());
        let mut file = tempfile::Builder::new()
            .prefix("ctx-")
            .suffix(".kdl")
            .disable_cleanup(true)
            .tempfile()
            .map_err(to_mux_err)?;
        let layout = via_shell(&resolve_layout(&self.layout, values)).map_err(to_mux_err)?;
        file.write_all(render_layout(&layout, &ctx.path)?.as_bytes())
            .map_err(to_mux_err)?;
        Ok(file.path().to_string_lossy().into_owned())
    }
}

impl Multiplexer for ZellijMultiplexer {
    fn can_open_in_place(&self) -> bool {
        // Inside zellij, open() re-points the current client and returns.
        env_truthy("ZELLIJ")
    }

    fn exists(&self, ctx: &Context) -> bool {
        let output = new_command("zellij")
            .args(["list-sessions", "--short"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        // Nonzero means no zellij server is running.
        let Ok(output) = output else {
            return false;
        };
        if !output.status.success() {
            return false;
        }
        let session = session_name(ctx);
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == session)
    }

    fn is_current(&self, ctx: &Context) -> bool {
        env_var("ZELLIJ_SESSION_NAME").as_deref() == Some(session_name(ctx).as_str())
    }

    fn create(
        &self,
        ctx: &Context,
        values: Option<&HashMap<String, String>>,
    ) -> Result<(), MultiplexerError> {
        if self.exists(ctx) {
            return Ok(());
        }
        let session = session_name(ctx);
        let layout_file = self.write_layout_file(ctx, values)?;
        let mut cmd = std::process::Command::new("zellij");
        cmd.env_clear().envs(env_without_zellij());
        let output = cmd
            .args([
                "--layout",
                &layout_file,
                "attach",
                "--create-background",
                &session,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|err| MultiplexerError(err.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = if stderr.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                stderr.trim().to_string()
            };
            let message = format!("zellij could not create '{session}'");
            return Err(MultiplexerError(if detail.is_empty() {
                message
            } else {
                format!("{message}: {detail}")
            }));
        }
        Ok(())
    }

    fn open(
        &self,
        ctx: &Context,
        values: Option<&HashMap<String, String>>,
    ) -> Result<(), MultiplexerError> {
        use std::os::unix::process::CommandExt;

        let session = session_name(ctx);
        let exists = self.exists(ctx);
        if env_truthy("ZELLIJ") {
            // A nested `zellij attach` cannot run inside a session, so re-point
            // the already-attached client instead (zellij >= 0.44). The layout
            // only takes effect when the target session doesn't exist yet.
            let mut args = vec![
                "action".to_string(),
                "switch-session".to_string(),
                session.clone(),
            ];
            if !exists {
                args.push("--layout".to_string());
                args.push(self.write_layout_file(ctx, values)?);
            }
            let output = new_command("zellij")
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .map_err(|err| MultiplexerError(err.to_string()))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let detail = if stderr.trim().is_empty() {
                    stdout.trim().to_string()
                } else {
                    stderr.trim().to_string()
                };
                let message = format!("zellij could not switch to '{session}'");
                return Err(MultiplexerError(if detail.is_empty() {
                    message
                } else {
                    format!("{message}: {detail}")
                }));
            }
            return Ok(());
        }
        let err = if exists {
            new_command("zellij").args(["attach", &session]).exec()
        } else {
            let layout_file = self.write_layout_file(ctx, values)?;
            new_command("zellij")
                .args([
                    "--session",
                    &session,
                    "--new-session-with-layout",
                    &layout_file,
                ])
                .exec()
        };
        Err(MultiplexerError(format!("could not exec zellij: {err}")))
    }

    fn kill(&self, ctx: &Context) -> Result<(), MultiplexerError> {
        let session = session_name(ctx);
        let output = new_command("zellij")
            .args(["delete-session", "--force", &session])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|err| MultiplexerError(err.to_string()))?;
        if !output.status.success() {
            return Err(super::CmdError {
                argv: vec![
                    "zellij".to_string(),
                    "delete-session".to_string(),
                    "--force".to_string(),
                    session,
                ],
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }
            .into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::layout::{Pane, Split};
    use crate::testutil::{push_env, test_env};

    fn ctx(repo: &str, name: &str) -> Context {
        Context {
            repo: repo.to_string(),
            name: name.to_string(),
            path: PathBuf::from("/w"),
        }
    }

    fn pane(command: &str) -> Node {
        Node::Pane(Pane {
            command: Some(command.to_string()),
            ..Pane::default()
        })
    }

    #[test]
    fn session_name_replaces_forbidden_characters() {
        assert_eq!(
            session_name_within("my.repo--a:b".replace(['.', ':'], "-"), None),
            "my-repo--a-b"
        );
    }

    #[test]
    fn session_name_within_budget_is_unchanged() {
        assert_eq!(
            session_name_within("repo--short".to_string(), Some(20)),
            "repo--short"
        );
    }

    #[test]
    fn session_name_over_budget_is_shortened() {
        let name = session_name_within("repo--a-very-long-context-name".to_string(), Some(20));

        assert_eq!(name.len(), 20);
        assert!(name.starts_with("repo--a-very-"));
    }

    #[test]
    fn shortened_session_names_stay_unique() {
        let first = session_name_within("repo--a-very-long-context-name".to_string(), Some(20));
        let second = session_name_within("repo--a-very-long-context-nam2".to_string(), Some(20));

        assert_ne!(first, second);
    }

    #[test]
    fn is_current_matches_the_session_env() {
        let mux = ZellijMultiplexer::new(Node::Pane(Pane::default()));
        let context = ctx("repo", "a");

        {
            let _env = push_env("ZELLIJ_SESSION_NAME", &session_name(&context));
            assert!(mux.is_current(&context));
        }
        {
            let _env = push_env("ZELLIJ_SESSION_NAME", "elsewhere");
            assert!(!mux.is_current(&context));
        }
    }

    #[test]
    fn layout_puts_pane_in_cwd() {
        let out = render_layout(&Node::Pane(Pane::default()), Path::new("/w")).unwrap();

        assert!(out.contains("pane cwd=\"/w\""));
    }

    #[test]
    fn layout_splits_command_into_args() {
        let out = render_layout(&pane("nvim -R file.txt"), Path::new("/w")).unwrap();

        assert!(out.contains("pane command=\"nvim\" cwd=\"/w\""));
        assert!(out.contains("args \"-R\" \"file.txt\""));
    }

    #[test]
    fn create_makes_a_background_session() {
        let env = test_env();
        let args_log = env.root().join("zellij-args.log");
        let env_log = env.root().join("zellij-env.log");
        // Only the create invocation must hide the session env; the exists()
        // probe legitimately runs with it.
        let script = format!(
            "{{ printf '%s\\037' \"$@\"; printf '\\n'; }} >> {args}\n\
             case \"$*\" in *--create-background*) printenv | grep '^ZELLIJ' >> {env} || true;; esac\n\
             exit 0",
            args = args_log.display(),
            env = env_log.display()
        );
        let _guard = env.fake_cli("zellij", &script);
        let _zellij = push_env("ZELLIJ", "0");
        let _session = push_env("ZELLIJ_SESSION_NAME", "elsewhere");
        let mux = ZellijMultiplexer::new(Node::Pane(Pane::default()));

        mux.create(&ctx("repo", "a"), Some(&HashMap::new()))
            .unwrap();

        let text = std::fs::read_to_string(&args_log).unwrap();
        let create_line = text
            .lines()
            .find(|line| line.contains("--create-background"))
            .expect("create invocation logged");
        let args: Vec<&str> = create_line
            .split('\x1f')
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(args[0], "--layout");
        assert_eq!(args[2..], ["attach", "--create-background", "repo--a"]);
        // With the session env visible, zellij would open the layout as new
        // tabs of the current session instead of creating one.
        let leaked = std::fs::read_to_string(&env_log).unwrap_or_default();
        assert_eq!(leaked.trim(), "");
    }

    #[test]
    fn layout_file_resolves_builtin_panes() {
        let mux = ZellijMultiplexer::new(Node::Pane(Pane {
            builtin: Some("claude".to_string()),
            ..Pane::default()
        }));
        let values = HashMap::from([("prompt".to_string(), "explore x".to_string())]);

        let layout_file = mux
            .write_layout_file(&ctx("repo", "a"), Some(&values))
            .unwrap();

        let content = std::fs::read_to_string(&layout_file).unwrap();
        assert!(content.contains("command=\"sh\""));
        let args_start = content.find("args \"").expect("args in layout") + 6;
        let script_path =
            &content[args_start..content[args_start..].find('"').unwrap() + args_start];
        let script = std::fs::read_to_string(script_path).unwrap();
        assert!(script.contains("ctx builtin claude trust; exec claude '\"'\"'explore x'\"'\"'"));
    }

    #[test]
    fn layout_escapes_kdl_strings() {
        let out = render_layout(&pane("claude 'say \"hi\"'"), Path::new("/w")).unwrap();

        assert!(out.contains("args \"say \\\"hi\\\"\""));
    }

    #[test]
    fn layout_marks_focus() {
        let node = Node::Pane(Pane {
            command: Some("nvim".to_string()),
            focus: true,
            ..Pane::default()
        });

        assert!(
            render_layout(&node, Path::new("/w"))
                .unwrap()
                .contains("focus=true")
        );
    }

    #[test]
    fn layout_maps_row_to_vertical_split() {
        let node = Node::Split(Split {
            direction: SplitDirection::Row,
            panes: vec![Node::Pane(Pane::default()), Node::Pane(Pane::default())],
        });

        let out = render_layout(&node, Path::new("/w")).unwrap();

        assert!(out.contains("split_direction=\"vertical\""));
    }

    #[test]
    fn layout_maps_column_to_horizontal_split() {
        let node = Node::Split(Split {
            direction: SplitDirection::Column,
            panes: vec![Node::Pane(Pane::default()), Node::Pane(Pane::default())],
        });

        let out = render_layout(&node, Path::new("/w")).unwrap();

        assert!(out.contains("split_direction=\"horizontal\""));
    }
}
