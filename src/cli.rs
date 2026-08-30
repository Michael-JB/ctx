use std::collections::HashMap;
use std::io::Write;

use clap::{Parser, Subcommand};

use crate::config::{Config, ConfigError, load_config};
use crate::contexts;
use crate::errors::{CtxError, Result, msg};
use crate::layout::accepted_keys;
use crate::multiplexer::{Multiplexer, get_multiplexer};
use crate::repos;
use crate::status;
use crate::{claude_hook, claude_trust};

/// Injectable dependencies shared by all commands.
pub struct Deps {
    pub cfg: Config,
    pub mux: Box<dyn Multiplexer>,
}

/// Both output streams, injectable so tests can capture them.
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

#[derive(Parser)]
#[command(
    name = "ctx",
    version,
    about = "Manage repo-scoped work contexts.",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a context: fresh checkout of REPO on a new local branch.
    ///
    /// NAME defaults to a random adjective-animal pair.
    New {
        repo: String,
        name: Option<String>,
        /// Base branch (default: the repo's default branch).
        #[arg(short = 'b', long = "branch")]
        base: Option<String>,
        /// Pass a value to the layout's builtin panes (e.g. prompt=... for claude).
        #[arg(short = 's', long = "set", value_name = "KEY=VALUE")]
        assignments: Vec<String>,
        /// Start the session without attaching to it.
        #[arg(short = 'd', long)]
        detach: bool,
    },
    /// Attach to a context's session, unarchiving it and recreating the session if needed.
    Open { name: String },
    /// List contexts with branch, dirtiness, and session state.
    List {
        /// List archived contexts instead.
        #[arg(long)]
        archived: bool,
    },
    /// Delete contexts, archived or not: kill their sessions and remove the checkouts.
    Rm {
        #[arg(required = true)]
        names: Vec<String>,
        /// Delete even with uncommitted or unpushed work.
        #[arg(long)]
        force: bool,
    },
    /// Archive contexts: kill their sessions and move the checkouts aside.
    Archive {
        names: Vec<String>,
        /// Permanently delete all archived contexts.
        #[arg(long)]
        empty: bool,
    },
    /// Restore an archived context.
    Unarchive { name: String },
    /// Manage contexts and repos interactively.
    Tui {
        /// Exit the TUI after opening a context.
        #[arg(long = "exit")]
        exit_on_open: bool,
    },
    /// Print ctx usage docs for coding agents, ready to install as a skill.
    AgentDocs,
    /// Print the installed version's changelog.
    Changelog,
    /// Entry points backing the builtins.
    #[command(subcommand, hide = true)]
    Builtin(BuiltinCommands),
    /// Manage registered repositories.
    #[command(subcommand)]
    Repo(RepoCommands),
}

#[derive(Subcommand)]
enum BuiltinCommands {
    /// Adapters for Claude Code.
    #[command(subcommand)]
    Claude(ClaudeCommands),
}

#[derive(Subcommand)]
enum ClaudeCommands {
    /// Feed the agent status column from a Claude Code hook event on stdin.
    StatusHook,
    /// Mark the current directory trusted so Claude Code skips its trust dialog.
    Trust,
}

#[derive(Subcommand)]
enum RepoCommands {
    /// Register a repository by cloning a local bare mirror of it.
    Add {
        url: String,
        /// Registry name (default: derived from URL).
        #[arg(long)]
        name: Option<String>,
    },
    /// List registered repositories.
    List,
    /// Show or set the repo new contexts are created in by default.
    Default {
        name: Option<String>,
        /// Clear the default repo.
        #[arg(long)]
        clear: bool,
    },
    /// Remove registered repositories' mirrors (contexts are untouched).
    Rm {
        #[arg(required = true)]
        names: Vec<String>,
    },
}

fn parse_assignments(cfg: &Config, assignments: &[String]) -> Result<HashMap<String, String>> {
    let accepted = accepted_keys(&cfg.layout);
    let mut values = HashMap::new();
    for assignment in assignments {
        let Some((key, value)) = assignment.split_once('=') else {
            return msg(format!("--set needs KEY=VALUE, got '{assignment}'"));
        };
        if key.is_empty() {
            return msg(format!("--set needs KEY=VALUE, got '{assignment}'"));
        }
        if values.contains_key(key) {
            return msg(format!("--set gives '{key}' twice"));
        }
        if !accepted.contains(key) {
            return msg(format!("no builtin pane in the layout accepts '{key}'"));
        }
        values.insert(key.to_string(), value.to_string());
    }
    Ok(values)
}

