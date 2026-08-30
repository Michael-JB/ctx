//! Run pane commands through the user's interactive shell.
//!
//! Multiplexers exec a pane's command directly, bypassing the shell, so
//! prompt-hook environment loaders (direnv, mise, ...) never run for the
//! pane. via_shell defers each command to a launcher script instead: the
//! user's shell starts interactively with the script as stdin, sources its
//! rc files and fires its prompt hooks, and only then reads the script's
//! single line, which execs the command with stdin re-pointed at the
//! pane's tty.

use std::io::Write;

use crate::layout::{Node, Pane, Split};

// XXX Assumes $SHELL is a POSIX-family shell that runs its prompt hooks
// before the first read from a non-tty stdin (holds for zsh and bash;
// fish untested, non-POSIX shells would break).
const LAUNCHER_HEAD: &str = r#"#!/bin/sh
shell="${SHELL:-/bin/sh}"
case "${shell##*/}" in
    zsh)
        # zsh's line editor reads the tty directly, ignoring a non-tty
        # stdin; +o zle makes zsh read the heredoc.
        set -- -i +o zle
        ;;
    *)
        set -- -i
        ;;
esac
# Hand the pane tty down on fd 9 rather than reopening /dev/tty: macOS
# kqueue cannot watch the /dev/tty alias, leaving kqueue-polling TUIs
# (anything on Node) deaf to input.
exec "$shell" "$@" 9<&0 <<'CTX_PANE_COMMAND'
"#;

/// POSIX shell quoting with Python shlex.quote's exact output shape.
pub fn quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if s.chars().all(safe) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r#"'"'"'"#))
}

fn launcher(command: &str) -> std::io::Result<String> {
    let mut file = tempfile::Builder::new()
        .prefix("ctx-pane-")
        .suffix(".sh")
        .disable_cleanup(true)
        .tempfile()?;
    write!(
        file,
        "{LAUNCHER_HEAD}exec {command} <&9 9<&-\nCTX_PANE_COMMAND\n"
    )?;
    Ok(format!("sh {}", quote(&file.path().to_string_lossy())))
}

/// Defer each command pane to a launcher run by the user's shell.
pub fn via_shell(node: &Node) -> std::io::Result<Node> {
    Ok(match node {
        Node::Pane(pane) => match &pane.command {
            None => node.clone(),
            Some(command) => Node::Pane(Pane {
                command: Some(launcher(command)?),
                focus: pane.focus,
                ..Pane::default()
            }),
        },
        Node::Split(split) => Node::Split(Split {
            direction: split.direction,
            panes: split
                .panes
                .iter()
                .map(via_shell)
                .collect::<Result<_, _>>()?,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::SplitDirection;

    fn script(node: &Node) -> String {
        let Node::Pane(pane) = node else {
            panic!("expected a pane")
        };
        let command = pane.command.as_deref().expect("pane has a command");
        let argv = shlex::split(command).expect("command parses");
        assert_eq!(argv[0], "sh");
        std::fs::read_to_string(&argv[1]).expect("launcher script exists")
    }

    fn command_pane(command: &str) -> Node {
        Node::Pane(Pane {
            command: Some(command.to_string()),
            ..Pane::default()
        })
    }

    #[test]
    fn leaves_plain_shell_panes_alone() {
        let pane = Node::Pane(Pane {
            focus: true,
            ..Pane::default()
        });
        assert_eq!(via_shell(&pane).unwrap(), pane);
    }

    #[test]
    fn defers_the_command_to_a_launcher_script() {
        let pane = Node::Pane(Pane {
            command: Some("nvim -R file.txt".to_string()),
            focus: true,
            ..Pane::default()
        });

        let wrapped = via_shell(&pane).unwrap();

        let Node::Pane(ref inner) = wrapped else {
            panic!("expected a pane")
        };
        assert!(inner.focus);
        let script = script(&wrapped);
        assert!(script.contains("exec nvim -R file.txt <&9 9<&-"));
        assert!(script.contains(r#""$shell" "$@""#));
    }

    #[test]
    fn starts_the_shell_interactively() {
        assert!(script(&via_shell(&command_pane("htop")).unwrap()).contains("set -- -i"));
    }

    #[test]
    fn recurses_into_splits() {
        let split = Node::Split(Split {
            direction: SplitDirection::Row,
            panes: vec![command_pane("nvim"), Node::Pane(Pane::default())],
        });

        let wrapped = via_shell(&split).unwrap();

        let Node::Split(split) = wrapped else {
            panic!("expected a split")
        };
        let Node::Pane(first) = &split.panes[0] else {
            panic!("expected a pane")
        };
        assert!(first.command.as_deref().unwrap().starts_with("sh "));
        assert_eq!(split.panes[1], Node::Pane(Pane::default()));
    }

    #[test]
    fn writes_a_fresh_script_per_pane() {
        let first = via_shell(&command_pane("nvim")).unwrap();
        let second = via_shell(&command_pane("nvim")).unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn quote_matches_python_shlex() {
        assert_eq!(quote(""), "''");
        assert_eq!(quote("plain-word_1.txt"), "plain-word_1.txt");
        assert_eq!(quote("two words"), "'two words'");
        assert_eq!(quote("it's"), r#"'it'"'"'s'"#);
    }
}
