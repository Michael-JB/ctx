use std::collections::{HashMap, HashSet};

use toml::{Table, Value};

use crate::builtins;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct LayoutError(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Row,    // panes side by side
    Column, // panes stacked
}

impl SplitDirection {
    const ALL: &[(&str, SplitDirection)] = &[
        ("row", SplitDirection::Row),
        ("column", SplitDirection::Column),
    ];

    fn parse(raw: &str) -> Option<SplitDirection> {
        Self::ALL
            .iter()
            .find(|(name, _)| *name == raw)
            .map(|(_, direction)| *direction)
    }

    fn names() -> String {
        Self::ALL
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pane {
    pub command: Option<String>, // None means a plain shell (unless a builtin is set)
    pub builtin: Option<String>, // a builtin standing in for a command
    pub args: Option<String>,    // extra flags appended to a builtin's command
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub direction: SplitDirection,
    pub panes: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Pane(Pane),
    Split(Split),
}

pub fn default_layout() -> Node {
    Node::Pane(Pane::default())
}

pub fn parse_layout(data: &Table) -> Result<Node, LayoutError> {
    let node = parse_node(data)?;
    if count_focus(&node) > 1 {
        return Err(LayoutError("at most one pane may set focus".to_string()));
    }
    Ok(node)
}

fn unknown_keys(data: &Table, known: &[&str]) -> Option<String> {
    let mut unknown: Vec<&str> = data
        .keys()
        .map(String::as_str)
        .filter(|key| !known.contains(key))
        .collect();
    if unknown.is_empty() {
        return None;
    }
    unknown.sort_unstable();
    Some(unknown.join(", "))
}

/// A TOML value as the string Python's `str()` coercion would make of it.
fn coerce_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Python-style truthiness for TOML values.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Boolean(b) => *b,
        Value::Integer(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Table(t) => !t.is_empty(),
        Value::Datetime(_) => true,
    }
}

fn parse_node(data: &Table) -> Result<Node, LayoutError> {
    if data.contains_key("split") {
        if let Some(unknown) = unknown_keys(data, &["split", "panes"]) {
            return Err(LayoutError(format!("unknown split key(s): {unknown}")));
        }
        let raw = coerce_string(&data["split"]);
        let Some(direction) = SplitDirection::parse(&raw) else {
            return Err(LayoutError(format!(
                "split must be one of {}, got '{raw}'",
                SplitDirection::names()
            )));
        };
        let panes = match data.get("panes") {
            Some(Value::Array(panes)) if !panes.is_empty() => panes,
            _ => {
                return Err(LayoutError(
                    "a split needs a non-empty 'panes' list".to_string(),
                ));
            }
        };
        let panes = panes
            .iter()
            .map(|pane| match pane {
                Value::Table(table) => parse_node(table),
                _ => Err(LayoutError(
                    "each entry in 'panes' must be a table".to_string(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Node::Split(Split { direction, panes }));
    }
    if let Some(unknown) = unknown_keys(data, &["command", "builtin", "args", "focus"]) {
        return Err(LayoutError(format!("unknown pane key(s): {unknown}")));
    }
    if data.contains_key("command") && data.contains_key("builtin") {
        return Err(LayoutError(
            "a pane takes either a command or a builtin, not both".to_string(),
        ));
    }
    let builtin = data.get("builtin").map(coerce_string);
    if let Some(builtin) = &builtin
        && !builtins::PANE_BUILTINS.contains(&builtin.as_str())
    {
        return Err(LayoutError(format!(
            "unknown pane builtin '{builtin}' (supported: {})",
            builtins::PANE_BUILTINS.join(", ")
        )));
    }
    if data.contains_key("args") && !data.contains_key("builtin") {
        return Err(LayoutError("pane args require a builtin".to_string()));
    }
    Ok(Node::Pane(Pane {
        command: data.get("command").map(coerce_string),
        builtin,
        args: data.get("args").map(coerce_string),
        focus: data.get("focus").is_some_and(truthy),
    }))
}

fn count_focus(node: &Node) -> usize {
    match node {
        Node::Pane(pane) => pane.focus as usize,
        Node::Split(split) => split.panes.iter().map(count_focus).sum(),
    }
}

/// All creation-time value keys the layout's builtin panes consume.
pub fn accepted_keys(node: &Node) -> HashSet<&'static str> {
    match node {
        Node::Pane(pane) => match &pane.builtin {
            Some(builtin) => builtins::builtin_keys(builtin).iter().copied().collect(),
            None => HashSet::new(),
        },
        Node::Split(split) => split.panes.iter().flat_map(accepted_keys).collect(),
    }
}

/// Concretise builtin panes into command panes.
///
/// `values` carries creation-time key=value data for the builtins; None
/// means the session is being recreated for an existing context.
pub fn resolve_layout(node: &Node, values: Option<&HashMap<String, String>>) -> Node {
    match node {
        Node::Pane(pane) => match &pane.builtin {
            None => node.clone(),
            Some(builtin) => Node::Pane(Pane {
                command: Some(builtins::builtin_command(
                    builtin,
                    pane.args.as_deref(),
                    values,
                )),
                focus: pane.focus,
                ..Pane::default()
            }),
        },
        Node::Split(split) => Node::Split(Split {
            direction: split.direction,
            panes: split
                .panes
                .iter()
                .map(|pane| resolve_layout(pane, values))
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Node, LayoutError> {
        let table: Table = toml::from_str(text).expect("valid TOML");
        parse_layout(&table)
    }

    fn pane(command: &str) -> Node {
        Node::Pane(Pane {
            command: Some(command.to_string()),
            ..Pane::default()
        })
    }

    fn err(text: &str) -> String {
        parse(text).expect_err("layout must be rejected").0
    }

    #[test]
    fn empty_pane_gives_defaults() {
        assert_eq!(parse("").unwrap(), Node::Pane(Pane::default()));
    }

    #[test]
    fn pane_takes_command_and_focus() {
        let node = parse("command = \"nvim\"\nfocus = true").unwrap();

        assert_eq!(
            node,
            Node::Pane(Pane {
                command: Some("nvim".to_string()),
                focus: true,
                ..Pane::default()
            })
        );
    }

    #[test]
    fn split_holds_panes() {
        let node = parse("split = \"row\"\npanes = [{}, { command = \"htop\" }]").unwrap();

        assert_eq!(
            node,
            Node::Split(Split {
                direction: SplitDirection::Row,
                panes: vec![Node::Pane(Pane::default()), pane("htop")],
            })
        );
    }

    #[test]
    fn splits_nest() {
        let node = parse("split = \"column\"\npanes = [{ split = \"row\", panes = [{}, {}] }, {}]")
            .unwrap();

        assert_eq!(
            node,
            Node::Split(Split {
                direction: SplitDirection::Column,
                panes: vec![
                    Node::Split(Split {
                        direction: SplitDirection::Row,
                        panes: vec![Node::Pane(Pane::default()), Node::Pane(Pane::default())],
                    }),
                    Node::Pane(Pane::default()),
                ],
            })
        );
    }

    #[test]
    fn pane_takes_a_builtin_with_args() {
        let node = parse("builtin = \"claude\"\nargs = \"--model opus\"\nfocus = true").unwrap();

        assert_eq!(
            node,
            Node::Pane(Pane {
                builtin: Some("claude".to_string()),
                args: Some("--model opus".to_string()),
                focus: true,
                ..Pane::default()
            })
        );
    }

    #[test]
    fn pane_rejects_command_and_builtin_together() {
        assert!(
            err("command = \"claude\"\nbuiltin = \"claude\"")
                .contains("either a command or a builtin")
        );
    }

    #[test]
    fn pane_rejects_an_unknown_builtin() {
        assert!(err("builtin = \"clod\"").contains("unknown pane builtin 'clod'"));
    }

    #[test]
    fn pane_rejects_args_without_a_builtin() {
        assert!(err("command = \"nvim\"\nargs = \"-R\"").contains("args require a builtin"));
    }

    #[test]
    fn unknown_pane_key_rejected() {
        assert!(err("comand = \"nvim\"").contains("unknown pane key"));
    }

    #[test]
    fn unknown_split_key_rejected() {
        assert!(err("split = \"row\"\npanes = [{}]\nfocus = true").contains("unknown split key"));
    }

    #[test]
    fn unknown_direction_rejected() {
        assert!(err("split = \"diagonal\"\npanes = [{}]").contains("split must be one of"));
    }

    #[test]
    fn empty_panes_rejected() {
        assert!(err("split = \"row\"\npanes = []").contains("non-empty 'panes'"));
    }

    #[test]
    fn missing_panes_rejected() {
        assert!(err("split = \"row\"").contains("non-empty 'panes'"));
    }

    #[test]
    fn multiple_focus_rejected() {
        assert!(
            err("split = \"row\"\npanes = [{ focus = true }, { focus = true }]")
                .contains("at most one pane")
        );
    }

    fn builtin_pane(builtin: &str, args: Option<&str>, focus: bool) -> Node {
        Node::Pane(Pane {
            builtin: Some(builtin.to_string()),
            args: args.map(str::to_string),
            focus,
            ..Pane::default()
        })
    }

    #[test]
    fn accepted_keys_collects_builtin_keys_across_the_tree() {
        let node = Node::Split(Split {
            direction: SplitDirection::Row,
            panes: vec![
                builtin_pane("claude", None, false),
                pane("nvim"),
                Node::Pane(Pane::default()),
            ],
        });

        assert_eq!(accepted_keys(&node), HashSet::from(["prompt"]));
    }

    #[test]
    fn accepted_keys_is_empty_without_builtins() {
        assert_eq!(accepted_keys(&pane("claude")), HashSet::new());
    }

    const TRUST: &str = "ctx builtin claude trust";

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn focused_pane(command: &str) -> Node {
        Node::Pane(Pane {
            command: Some(command.to_string()),
            focus: true,
            ..Pane::default()
        })
    }

    #[test]
    fn resolve_claude_passes_the_prompt_as_one_word() {
        let node = resolve_layout(
            &builtin_pane("claude", None, true),
            Some(&values(&[("prompt", "explore the bug")])),
        );

        let quoted_prompt = r#"'"'"'explore the bug'"'"'"#;
        assert_eq!(
            node,
            focused_pane(&format!("sh -c '{TRUST}; exec claude {quoted_prompt}'"))
        );
    }

    #[test]
    fn resolve_claude_without_a_prompt() {
        let node = resolve_layout(&builtin_pane("claude", None, false), Some(&values(&[])));

        assert_eq!(node, pane(&format!("sh -c '{TRUST}; exec claude'")));
    }

    #[test]
    fn resolve_claude_keeps_extra_args() {
        let node = resolve_layout(
            &builtin_pane("claude", Some("--model opus"), false),
            Some(&values(&[])),
        );

        assert_eq!(
            node,
            pane(&format!("sh -c '{TRUST}; exec claude --model opus'"))
        );
    }

    #[test]
    fn resolve_claude_on_a_recreated_session_resumes() {
        let node = resolve_layout(&builtin_pane("claude", None, false), None);

        assert_eq!(
            node,
            pane(&format!("sh -c '{TRUST}; exec claude --continue'"))
        );
    }

    #[test]
    fn resolve_claude_on_a_recreated_session_keeps_extra_args() {
        let node = resolve_layout(&builtin_pane("claude", Some("--model opus"), false), None);

        assert_eq!(
            node,
            pane(&format!(
                "sh -c '{TRUST}; exec claude --model opus --continue'"
            ))
        );
    }

    #[test]
    fn resolve_leaves_command_panes_alone() {
        let node = Node::Split(Split {
            direction: SplitDirection::Column,
            panes: vec![pane("nvim"), builtin_pane("claude", None, false)],
        });

        let resolved = resolve_layout(&node, Some(&values(&[("prompt", "x")])));

        assert_eq!(
            resolved,
            Node::Split(Split {
                direction: SplitDirection::Column,
                panes: vec![
                    pane("nvim"),
                    pane(&format!("sh -c '{TRUST}; exec claude x'"))
                ],
            })
        );
    }
}
