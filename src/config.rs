use std::env;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::layout::{LayoutError, Node, coerce_string, default_layout, parse_layout};
use crate::multiplexer::MultiplexerKind;

/// The home directory of a passwd entry produced by `lookup`.
///
/// getpwnam/getpwuid share a static buffer; ctx only calls them from these
/// rare fallbacks, never concurrently with themselves in practice.
fn passwd_home(lookup: impl FnOnce() -> *mut libc::passwd) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    let entry = lookup();
    if entry.is_null() {
        return None;
    }
    let dir = unsafe { (*entry).pw_dir };
    if dir.is_null() {
        return None;
    }
    let bytes = unsafe { std::ffi::CStr::from_ptr(dir) }.to_bytes();
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

fn user_home(user: &str) -> Option<PathBuf> {
    let name = std::ffi::CString::new(user).ok()?;
    passwd_home(|| unsafe { libc::getpwnam(name.as_ptr()) })
}

/// $HOME, falling back to the pwd database like Python's Path.home().
pub(crate) fn home() -> PathBuf {
    if let Some(home) = env::var_os("HOME").filter(|home| !home.is_empty()) {
        return PathBuf::from(home);
    }
    passwd_home(|| unsafe { libc::getpwuid(libc::getuid()) }).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    match env::var(variable) {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => home().join(fallback),
    }
}

pub fn config_path() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config")
        .join("ctx")
        .join("config.toml")
}

fn data_dir() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share").join("ctx")
}

fn expand_user(path: &str) -> PathBuf {
    if path == "~" {
        return home();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home().join(rest);
    }
    if let Some(rest) = path.strip_prefix('~') {
        // ~user forms resolve via the pwd database, like Python's
        // expanduser; an unknown user stays verbatim, also like it.
        let (user, sub) = match rest.split_once('/') {
            Some((user, sub)) => (user, Some(sub)),
            None => (rest, None),
        };
        if let Some(dir) = user_home(user) {
            return match sub {
                Some(sub) => dir.join(sub),
                None => dir,
            };
        }
    }
    PathBuf::from(path)
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Layout(#[from] LayoutError),
}

fn config_err<T>(message: impl Into<String>) -> Result<T, ConfigError> {
    Err(ConfigError::Config(message.into()))
}

pub const BUILTIN_STATUS: &[&str] = &["agent", "github"];

/// A named status column in listings, filled by a command or a built-in.
///
/// `interval` is the column's sampling period in seconds; None picks the
/// provider's default.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusColumn {
    pub name: String,
    pub command: Option<String>,
    pub builtin: Option<String>,
    pub interval: Option<f64>,
}

/// TUI colours; the defaults stick to the terminal's ANSI palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub foreground: String,
    pub selection: String,
    pub border_active: String,
    pub border_inactive: String,
}