fn create_and_open(
    deps: &Deps,
    io: &mut Io,
    repo: &str,
    name: Option<String>,
    base: Option<&str>,
    values: HashMap<String, String>,
    detach: bool,
) -> Result<()> {
    let name = match name {
        Some(name) => name,
        None => contexts::random_name(&deps.cfg)?,
    };
    let ctx = contexts::create_context(&deps.cfg, repo, &name, base)?;
    writeln!(
        io.out,
        "created {} at {} on {}",
        ctx.qualified(),
        ctx.path.display(),
        contexts::current_branch(&ctx)
    )?;
    if detach {
        deps.mux.create(&ctx, Some(&values))?;
    } else {
        deps.mux.open(&ctx, Some(&values))?;
    }
    Ok(())
}

fn cmd_open(deps: &Deps, io: &mut Io, name: &str) -> Result<()> {
    let mut ctx = contexts::find_any(&deps.cfg, name)?;
    if contexts::is_archived(&deps.cfg, &ctx) {
        ctx = contexts::unarchive_context(&deps.cfg, &ctx)?;
        writeln!(io.out, "unarchived {}", ctx.qualified())?;
    }
    deps.mux.open(&ctx, None)?;
    Ok(())
}

/// Every context's status cells, fetched concurrently.
fn all_status_cells(cfg: &Config, ctxs: &[contexts::Context]) -> Vec<Vec<String>> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = ctxs
            .iter()
            .map(|ctx| scope.spawn(move || status::status_cells(cfg, ctx)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("status fetch must not panic"))
            .collect()
    })
}

fn cmd_list(deps: &Deps, io: &mut Io, archived: bool) -> Result<()> {
    let all_contexts = if archived {
        contexts::list_archived(&deps.cfg)
    } else {
        contexts::list_contexts(&deps.cfg)
    };
    if all_contexts.is_empty() {
        writeln!(
            io.out,
            "{}",
            if archived {
                "no archived contexts"
            } else {
                "no contexts"
            }
        )?;
        return Ok(());
    }
    let mut header = vec![
        "NAME".to_string(),
        "REPO".to_string(),
        "BRANCH".to_string(),
        "STATUS".to_string(),
    ];
    header.extend(deps.cfg.status.iter().map(|s| s.name.to_uppercase()));
    let mut rows = vec![header];
    for (ctx, cells) in all_contexts
        .iter()
        .zip(all_status_cells(&deps.cfg, &all_contexts))
    {
        let mut row = vec![
            ctx.name.clone(),
            ctx.repo.clone(),
            contexts::current_branch(ctx),
        ];
        row.extend(cells);
        rows.push(row);
    }
    let columns = rows[0].len();
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in rows {
        let line = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| {
                let pad = width.saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(pad))
            })
            .collect::<Vec<_>>()
            .join("  ");
        writeln!(io.out, "{}", line.trim_end())?;
    }
    Ok(())
}

fn cmd_rm(deps: &Deps, io: &mut Io, names: &[String], force: bool) -> Result<i32> {
    let mut failed = false;
    for name in names {
        if let Err(error) = remove_one(deps, io, name, force) {
            writeln!(io.err, "error: {error}")?;
            failed = true;
        }
    }
    Ok(if failed { 1 } else { 0 })
}

/// Delete one context, reporting problems as errors instead of proceeding.
fn remove_one(deps: &Deps, io: &mut Io, name: &str, force: bool) -> Result<()> {
    let ctx = contexts::find_any(&deps.cfg, name)?;
    if !force {
        let mut problems = Vec::new();
        if contexts::is_dirty(&ctx)? {
            problems.push("uncommitted changes".to_string());
        }
        let unpushed = contexts::unpushed_commits(&ctx)?;
        if !unpushed.is_empty() {
            problems.push(format!("{} unpushed commit(s)", unpushed.len()));
        }
        if !problems.is_empty() {
            return msg(format!(
                "{} has {}; use --force to delete anyway",
                ctx.qualified(),
                problems.join(" and ")
            ));
        }
    }
    // Kill last: killing our own session takes this process down with it,
    // so nothing after the kill is guaranteed to run. Kill even when the
    // removal fails half-way; the startup sweep finishes the removal.
    let removed = contexts::remove_context(&ctx);
    if removed.is_ok() {
        writeln!(io.out, "removed {}", ctx.qualified())?;
    }
    if deps.mux.exists(&ctx) {
        deps.mux.kill(&ctx)?;
    }
    removed
}

