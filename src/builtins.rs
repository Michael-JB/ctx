//! Pane builtins: named panes whose commands ctx composes itself.
//!
//! A builtin stands in for a pane's command string, letting ctx adapt the
//! invocation to the occasion. Each implementation's specifics live here,
//! behind the builtin's name.

use std::collections::HashMap;

use crate::shellrun::quote;

fn claude(args: Option<&str>, values: Option<&HashMap<String, String>>) -> String {
    let mut command = String::from("claude");
    if let Some(args) = args
        && !args.is_empty()
    {
        command = format!("{command} {args}");
    }
    match values {
        // A recreated session resumes the checkout's conversation.
        None => command.push_str(" --continue"),
        Some(values) => {
            if let Some(prompt) = values.get("prompt") {
                command = format!("{command} {}", quote(prompt));
            }
        }
    }
    // Pre-trust the checkout so the session doesn't stop at the trust dialog.
    // Run through `sh` so the composed line parses the same everywhere: the
    // shell that ends up reading it may not speak POSIX quoting (fish).
    format!(
        "sh -c {}",
        quote(&format!("ctx builtin claude trust; exec {command}"))
    )
}

pub const PANE_BUILTINS: &[&str] = &["claude"];

/// The values a builtin consumes at context creation.
pub fn builtin_keys(name: &str) -> &'static [&'static str] {
    match name {
        "claude" => &["prompt"],
        _ => &[],
    }
}

/// The command a builtin pane runs.
///
/// `values` carries creation-time key=value data; None means the session
/// is being recreated for an existing context.
pub fn builtin_command(
    name: &str,
    args: Option<&str>,
    values: Option<&HashMap<String, String>>,
) -> String {
    match name {
        "claude" => claude(args, values),
        _ => unreachable!("unknown builtin '{name}' survived layout parsing"),
    }
}