impl Default for Theme {
    fn default() -> Theme {
        Theme {
            foreground: "ansi_default".to_string(),
            selection: "ansi_blue".to_string(),
            border_active: "ansi_blue".to_string(),
            border_inactive: "ansi_default".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub contexts_dir: PathBuf,
    pub repos_dir: PathBuf,
    pub archive_dir: PathBuf,
    pub branch_prefix: String,
    pub multiplexer: MultiplexerKind,
    pub nerd_font: bool,
    pub layout: Node,
    pub status: Vec<StatusColumn>,
    pub theme: Theme,
}

impl Default for Config {
    fn default() -> Config {
        let data = data_dir();
        Config {
            contexts_dir: data.join("contexts"),
            repos_dir: data.join("repos"),
            archive_dir: data.join("archive"),
            branch_prefix: String::new(),
            multiplexer: MultiplexerKind::Tmux,
            nerd_font: true,
            layout: default_layout(),
            status: Vec::new(),
            theme: Theme::default(),
        }
    }
}

pub fn load_config(path: &Path) -> Result<Config, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // Only absence means defaults; an existing config that cannot be
        // read must not be silently ignored.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(err) => return config_err(format!("cannot read {}: {err}", path.display())),
    };
    let data: Table =
        toml::from_str(&text).map_err(|exc| ConfigError::Config(exc.message().to_string()))?;
    let mut cfg = Config::default();
    if let Some(value) = data.get("contexts_dir") {
        cfg.contexts_dir = expand_user(&coerce_string(value));
    }
    if let Some(value) = data.get("repos_dir") {
        cfg.repos_dir = expand_user(&coerce_string(value));
    }
    if let Some(value) = data.get("archive_dir") {
        cfg.archive_dir = expand_user(&coerce_string(value));
    }
    if let Some(value) = data.get("branch_prefix") {
        cfg.branch_prefix = coerce_string(value);
    }
    if let Some(value) = data.get("multiplexer") {
        let raw = coerce_string(value);
        match MultiplexerKind::parse(&raw) {
            Some(kind) => cfg.multiplexer = kind,
            None => {
                return config_err(format!(
                    "unknown multiplexer '{raw}' (supported: {})",
                    MultiplexerKind::names()
                ));
            }
        }
    }
    if let Some(value) = data.get("nerd_font") {
        match value {
            Value::Boolean(flag) => cfg.nerd_font = *flag,
            _ => return config_err("nerd_font must be a boolean"),
        }
    }
    if let Some(value) = data.get("layout") {
        match value {
            Value::Table(table) => cfg.layout = parse_layout(table)?,
            _ => return config_err("layout must be a table"),
        }
    }
    if let Some(value) = data.get("status") {
        cfg.status = parse_status(value)?;
    }
    if let Some(value) = data.get("theme") {
        cfg.theme = parse_theme(value)?;
    }
    Ok(cfg)
}

// Hex only: the TUI toolkit's colour names are an implementation detail.
fn is_hex_colour(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit())
}

fn parse_theme(data: &Value) -> Result<Theme, ConfigError> {
    let Value::Table(data) = data else {
        return config_err("theme must be a table");
    };
    const KNOWN: &[&str] = &[
        "foreground",
        "selection",
        "border_active",
        "border_inactive",
    ];
    let mut unknown: Vec<&str> = data
        .keys()
        .map(String::as_str)
        .filter(|key| !KNOWN.contains(key))
        .collect();
    if !unknown.is_empty() {
        unknown.sort_unstable();
        return config_err(format!("unknown theme key(s): {}", unknown.join(", ")));
    }
    let mut theme = Theme::default();
    for (key, value) in data {
        match value {
            Value::String(colour) if is_hex_colour(colour) => {
                let field = match key.as_str() {
                    "foreground" => &mut theme.foreground,
                    "selection" => &mut theme.selection,
                    "border_active" => &mut theme.border_active,
                    "border_inactive" => &mut theme.border_inactive,
                    _ => unreachable!("unknown keys were rejected above"),
                };
                *field = colour.clone();
            }
            _ => {
                return config_err(format!("theme {key} must be a hex colour like '#2d3f76'"));
            }
        }
    }
    Ok(theme)
}