fn cmd_archive(deps: &Deps, io: &mut Io, names: &[String], empty: bool) -> Result<i32> {
    if empty {
        if !names.is_empty() {
            writeln!(io.err, "error: --empty takes no context names")?;
            return Ok(2);
        }
        let count = contexts::list_archived(&deps.cfg).len();
        contexts::empty_archive(&deps.cfg)?;
        writeln!(io.out, "emptied archive ({count} context(s))")?;
        return Ok(0);
    }
    if names.is_empty() {
        writeln!(io.err, "error: provide context names or --empty")?;
        return Ok(2);
    }
    let mut failed = false;
    for name in names {
        if let Err(error) = archive_one(deps, io, name) {
            writeln!(io.err, "error: {error}")?;
            failed = true;
        }
    }
    Ok(if failed { 1 } else { 0 })
}

/// Archive one context, reporting problems as errors instead of proceeding.
fn archive_one(deps: &Deps, io: &mut Io, name: &str) -> Result<()> {
    let ctx = contexts::find_context(&deps.cfg, name)?;
    contexts::archive_context(&deps.cfg, &ctx)?;
    if deps.mux.exists(&ctx) {
        deps.mux.kill(&ctx)?;
    }
    writeln!(io.out, "archived {}", ctx.qualified())?;
    Ok(())
}

fn cmd_unarchive(deps: &Deps, io: &mut Io, name: &str) -> Result<()> {
    let archived = contexts::find_archived(&deps.cfg, name)?;
    let ctx = contexts::unarchive_context(&deps.cfg, &archived)?;
    writeln!(io.out, "unarchived {}", ctx.qualified())?;
    Ok(())
}

fn cmd_tui(_deps: &Deps, _io: &mut Io, _exit_on_open: bool) -> Result<()> {
    // The ratatui port lands as its own step; everything else works already.
    msg("the interactive TUI is not ported yet")
}

fn cmd_repo(deps: &Deps, io: &mut Io, command: &RepoCommands) -> Result<i32> {
    match command {
        RepoCommands::Add { url, name } => {
            writeln!(io.out, "cloning {url}")?;
            let registered = repos::add_repo(&deps.cfg, url, name.as_deref())?;
            writeln!(io.out, "registered '{registered}'")?;
        }
        RepoCommands::List => {
            for name in repos::repo_names(&deps.cfg) {
                writeln!(io.out, "{name}\t{}", repos::repo_url(&deps.cfg, &name)?)?;
            }
        }
        RepoCommands::Default { name, clear } => {
            if *clear {
                if name.is_some() {
                    writeln!(io.err, "error: --clear takes no repo name")?;
                    return Ok(2);
                }
                repos::set_default_repo(&deps.cfg, None)?;
                writeln!(io.out, "cleared default repo")?;
                return Ok(0);
            }
            match name {
                None => {
                    let current = repos::default_repo(&deps.cfg);
                    writeln!(
                        io.out,
                        "{}",
                        current.as_deref().unwrap_or("no default repo")
                    )?;
                }
                Some(name) => {
                    repos::set_default_repo(&deps.cfg, Some(name))?;
                    writeln!(io.out, "default repo is '{name}'")?;
                }
            }
        }
        RepoCommands::Rm { names } => {
            let mut failed = false;
            for name in names {
                match repos::remove_repo(&deps.cfg, name) {
                    Ok(()) => writeln!(io.out, "removed '{name}'")?,
                    Err(error) => {
                        writeln!(io.err, "error: {error}")?;
                        failed = true;
                    }
                }
            }
            if failed {
                return Ok(1);
            }
        }
    }
    Ok(0)
}

fn dispatch(cli: Cli, deps: &Deps, io: &mut Io) -> Result<i32> {
    let command = match cli.command {
        None => Commands::Tui {
            exit_on_open: false,
        },
        Some(command) => command,
    };
    match command {
        Commands::New {
            repo,
            name,
            base,
            assignments,
            detach,
        } => {
            let values = parse_assignments(&deps.cfg, &assignments)?;
            create_and_open(deps, io, &repo, name, base.as_deref(), values, detach)?;
        }
        Commands::Open { name } => cmd_open(deps, io, &name)?,
        Commands::List { archived } => cmd_list(deps, io, archived)?,
        Commands::Rm { names, force } => return cmd_rm(deps, io, &names, force),
        Commands::Archive { names, empty } => return cmd_archive(deps, io, &names, empty),
        Commands::Unarchive { name } => cmd_unarchive(deps, io, &name)?,
        Commands::Tui { exit_on_open } => cmd_tui(deps, io, exit_on_open)?,
        Commands::AgentDocs => {
            write!(io.out, "{}", include_str!("ctx/agent_docs.md"))?;
        }
        Commands::Changelog => {
            write!(io.out, "{}", include_str!("ctx/CHANGELOG.md"))?;
        }
        Commands::Builtin(BuiltinCommands::Claude(command)) => {
            let cwd = std::env::current_dir()?;
            match command {
                ClaudeCommands::StatusHook => {
                    let mut raw = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw)?;
                    claude_hook::handle(&raw, &cwd);
                }
                ClaudeCommands::Trust => claude_trust::trust(&cwd),
            }
        }
        Commands::Repo(command) => return cmd_repo(deps, io, &command),
    }
    Ok(0)
}