fn parse_status(data: &Value) -> Result<Vec<StatusColumn>, ConfigError> {
    let Value::Array(data) = data else {
        return config_err("status must be an array of tables ([[status]])");
    };
    let mut columns = Vec::new();
    for entry in data {
        let entry = match entry {
            Value::Table(table) if table.contains_key("name") => table,
            _ => return config_err("each [[status]] needs a name"),
        };
        if entry.contains_key("command") == entry.contains_key("builtin") {
            return config_err("each [[status]] needs either a command or a builtin");
        }
        let builtin = entry.get("builtin").map(coerce_string);
        if let Some(builtin) = &builtin
            && !BUILTIN_STATUS.contains(&builtin.as_str())
        {
            return config_err(format!(
                "unknown status builtin '{builtin}' (supported: {})",
                BUILTIN_STATUS.join(", ")
            ));
        }
        let interval = match entry.get("interval") {
            None => None,
            Some(Value::Integer(seconds)) if *seconds >= 0 => Some(*seconds as f64),
            Some(Value::Float(seconds)) if *seconds >= 0.0 => Some(*seconds),
            Some(_) => {
                return config_err("status interval must be a non-negative number of seconds");
            }
        };
        columns.push(StatusColumn {
            name: coerce_string(&entry["name"]),
            command: entry.get("command").map(coerce_string),
            builtin,
            interval,
        });
    }
    let mut names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    if names.len() != columns.len() {
        return config_err("status names must be unique");
    }
    Ok(columns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Pane;

    fn write_config(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn load(text: &str) -> Result<Config, ConfigError> {
        let dir = tempfile::tempdir().unwrap();
        load_config(&write_config(dir.path(), text))
    }

    fn err(text: &str) -> String {
        load(text).expect_err("config must be rejected").to_string()
    }

    fn column(
        name: &str,
        command: Option<&str>,
        builtin: Option<&str>,
        interval: Option<f64>,
    ) -> StatusColumn {
        StatusColumn {
            name: name.to_string(),
            command: command.map(str::to_string),
            builtin: builtin.map(str::to_string),
            interval,
        }
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();

        let cfg = load_config(&dir.path().join("missing.toml")).unwrap();

        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn contexts_dir_override() {
        let cfg = load("contexts_dir = \"/data/contexts\"").unwrap();

        assert_eq!(cfg.contexts_dir, PathBuf::from("/data/contexts"));
    }

    #[test]
    fn repos_dir_override_expands_user() {
        let cfg = load("repos_dir = \"~/repos\"").unwrap();

        assert_eq!(cfg.repos_dir, home().join("repos"));
    }

    #[test]
    fn archive_dir_override_expands_user() {
        let cfg = load("archive_dir = \"~/archive\"").unwrap();

        assert_eq!(cfg.archive_dir, home().join("archive"));
    }

    #[test]
    fn tilde_user_paths_resolve_via_the_pwd_database() {
        let root_home = user_home("root").expect("root exists in passwd");

        assert_eq!(expand_user("~root"), root_home);
        assert_eq!(expand_user("~root/repos"), root_home.join("repos"));
        // Unknown users stay verbatim, like Python's expanduser.
        assert_eq!(
            expand_user("~no-such-user-xyz/repos"),
            PathBuf::from("~no-such-user-xyz/repos")
        );
    }

    #[test]
    fn unreadable_config_errors_instead_of_defaulting() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), "branch_prefix = \"mb/\"");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = load_config(&path);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            result
                .expect_err("must not default")
                .to_string()
                .contains("cannot read")
        );
    }

    #[test]
    fn branch_prefix_override() {
        assert_eq!(
            load("branch_prefix = \"mb/\"").unwrap().branch_prefix,
            "mb/"
        );
    }

    #[test]
    fn status_columns_override() {
        let cfg = load(
            "[[status]]\nname = \"claude\"\nbuiltin = \"agent\"\n\n\
             [[status]]\nname = \"ci\"\ncommand = \"my-ci-status\"",
        )
        .unwrap();

        assert_eq!(
            cfg.status,
            vec![
                column("claude", None, Some("agent"), None),
                column("ci", Some("my-ci-status"), None, None),
            ]
        );
    }

    #[test]
    fn no_status_columns_by_default() {
        let dir = tempfile::tempdir().unwrap();

        let cfg = load_config(&dir.path().join("missing.toml")).unwrap();

        assert_eq!(cfg.status, vec![]);
    }

    #[test]
    fn status_requires_a_name() {
        assert!(err("[[status]]\ncommand = \"true\"").contains("needs a name"));
    }

    #[test]
    fn status_requires_a_command_or_a_builtin() {
        assert!(err("[[status]]\nname = \"ci\"").contains("either a command or a builtin"));
    }

    #[test]
    fn status_rejects_a_command_combined_with_a_builtin() {
        let text = "[[status]]\nname = \"ci\"\ncommand = \"true\"\nbuiltin = \"agent\"";

        assert!(err(text).contains("either a command or a builtin"));
    }

    #[test]
    fn status_interval_override() {
        let cfg = load("[[status]]\nname = \"ci\"\nbuiltin = \"github\"\ninterval = 60").unwrap();

        assert_eq!(
            cfg.status,
            vec![column("ci", None, Some("github"), Some(60.0))]
        );
    }

    #[test]
    fn status_rejects_negative_intervals() {
        let text = "[[status]]\nname = \"ci\"\ncommand = \"true\"\ninterval = -1";

        assert!(err(text).contains("non-negative number"));
    }

    #[test]
    fn status_rejects_non_numeric_intervals() {
        let text = "[[status]]\nname = \"ci\"\ncommand = \"true\"\ninterval = \"60\"";

        assert!(err(text).contains("non-negative number"));
    }

    #[test]
    fn status_rejects_unknown_builtins() {
        assert!(
            err("[[status]]\nname = \"ci\"\nbuiltin = \"gitlab\"")
                .contains("unknown status builtin 'gitlab'")
        );
    }

    #[test]
    fn status_rejects_duplicate_names() {
        let text = "[[status]]\nname = \"ci\"\ncommand = \"true\"\n\n\
                    [[status]]\nname = \"ci\"\nbuiltin = \"github\"";

        assert!(err(text).contains("unique"));
    }

    #[test]
    fn theme_defaults_to_the_ansi_palette() {
        let dir = tempfile::tempdir().unwrap();

        let cfg = load_config(&dir.path().join("missing.toml")).unwrap();

        assert_eq!(cfg.theme, Theme::default());
        assert_eq!(cfg.theme.selection, "ansi_blue");
    }

    #[test]
    fn theme_override() {
        let cfg = load("[theme]\nselection = \"#2d3f76\"\nborder_active = \"#ff966c\"").unwrap();

        assert_eq!(cfg.theme.selection, "#2d3f76");
        assert_eq!(cfg.theme.border_active, "#ff966c");
        assert_eq!(cfg.theme.foreground, "ansi_default");
    }

    #[test]
    fn theme_rejects_unknown_keys() {
        assert!(err("[theme]\nselektion = \"#2d3f76\"").contains("unknown theme key"));
    }

    #[test]
    fn theme_rejects_malformed_colours() {
        assert!(err("[theme]\nselection = \"#12\"").contains("colour"));
    }

    #[test]
    fn theme_rejects_colour_names() {
        // Toolkit colour names are an implementation detail, not config surface.
        assert!(err("[theme]\nselection = \"ansi_blue\"").contains("hex colour"));
    }

    #[test]
    fn multiplexer_override() {
        let cfg = load("multiplexer = \"zellij\"").unwrap();

        assert_eq!(cfg.multiplexer, MultiplexerKind::Zellij);
    }

    #[test]
    fn layout_override() {
        let cfg = load("layout = { command = \"nvim\" }").unwrap();

        assert_eq!(
            cfg.layout,
            Node::Pane(Pane {
                command: Some("nvim".to_string()),
                ..Pane::default()
            })
        );
    }

    #[test]
    fn unknown_multiplexer_rejected() {
        assert!(err("multiplexer = \"screen\"").contains("unknown multiplexer"));
    }

    #[test]
    fn nerd_font_is_on_by_default_and_can_be_disabled() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            load_config(&dir.path().join("missing.toml"))
                .unwrap()
                .nerd_font
        );
        assert!(!load("nerd_font = false").unwrap().nerd_font);
    }

    #[test]
    fn nerd_font_rejects_non_booleans() {
        assert!(err("nerd_font = \"yes\"").contains("nerd_font"));
    }
}