fn report(err: &CtxError, io: &mut Io) -> i32 {
    match err {
        CtxError::Git(git) => {
            let detail = git.stderr.as_deref().unwrap_or("").trim();
            let message = format!("error: command failed ({})", git.argv.join(" "));
            let _ = if detail.is_empty() {
                writeln!(io.err, "{message}")
            } else {
                writeln!(io.err, "{message}\n{detail}")
            };
            git.code.unwrap_or(1)
        }
        other => {
            let _ = writeln!(io.err, "error: {other}");
            1
        }
    }
}

pub fn main() -> i32 {
    let cli = Cli::parse();
    let (out, err) = (std::io::stdout(), std::io::stderr());
    let mut io = Io {
        out: &mut out.lock(),
        err: &mut err.lock(),
    };
    let cfg = match load_config(&crate::config::config_path()) {
        Ok(cfg) => cfg,
        Err(error) => {
            let kind = match error {
                ConfigError::Config(_) => "config",
                ConfigError::Layout(_) => "layout",
            };
            let _ = writeln!(io.err, "error: invalid {kind}: {error}");
            return 1;
        }
    };
    let mux = get_multiplexer(cfg.multiplexer, cfg.layout.clone());
    let deps = Deps { cfg, mux };
    match dispatch(cli, &deps, &mut io) {
        Ok(code) => code,
        Err(error) => report(&error, &mut io),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::contexts::Context;
    use crate::multiplexer::MultiplexerError;
    use crate::testutil::{TestEnv, commit_file, test_env};

    /// Spy double: canned exists() answers plus a record of open/kill calls.
    #[derive(Default)]
    struct SpyState {
        running: Vec<String>,
        current: Option<String>,
        opened: Vec<String>,
        created: Vec<String>,
        killed: Vec<String>,
        path_present_at_kill: Option<bool>,
        values: Vec<Option<HashMap<String, String>>>,
    }

    #[derive(Clone, Default)]
    struct SpyMultiplexer(Arc<Mutex<SpyState>>);

    impl SpyMultiplexer {
        fn state(&self) -> std::sync::MutexGuard<'_, SpyState> {
            self.0.lock().unwrap()
        }
    }

    impl Multiplexer for SpyMultiplexer {
        fn can_open_in_place(&self) -> bool {
            true
        }

        fn exists(&self, ctx: &Context) -> bool {
            self.state().running.contains(&ctx.qualified())
        }

        fn is_current(&self, ctx: &Context) -> bool {
            self.state().current.as_deref() == Some(ctx.qualified().as_str())
        }

        fn create(
            &self,
            ctx: &Context,
            values: Option<&HashMap<String, String>>,
        ) -> std::result::Result<(), MultiplexerError> {
            let mut state = self.state();
            state.created.push(ctx.qualified());
            state.values.push(values.cloned());
            Ok(())
        }

        fn open(
            &self,
            ctx: &Context,
            values: Option<&HashMap<String, String>>,
        ) -> std::result::Result<(), MultiplexerError> {
            let mut state = self.state();
            state.opened.push(ctx.qualified());
            state.values.push(values.cloned());
            Ok(())
        }

        fn kill(&self, ctx: &Context) -> std::result::Result<(), MultiplexerError> {
            let mut state = self.state();
            state.path_present_at_kill = Some(ctx.path.exists());
            state.killed.push(ctx.qualified());
            Ok(())
        }
    }

    struct Run {
        code: i32,
        out: String,
        err: String,
    }

    fn invoke(args: &[&str], deps: &Deps) -> Run {
        let mut argv = vec!["ctx"];
        argv.extend(args);
        let cli = match Cli::try_parse_from(&argv) {
            Ok(cli) => cli,
            Err(parse_error) => {
                return Run {
                    code: parse_error.exit_code(),
                    out: String::new(),
                    err: parse_error.to_string(),
                };
            }
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut io = Io {
            out: &mut out,
            err: &mut err,
        };
        let code = match dispatch(cli, deps, &mut io) {
            Ok(code) => code,
            Err(error) => report(&error, &mut io),
        };
        Run {
            code,
            out: String::from_utf8(out).unwrap(),
            err: String::from_utf8(err).unwrap(),
        }
    }

    fn deps_for(env: &TestEnv) -> (Deps, SpyMultiplexer) {
        let mux = SpyMultiplexer::default();
        (
            Deps {
                cfg: env.cfg.clone(),
                mux: Box::new(mux.clone()),
            },
            mux,
        )
    }

    fn registered() -> (TestEnv, Deps, SpyMultiplexer) {
        let env = test_env();
        let origin = env.origin();
        repos::add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();
        let (deps, mux) = deps_for(&env);
        (env, deps, mux)
    }

    fn create(deps: &Deps, name: &str) -> Context {
        contexts::create_context(&deps.cfg, "origin", name, None).unwrap()
    }

    #[test]
    fn help() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["--help"], &deps);

        assert_eq!(run.code, 0);
    }

    #[test]
    fn version() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["--version"], &deps);

        assert_eq!(run.code, 0);
    }

    #[test]
    fn agent_docs_prints_the_spin_off_flow() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["agent-docs"], &deps);

        assert_eq!(run.code, 0);
        assert!(run.out.contains("ctx new <repo> <name> --detach"));
        assert!(run.out.contains("--set prompt=\""));
    }

    #[test]
    fn changelog_prints_release_sections() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["changelog"], &deps);

        assert_eq!(run.code, 0);
        assert!(run.out.starts_with("# Changelog"));
        assert!(run.out.contains("## [0"));
    }

    #[test]
    fn new_reports_the_created_context() {
        let (_env, deps, _mux) = registered();

        let run = invoke(&["new", "origin", "feat"], &deps);

        assert_eq!(run.code, 0);
        assert!(run.out.contains("created origin/feat"));
    }

    #[test]
    fn new_without_a_name_generates_one() {
        let (_env, deps, _mux) = registered();

        let run = invoke(&["new", "origin"], &deps);

        assert_eq!(run.code, 0);
        assert!(run.out.contains("created origin/"));
    }

    #[test]
    fn new_opens_a_session() {
        let (_env, deps, mux) = registered();

        invoke(&["new", "origin", "feat"], &deps);

        assert_eq!(mux.state().opened, ["origin/feat"]);
    }

    #[test]
    fn new_rejects_an_unregistered_repo() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["new", "nope", "feat"], &deps);

        assert_eq!(run.code, 1);
        assert!(run.err.contains("not registered"));
    }

    #[test]
    fn new_detach_creates_the_session_without_opening() {
        let (_env, deps, mux) = registered();

        let run = invoke(&["new", "origin", "feat", "--detach"], &deps);

        assert_eq!(run.code, 0);
        let state = mux.state();
        assert_eq!(state.created, ["origin/feat"]);
        assert!(state.opened.is_empty());
        assert_eq!(state.values, [Some(HashMap::new())]);
    }

    #[test]
    fn new_marks_the_session_as_fresh() {
        let (_env, deps, mux) = registered();

        invoke(&["new", "origin", "feat"], &deps);

        assert_eq!(mux.state().values, [Some(HashMap::new())]);
    }

    #[test]
    fn open_marks_the_session_as_recreated() {
        let (_env, deps, mux) = registered();
        create(&deps, "feat");

        invoke(&["open", "feat"], &deps);

        assert_eq!(mux.state().values, [None]);
    }

    fn with_claude_layout(deps: Deps) -> Deps {
        let mut cfg = deps.cfg;
        cfg.layout = crate::layout::Node::Pane(crate::layout::Pane {
            builtin: Some("claude".to_string()),
            ..crate::layout::Pane::default()
        });
        Deps { cfg, mux: deps.mux }
    }

    #[test]
    fn new_set_passes_values_to_the_session() {
        let (_env, deps, mux) = registered();
        let deps = with_claude_layout(deps);

        let run = invoke(
            &["new", "origin", "feat", "--set", "prompt=explore x"],
            &deps,
        );

        assert_eq!(run.code, 0);
        assert_eq!(
            mux.state().values,
            [Some(HashMap::from([(
                "prompt".to_string(),
                "explore x".to_string()
            )]))]
        );
    }

    #[test]
    fn new_set_rejects_a_key_no_builtin_accepts() {
        let (_env, deps, _mux) = registered();

        let run = invoke(&["new", "origin", "feat", "--set", "prompt=x"], &deps);

        assert_eq!(run.code, 1);
        assert!(
            run.err
                .contains("no builtin pane in the layout accepts 'prompt'")
        );
    }

    #[test]
    fn new_set_rejects_a_malformed_assignment() {
        let (_env, deps, _mux) = registered();

        let run = invoke(&["new", "origin", "feat", "--set", "prompt"], &deps);

        assert_eq!(run.code, 1);
        assert!(run.err.contains("--set needs KEY=VALUE"));
    }

    #[test]
    fn new_set_rejects_a_repeated_key() {
        let (_env, deps, _mux) = registered();
        let deps = with_claude_layout(deps);

        let run = invoke(
            &[
                "new", "origin", "feat", "--set", "prompt=a", "--set", "prompt=b",
            ],
            &deps,
        );

        assert_eq!(run.code, 1);
        assert!(run.err.contains("'prompt' twice"));
    }

    #[test]
    fn new_rejects_an_invalid_name() {
        let (_env, deps, _mux) = registered();

        let run = invoke(&["new", "origin", "feat~1"], &deps);

        assert_eq!(run.code, 1);
        assert!(run.err.contains("valid branch name"));
    }

    #[test]
    fn open_opens_the_context_session() {
        let (_env, deps, mux) = registered();
        create(&deps, "feat");

        let run = invoke(&["open", "feat"], &deps);

        assert_eq!(run.code, 0);
        assert_eq!(mux.state().opened, ["origin/feat"]);
    }

    #[test]
    fn open_unarchives_an_archived_context() {
        let (_env, deps, mux) = registered();
        contexts::archive_context(&deps.cfg, &create(&deps, "feat")).unwrap();

        let run = invoke(&["open", "feat"], &deps);

        assert_eq!(run.code, 0);
        assert!(run.out.contains("unarchived origin/feat"));
        assert_eq!(mux.state().opened, ["origin/feat"]);
        assert!(contexts::list_archived(&deps.cfg).is_empty());
        assert!(
            contexts::find_context(&deps.cfg, "feat")
                .unwrap()
                .path
                .exists()
        );
    }

    #[test]
    fn open_rejects_an_unknown_context() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["open", "feat"], &deps);

        assert_eq!(run.code, 1);
        assert!(run.err.contains("no context 'feat'"));
    }

    #[test]
    fn list_without_contexts() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["list"], &deps);

        assert_eq!(run.out, "no contexts\n");
    }

    #[test]
    fn list_shows_each_context() {
        let (_env, deps, _mux) = registered();
        create(&deps, "feat");

        let run = invoke(&["list"], &deps);

        let lines: Vec<&str> = run.out.lines().collect();
        let header: Vec<&str> = lines[0].split_whitespace().collect();
        let row: Vec<&str> = lines[1].split_whitespace().collect();
        assert_eq!(header, ["NAME", "REPO", "BRANCH", "STATUS"]);
        assert_eq!(row, ["feat", "origin", "feat"]);
    }

    #[test]
    fn list_adds_a_column_per_status_column() {
        let (_env, mut deps, _mux) = registered();
        deps.cfg.status = vec![crate::config::StatusColumn {
            name: "claude".to_string(),
            command: Some("echo working".to_string()),
            builtin: None,
            interval: None,
        }];
        create(&deps, "feat");

        let run = invoke(&["list"], &deps);

        let lines: Vec<&str> = run.out.lines().collect();
        let header: Vec<&str> = lines[0].split_whitespace().collect();
        let row: Vec<&str> = lines[1].split_whitespace().collect();
        assert_eq!(header, ["NAME", "REPO", "BRANCH", "STATUS", "CLAUDE"]);
        assert_eq!(row, ["feat", "origin", "feat", "working"]);
    }

    #[test]
    fn list_marks_dirty_contexts() {
        let (_env, deps, _mux) = registered();
        let ctx = create(&deps, "feat");
        std::fs::write(ctx.path.join("scratch.txt"), "x\n").unwrap();

        let run = invoke(&["list"], &deps);

        assert!(run.out.contains('*'));
    }

    #[test]
    fn rm_deletes_the_checkout() {
        let (_env, deps, _mux) = registered();
        let ctx = create(&deps, "feat");

        let run = invoke(&["rm", "feat"], &deps);

        assert_eq!(run.code, 0);
        assert!(run.out.contains("removed origin/feat"));
        assert!(!ctx.path.exists());
    }

    #[test]
    fn rm_kills_a_running_session() {
        let (_env, deps, mux) = registered();
        create(&deps, "feat");
        mux.state().running.push("origin/feat".to_string());

        invoke(&["rm", "feat"], &deps);

        assert_eq!(mux.state().killed, ["origin/feat"]);
    }

    #[test]
    fn rm_of_the_current_context_removes_before_the_kill() {
        // Killing our own session ends this process; the removal must land first.
        let (_env, deps, mux) = registered();
        let ctx = create(&deps, "feat");
        {
            let mut state = mux.state();
            state.running.push("origin/feat".to_string());
            state.current = Some("origin/feat".to_string());
        }

        let run = invoke(&["rm", "feat"], &deps);

        assert_eq!(run.code, 0);
        assert!(!ctx.path.exists());
        let state = mux.state();
        assert_eq!(state.killed, ["origin/feat"]);
        assert_eq!(state.path_present_at_kill, Some(false));
    }

    #[test]
    fn rm_kills_the_session_even_when_removal_fails() {
        use std::os::unix::fs::PermissionsExt;

        let (_env, deps, mux) = registered();
        let ctx = create(&deps, "feat");
        mux.state().running.push("origin/feat".to_string());
        // A read-only directory inside the checkout makes the delete fail
        // half-way: its contents cannot be unlinked.
        let locked = ctx.path.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("pin"), "x\n").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

        let run = invoke(&["rm", "--force", "feat"], &deps);

        // Unlock the leftover so the temp dir can be cleaned up.
        let leftover = ctx.path.with_file_name("feat.deleting").join("locked");
        std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_ne!(run.code, 0);
        assert_eq!(mux.state().killed, ["origin/feat"]);
    }

    #[test]
    fn rm_refuses_unpushed_work() {
        let (_env, deps, _mux) = registered();
        let ctx = create(&deps, "feat");
        commit_file(&ctx.path, "work.txt", "x\n");

        let run = invoke(&["rm", "feat"], &deps);

        assert_eq!(run.code, 1);
        assert!(run.err.contains("unpushed commit"));
        assert!(ctx.path.exists());
    }

    #[test]
    fn rm_force_overrides_the_guard() {
        let (_env, deps, _mux) = registered();
        let ctx = create(&deps, "feat");
        commit_file(&ctx.path, "work.txt", "x\n");

        let run = invoke(&["rm", "--force", "feat"], &deps);

        assert_eq!(run.code, 0);
        assert!(!ctx.path.exists());
    }

    #[test]
    fn rm_archived_deletes_the_archived_checkout() {
        let (_env, deps, _mux) = registered();
        let archived = contexts::archive_context(&deps.cfg, &create(&deps, "feat")).unwrap();

        let run = invoke(&["rm", "feat"], &deps);

        assert_eq!(run.code, 0);
        assert!(!archived.path.exists());
    }

    #[test]
    fn rm_archived_kills_a_lingering_session() {
        let (_env, deps, mux) = registered();
        contexts::archive_context(&deps.cfg, &create(&deps, "feat")).unwrap();
        mux.state().running.push("origin/feat".to_string());

        let run = invoke(&["rm", "feat"], &deps);

        assert_eq!(run.code, 0);
        assert_eq!(mux.state().killed, ["origin/feat"]);
    }

    #[test]
    fn rm_archived_refuses_unpushed_work() {
        let (_env, deps, _mux) = registered();
        let ctx = create(&deps, "feat");
        commit_file(&ctx.path, "work.txt", "x\n");
        contexts::archive_context(&deps.cfg, &ctx).unwrap();

        let run = invoke(&["rm", "feat"], &deps);

        assert_eq!(run.code, 1);
        assert!(run.err.contains("unpushed commit"));
    }

    #[test]
    fn rm_rejects_an_unknown_context() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["rm", "feat"], &deps);

        assert_eq!(run.code, 1);
        assert!(run.err.contains("no context 'feat'"));
    }

    #[test]
    fn archive_moves_the_context_and_kills_its_session() {
        let (_env, deps, mux) = registered();
        let ctx = create(&deps, "feat");
        mux.state().running.push("origin/feat".to_string());

        let run = invoke(&["archive", "feat"], &deps);

        assert_eq!(run.code, 0);
        assert!(run.out.contains("archived origin/feat"));
        assert_eq!(mux.state().killed, ["origin/feat"]);
        assert!(!ctx.path.exists());
        assert!(
            contexts::find_archived(&deps.cfg, "feat")
                .unwrap()
                .path
                .exists()
        );
    }

    #[test]
    fn archive_rejects_an_unknown_context() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["archive", "feat"], &deps);

        assert_eq!(run.code, 1);
        assert!(run.err.contains("no context 'feat'"));
    }

    #[test]
    fn list_archived_without_archived_contexts() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["list", "--archived"], &deps);

        assert_eq!(run.out, "no archived contexts\n");
    }

    #[test]
    fn list_archived_shows_archived_contexts_only() {
        let (_env, deps, _mux) = registered();
        contexts::archive_context(&deps.cfg, &create(&deps, "cold")).unwrap();
        create(&deps, "hot");

        let run = invoke(&["list", "--archived"], &deps);

        let lines: Vec<&str> = run.out.lines().collect();
        let header: Vec<&str> = lines[0].split_whitespace().collect();
        let row: Vec<&str> = lines[1].split_whitespace().collect();
        assert_eq!(header, ["NAME", "REPO", "BRANCH", "STATUS"]);
        assert_eq!(&row[..2], ["cold", "origin"]);
    }

    #[test]
    fn archive_empty_deletes_all_archived_contexts() {
        let (_env, deps, _mux) = registered();
        contexts::archive_context(&deps.cfg, &create(&deps, "cold")).unwrap();
        let kept = create(&deps, "hot");

        let run = invoke(&["archive", "--empty"], &deps);

        assert_eq!(run.code, 0);
        assert!(run.out.contains("emptied archive (1 context(s))"));
        assert!(contexts::list_archived(&deps.cfg).is_empty());
        assert!(kept.path.exists());
    }

    #[test]
    fn archive_empty_rejects_names() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["archive", "--empty", "feat"], &deps);

        assert_eq!(run.code, 2);
    }

    #[test]
    fn unarchive_restores_the_context_without_opening() {
        let (_env, deps, mux) = registered();
        contexts::archive_context(&deps.cfg, &create(&deps, "feat")).unwrap();

        let run = invoke(&["unarchive", "feat"], &deps);

        assert_eq!(run.code, 0);
        assert!(run.out.contains("unarchived origin/feat"));
        assert!(mux.state().opened.is_empty());
        assert!(
            contexts::find_context(&deps.cfg, "feat")
                .unwrap()
                .path
                .exists()
        );
    }

    #[test]
    fn unarchive_rejects_an_unknown_context() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["unarchive", "feat"], &deps);

        assert_eq!(run.code, 1);
        assert!(run.err.contains("no archived context 'feat'"));
    }

    #[test]
    fn repo_add_registers() {
        let env = test_env();
        let origin = env.origin();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["repo", "add", &origin.to_string_lossy()], &deps);

        assert_eq!(run.code, 0);
        assert!(run.out.contains("registered 'origin'"));
    }

    #[test]
    fn repo_list_shows_name_and_url() {
        let env = test_env();
        let origin = env.origin();
        repos::add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["repo", "list"], &deps);

        assert_eq!(run.out, format!("origin\t{}\n", origin.display()));
    }

    #[test]
    fn repo_rm_unregisters() {
        let (_env, deps, _mux) = registered();

        let run = invoke(&["repo", "rm", "origin"], &deps);

        assert_eq!(run.code, 0);
        assert_eq!(invoke(&["repo", "list"], &deps).out, "");
    }

    #[test]
    fn repo_rm_rejects_unregistered() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["repo", "rm", "nope"], &deps);

        assert_eq!(run.code, 1);
        assert!(run.err.contains("not registered"));
    }

    #[test]
    fn repo_default_sets_and_shows() {
        let (_env, deps, _mux) = registered();

        let run = invoke(&["repo", "default", "origin"], &deps);

        assert_eq!(run.code, 0);
        assert_eq!(invoke(&["repo", "default"], &deps).out, "origin\n");
    }

    #[test]
    fn repo_default_clear() {
        let (_env, deps, _mux) = registered();
        invoke(&["repo", "default", "origin"], &deps);

        let run = invoke(&["repo", "default", "--clear"], &deps);

        assert_eq!(run.code, 0);
        assert_eq!(invoke(&["repo", "default"], &deps).out, "no default repo\n");
    }

    #[test]
    fn repo_default_rejects_unregistered() {
        let env = test_env();
        let (deps, _mux) = deps_for(&env);

        let run = invoke(&["repo", "default", "nope"], &deps);

        assert_eq!(run.code, 1);
        assert!(run.err.contains("not registered"));
    }
}
