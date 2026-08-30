//! Status column providers: user commands plus the built-ins.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use crate::config::{Config, StatusColumn};
use crate::contexts::Context;
use crate::errors::{Result, msg};
use crate::git::new_command;

const TIMEOUT: Duration = Duration::from_secs(2);

const AGENT_STALE_SECONDS: f64 = 3600.0;

const GITHUB_QUERY: &str = "
query($owner: String!, $repo: String!, $branch: String!) {
  repository(owner: $owner, name: $repo) {
    pullRequests(headRefName: $branch, first: 1, orderBy: {field: CREATED_AT, direction: DESC}) {
      nodes {
        state
        isDraft
        mergeable
        commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
      }
    }
  }
}
";
const GITHUB_JQ: &str = ".data.repository.pullRequests.nodes[0]\
 | if . == null then empty else\
 [.state, (.isDraft | tostring), .mergeable,\
 (.commits.nodes[0].commit.statusCheckRollup.state // \"NONE\")]\
 | join(\" \") end";

/// First line of a command's output, run in the checkout; None if it yields nothing.
///
/// Failures (non-zero exit, timeout, missing executable) also yield None: the
/// contract is "produce a status or stay quiet", so a broken or inapplicable
/// provider must not break listings.
fn run(mut cmd: Command, ctx: &Context) -> Option<String> {
    use wait_timeout::ChildExt;

    cmd.current_dir(&ctx.path)
        .env("CTX_REPO", &ctx.repo)
        .env("CTX_NAME", &ctx.name)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut child = cmd.spawn().ok()?;
    // Drain stdout on a thread so a chatty provider can't fill the pipe and
    // deadlock against the timed wait. The result comes over a channel: the
    // timeout must bound the read as well as the wait, because a spawned
    // grandchild can hold the pipe open long after the provider exits.
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let status = match child.wait_timeout(TIMEOUT).ok()? {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    let buf = rx.recv_timeout(remaining).ok()?;
    if !status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&buf);
    let first = text.trim().lines().next()?.trim().to_string();
    Some(first)
}

fn run_argv(argv: &[&str], ctx: &Context) -> Option<String> {
    let mut cmd = new_command(argv[0]);
    cmd.args(&argv[1..]);
    run(cmd, ctx)
}

fn run_shell(command: &str, ctx: &Context) -> Option<String> {
    let mut cmd = new_command("sh");
    cmd.args(["-c", command]);
    run(cmd, ctx)
}

/// The `command` provider: a user-configured shell command.
pub fn command_status(ctx: &Context, command: &str) -> Option<String> {
    run_shell(command, ctx)
}

/// The `agent` built-in: the checkout's agent-status file.
///
/// Agent harness hooks write a word (e.g. working/blocked/idle) to
/// `.git/agent-status`, rewriting it only when the state changes, so the
/// file's mtime is the state's start; active states show their age from it.
/// A file untouched for an hour is stale — the agent likely died without
/// its hooks firing — and reads as no status.
pub fn agent_status(ctx: &Context) -> Option<String> {
    let path = ctx.path.join(".git").join("agent-status");
    let mtime = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok()?;
    let age = SystemTime::now()
        .duration_since(mtime)
        .unwrap_or(Duration::ZERO)
        .as_secs_f64();
    if age > AGENT_STALE_SECONDS {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let word = text.trim().lines().next()?.trim().to_string();
    if word.is_empty() {
        return None;
    }
    if word == "working" || word == "monitoring" {
        return Some(format!("{word} {}", elapsed(age)));
    }
    Some(word)
}

/// Seconds only under the first minute: a table full of ticking
/// second-counters reads as nervous.
fn elapsed(seconds: f64) -> String {
    let whole = seconds as u64;
    if whole < 60 {
        format!("{whole}s")
    } else if whole < 3600 {
        format!("{}m", whole / 60)
    } else {
        format!("{}h{}m", whole / 3600, whole % 3600 / 60)
    }
}

/// The (owner, repo) of a git remote URL, tolerating ssh/https/scp forms.
pub fn github_repo(url: &str) -> Result<(String, String)> {
    let trimmed = url.strip_suffix(".git").unwrap_or(url).replace(':', "/");
    let mut parts = trimmed.rsplitn(3, '/');
    let repo = parts.next().unwrap_or_default();
    let owner = parts.next().unwrap_or_default();
    if parts.next().is_none() || owner.is_empty() || repo.is_empty() {
        return msg(format!("cannot parse owner/repo from remote URL '{url}'"));
    }
    Ok((owner.to_string(), repo.to_string()))
}

/// A GraphQL query via gh for the checkout's branch, lowercased.
///
/// No `gh`, a non-GitHub remote, or an empty jq result all read as no status.
fn github_query(ctx: &Context, query: &str, jq: &str) -> Option<String> {
    let origin = run_argv(&["git", "remote", "get-url", "origin"], ctx)?;
    let branch = run_argv(&["git", "branch", "--show-current"], ctx)?;
    let (owner, repo) = github_repo(&origin).ok()?;
    let state = run_argv(
        &[
            "gh",
            "api",
            "graphql",
            "-F",
            &format!("owner={owner}"),
            "-F",
            &format!("repo={repo}"),
            "-F",
            &format!("branch={branch}"),
            "-f",
            &format!("query={query}"),
            "--jq",
            jq,
        ],
        ctx,
    )?;
    if state.is_empty() {
        return None;
    }
    Some(state.to_lowercase())
}

/// The `github` built-in: the branch's latest PR collapsed into one cell.
///
/// Shows the most urgent fact about the PR: merged / closed / conflicts /
/// failing / draft / pending / ready. No PR reads as no status.
pub fn github_status(ctx: &Context) -> Option<String> {
    let raw = github_query(ctx, GITHUB_QUERY, GITHUB_JQ)?;
    github_state(&raw)
}

/// Collapse '<state> <draft> <mergeable> <ci>' into the most urgent fact.
fn github_state(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    let [state, draft, mergeable, ci] = parts.as_slice() else {
        return None;
    };
    let fact = if *state == "merged" || *state == "closed" {
        state
    } else if *mergeable == "conflicting" {
        "conflicts"
    } else if *ci == "failure" || *ci == "error" {
        "failing"
    } else if *draft == "true" {
        "draft"
    } else if *ci == "pending" || *ci == "expected" {
        "pending"
    } else {
        "ready"
    };
    Some(fact.to_string())
}

// Compact display forms per built-in; colour still keys on the status word.
// Nerd-font glyphs by default, plain Unicode with nerd_font = false.
fn builtin_icon(builtin: &str, word: &str, nerd_font: bool) -> Option<&'static str> {
    if builtin != "github" {
        return None;
    }
    if nerd_font {
        match word {
            "merged" => Some("\u{f419}"),    // nf-oct-git_merge
            "closed" => Some("\u{f05e}"),    // nf-fa-ban
            "conflicts" => Some("\u{f071}"), // nf-fa-warning
            "failing" => Some("\u{f00d}"),   // nf-fa-times
            "draft" => Some("\u{f040}"),     // nf-fa-pencil
            "pending" => Some("\u{f017}"),   // nf-fa-clock_o
            "ready" => Some("\u{f00c}"),     // nf-fa-check
            _ => None,
        }
    } else {
        match word {
            "merged" => Some("◆"),
            "closed" => Some("⊘"),
            "conflicts" => Some("⚠"),
            "failing" => Some("✖"),
            "draft" => Some("✎"),
            "pending" => Some("◌"),
            "ready" => Some("✔"),
            _ => None,
        }
    }
}

/// A cell's display form: its leading word mapped through the built-in's icons.
///
/// Any detail after the word (e.g. the elapsed time in "working 1m30s")
/// is kept as is.
pub fn cell_icon(column: &StatusColumn, cell: &str, nerd_font: bool) -> String {
    let (word, rest) = match cell.split_once(' ') {
        Some((word, rest)) => (word, rest),
        None => (cell, ""),
    };
    let display = column
        .builtin
        .as_deref()
        .and_then(|builtin| builtin_icon(builtin, word, nerd_font))
        .unwrap_or(word);
    if rest.is_empty() {
        display.to_string()
    } else {
        format!("{display} {rest}")
    }
}

/// A cell's colour: the shared vocabulary's style for its leading word.
pub fn cell_style(cell: &str) -> Option<&'static str> {
    let word = cell.split(' ').next().unwrap_or(cell);
    STATUS_STYLES
        .iter()
        .find(|(known, _)| *known == word)
        .map(|(_, style)| *style)
}

// Colours for well-known status words, keyed by value so that command
// providers speaking the same vocabulary get them too. GitHub's conventions
// for PR and check states; attention-based colours for agent states (red
// needs you now, yellow wants new instructions, green is progressing).
pub const STATUS_STYLES: &[(&str, &str)] = &[
    ("working", "bold bright_green"),
    ("monitoring", "bold bright_cyan"),
    ("open", "bold bright_green"),
    ("success", "bold bright_green"),
    ("idle", "bold bright_yellow"),
    ("pending", "bold bright_yellow"),
    ("blocked", "bold bright_red"),
    ("closed", "bold bright_red"),
    ("failure", "bold bright_red"),
    ("error", "bold bright_red"),
    ("failing", "bold bright_red"),
    ("conflicts", "bold bright_yellow"),
    ("ready", "bold bright_green"),
    ("merged", "bold bright_magenta"),
    ("draft", "bright_black"),
];

/// Seconds between runs of a column's provider; 0 means every ask.
pub fn refresh_interval(column: &StatusColumn) -> f64 {
    if let Some(interval) = column.interval {
        return interval;
    }
    // Default refresh interval per built-in: how often a caller should re-run
    // the provider. Keeps the GitHub built-in well inside API rate limits when
    // the caller polls every couple of seconds. 0 means every ask.
    match column.builtin.as_deref() {
        Some("github") => 30.0,
        _ => 0.0,
    }
}

pub fn column_status(ctx: &Context, column: &StatusColumn) -> Option<String> {
    if let Some(command) = &column.command {
        return command_status(ctx, command);
    }
    // parse_status guarantees command xor builtin.
    match column.builtin.as_deref().expect("column has a builtin") {
        "agent" => agent_status(ctx),
        "github" => github_status(ctx),
        other => unreachable!("unknown status builtin '{other}' survived config parsing"),
    }
}

// Scoped provider threads carry the calling test's env stubs along.
#[cfg(test)]
use crate::testutil::propagate_env as carry_env;
#[cfg(not(test))]
fn carry_env<R, F: FnOnce() -> R + Send>(f: F) -> F {
    f
}

/// Compact git state: `*` for uncommitted changes, `↑n` for unpushed commits.
pub fn git_state(ctx: &Context) -> String {
    // Both probes can idle up to the timeout; overlap them.
    let (dirty, unpushed) = std::thread::scope(|scope| {
        let dirty = scope.spawn(carry_env(|| {
            run_argv(&["git", "status", "--porcelain"], ctx)
        }));
        let unpushed = run_argv(
            &[
                "git",
                "rev-list",
                "--count",
                "--branches",
                "--not",
                "--remotes",
            ],
            ctx,
        );
        (dirty.join().unwrap_or(None), unpushed)
    });
    let mut parts = Vec::new();
    if dirty.is_some_and(|out| !out.is_empty()) {
        parts.push("*".to_string());
    }
    if let Some(count) = unpushed
        && !count.is_empty()
        && count != "0"
    {
        parts.push(format!("↑{count}"));
    }
    parts.join(" ")
}

/// The STATUS column's value plus one display cell per configured column.
pub fn status_cells(cfg: &Config, ctx: &Context) -> Vec<String> {
    // Each provider can take up to the full timeout; running them together
    // keeps a listing's latency at the slowest provider, not the sum.
    let (state, cells) = std::thread::scope(|scope| {
        let columns: Vec<_> = cfg
            .status
            .iter()
            .map(|column| scope.spawn(carry_env(move || column_status(ctx, column))))
            .collect();
        let state = git_state(ctx);
        let cells: Vec<Option<String>> = columns
            .into_iter()
            .map(|handle| handle.join().unwrap_or(None))
            .collect();
        (state, cells)
    });
    let mut out = vec![state];
    for (column, cell) in cfg.status.iter().zip(cells) {
        out.push(match cell {
            Some(cell) if !cell.is_empty() => cell_icon(column, &cell, cfg.nerd_font),
            _ => String::new(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config;
    use crate::repos::add_repo;
    use crate::testutil::{TestEnv, commit_file, test_env};

    fn context() -> (TestEnv, Context) {
        let env = test_env();
        let origin = env.origin();
        add_repo(&env.cfg, &origin.to_string_lossy(), None).unwrap();
        let ctx = crate::contexts::create_context(&env.cfg, "origin", "feat", None).unwrap();
        (env, ctx)
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

    fn set_mtime_secs_ago(path: &PathBuf, ago: u64) {
        let when = SystemTime::now() - Duration::from_secs(ago);
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_times(
            std::fs::FileTimes::new()
                .set_accessed(when)
                .set_modified(when),
        )
        .unwrap();
    }

    #[test]
    fn every_allowlisted_builtin_dispatches() {
        // column_status panics on a builtin the allowlist admits but the
        // dispatch below doesn't know; probe each against a real checkout.
        let (_env, ctx) = context();

        for builtin in config::BUILTIN_STATUS {
            column_status(&ctx, &column(builtin, None, Some(builtin), None));
        }
    }

    #[test]
    fn github_state_collapses_to_the_most_urgent_fact() {
        for (raw, state) in [
            ("merged false mergeable none", Some("merged")),
            ("closed false conflicting failure", Some("closed")),
            ("open false conflicting success", Some("conflicts")),
            ("open true mergeable failure", Some("failing")),
            ("open false mergeable error", Some("failing")),
            ("open true mergeable success", Some("draft")),
            ("open false mergeable pending", Some("pending")),
            ("open false unknown success", Some("ready")),
            ("open false mergeable none", Some("ready")),
            ("garbage", None),
        ] {
            assert_eq!(github_state(raw).as_deref(), state, "raw: {raw}");
        }
    }

    #[test]
    fn github_status_combines_the_query_fields() {
        let (env, ctx) = context();
        let _gh = env.fake_cli("gh", "echo 'OPEN false MERGEABLE FAILURE'");

        assert_eq!(github_status(&ctx).as_deref(), Some("failing"));
    }

    #[test]
    fn github_status_without_a_pr_is_empty() {
        let (env, ctx) = context();
        let _gh = env.fake_cli("gh", "exit 0");

        assert_eq!(github_status(&ctx), None);
    }

    #[test]
    fn github_cells_render_as_nerd_font_icons() {
        let column = column("pr", None, Some("github"), None);

        assert_eq!(cell_icon(&column, "merged", true), "\u{f419}");
        assert_eq!(cell_icon(&column, "ready", true), "\u{f00c}");
    }

    #[test]
    fn nerd_font_off_falls_back_to_plain_unicode() {
        let column = column("pr", None, Some("github"), None);

        assert_eq!(cell_icon(&column, "merged", false), "◆");
        assert_eq!(cell_icon(&column, "ready", false), "✔");
    }

    #[test]
    fn command_cells_show_their_word() {
        let column = column("claude", Some("echo working"), None, None);

        assert_eq!(cell_icon(&column, "working", true), "working");
    }

    #[test]
    fn cell_icons_keep_the_detail_after_the_word() {
        let column = column("claude", None, Some("agent"), None);

        assert_eq!(cell_icon(&column, "working 12m", true), "working 12m");
    }

    #[test]
    fn cell_style_keys_on_the_leading_word() {
        assert_eq!(cell_style("working 12m"), Some("bold bright_green"));
        assert_eq!(cell_style("idle"), Some("bold bright_yellow"));
        assert_eq!(cell_style("anything-else"), None);
    }

    #[test]
    fn command_status_returns_the_first_output_line() {
        let (_env, ctx) = context();

        assert_eq!(
            command_status(&ctx, "printf 'working\\nextra'").as_deref(),
            Some("working")
        );
    }

    #[test]
    fn command_status_runs_in_the_checkout() {
        let (_env, ctx) = context();
        std::fs::write(ctx.path.join(".git").join("agent-status"), "blocked\n").unwrap();

        assert_eq!(
            command_status(&ctx, "cat .git/agent-status").as_deref(),
            Some("blocked")
        );
    }

    #[test]
    fn command_status_exposes_the_context_in_env() {
        let (_env, ctx) = context();

        assert_eq!(
            command_status(&ctx, "echo \"$CTX_REPO/$CTX_NAME\"").as_deref(),
            Some("origin/feat")
        );
    }

    #[test]
    fn command_status_is_bounded_even_when_a_grandchild_holds_the_pipe() {
        // A backgrounded process inherits the provider's stdout; the read
        // must give up at the timeout instead of waiting for its exit.
        let (_env, ctx) = context();

        let start = std::time::Instant::now();
        let cell = command_status(&ctx, "sleep 5 & echo working");

        assert_eq!(cell, None);
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "the provider read outlived the timeout"
        );
    }

    #[test]
    fn status_cells_run_a_context_s_providers_concurrently() {
        let (env, ctx) = context();
        let mut cfg = env.cfg.clone();
        cfg.status = vec![
            column("a", Some("sleep 0.6; echo a"), None, None),
            column("b", Some("sleep 0.6; echo b"), None, None),
        ];

        let start = std::time::Instant::now();
        let cells = status_cells(&cfg, &ctx);

        assert_eq!(cells, vec!["", "a", "b"]);
        assert!(
            start.elapsed() < Duration::from_millis(1100),
            "providers ran sequentially: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn command_status_swallows_failures_and_silence() {
        let (_env, ctx) = context();

        assert_eq!(command_status(&ctx, "cat .git/agent-status"), None);
        assert_eq!(command_status(&ctx, "true"), None);
    }

    #[test]
    fn agent_status_reads_the_status_file() {
        let (_env, ctx) = context();
        std::fs::write(ctx.path.join(".git").join("agent-status"), "blocked\n").unwrap();

        assert_eq!(agent_status(&ctx).as_deref(), Some("blocked"));
    }

    #[test]
    fn agent_status_shows_how_long_active_states_have_run() {
        // The hooks rewrite the file only on change, so mtime is the state's start.
        let (_env, ctx) = context();
        let path = ctx.path.join(".git").join("agent-status");
        std::fs::write(&path, "working\n").unwrap();
        set_mtime_secs_ago(&path, 300);

        assert_eq!(agent_status(&ctx).as_deref(), Some("working 5m"));

        std::fs::write(&path, "monitoring\n").unwrap();
        set_mtime_secs_ago(&path, 300);
        assert_eq!(agent_status(&ctx).as_deref(), Some("monitoring 5m"));
    }

    #[test]
    fn elapsed_formats_by_magnitude() {
        assert_eq!(elapsed(42.0), "42s");
        assert_eq!(elapsed(99.0), "1m");
        assert_eq!(elapsed(300.0), "5m");
        assert_eq!(elapsed(3900.0), "1h5m");
    }

    #[test]
    fn agent_status_without_a_file_is_empty() {
        let (_env, ctx) = context();

        assert_eq!(agent_status(&ctx), None);
    }

    #[test]
    fn agent_status_ignores_stale_files() {
        let (_env, ctx) = context();
        let path = ctx.path.join(".git").join("agent-status");
        std::fs::write(&path, "working\n").unwrap();
        set_mtime_secs_ago(&path, 4000);

        assert_eq!(agent_status(&ctx), None);
    }

    #[test]
    fn github_repo_parses_remote_url_forms() {
        for url in [
            "git@github.com:jane/tool.git",
            "https://github.com/jane/tool.git",
            "https://github.com/jane/tool",
            "ssh://git@github.com/jane/tool.git",
        ] {
            assert_eq!(
                github_repo(url).unwrap(),
                ("jane".to_string(), "tool".to_string())
            );
        }
    }

    #[test]
    fn github_repo_rejects_unparseable_urls() {
        let err = github_repo("nonsense").expect_err("must reject");

        assert!(err.to_string().contains("cannot parse"));
    }

    #[test]
    fn github_builtin_defaults_to_a_coarse_interval() {
        assert_eq!(
            refresh_interval(&column("pr", None, Some("github"), None)),
            30.0
        );
    }

    #[test]
    fn other_columns_default_to_every_ask() {
        assert_eq!(
            refresh_interval(&column("a", None, Some("agent"), None)),
            0.0
        );
        assert_eq!(
            refresh_interval(&column("c", Some("echo hi"), None, None)),
            0.0
        );
    }

    #[test]
    fn a_user_interval_overrides_the_default() {
        assert_eq!(
            refresh_interval(&column("pr", None, Some("github"), Some(5.0))),
            5.0
        );
        assert_eq!(
            refresh_interval(&column("c", Some("echo hi"), None, Some(60.0))),
            60.0
        );
    }

    #[test]
    fn github_swallows_gh_failures() {
        let (env, ctx) = context();
        let _gh = env.fake_cli("gh", "exit 1");

        assert_eq!(github_status(&ctx), None);
    }

    #[test]
    fn column_status_dispatches_on_the_column_kind() {
        let (env, ctx) = context();
        let _gh = env.fake_cli("gh", "echo 'OPEN false MERGEABLE FAILURE'");
        std::fs::write(ctx.path.join(".git").join("agent-status"), "idle\n").unwrap();

        assert_eq!(
            column_status(&ctx, &column("c", Some("echo hi"), None, None)).as_deref(),
            Some("hi")
        );
        assert_eq!(
            column_status(&ctx, &column("a", None, Some("agent"), None)).as_deref(),
            Some("idle")
        );
        assert_eq!(
            column_status(&ctx, &column("g", None, Some("github"), None)).as_deref(),
            Some("failing")
        );
    }

    #[test]
    fn git_state_is_empty_for_a_clean_checkout() {
        let (_env, ctx) = context();

        assert_eq!(git_state(&ctx), "");
    }

    #[test]
    fn git_state_marks_dirty_and_unpushed_work() {
        let (_env, ctx) = context();
        commit_file(&ctx.path, "work.txt", "x\n");
        std::fs::write(ctx.path.join("scratch.txt"), "x\n").unwrap();

        assert_eq!(git_state(&ctx), "* ↑1");
    }

    #[test]
    fn status_cells_hold_git_state_and_column_output() {
        let (env, ctx) = context();
        let mut cfg = env.cfg.clone();
        cfg.status = vec![
            column("claude", None, Some("agent"), None),
            column("ci", Some("false"), None, None),
        ];
        std::fs::write(ctx.path.join(".git").join("agent-status"), "working\n").unwrap();
        std::fs::write(ctx.path.join("scratch.txt"), "x\n").unwrap();

        assert_eq!(status_cells(&cfg, &ctx), vec!["*", "working 0s", ""]);
    }

    #[test]
    fn status_cells_without_columns_report_git_state() {
        let (env, ctx) = context();

        assert_eq!(status_cells(&env.cfg, &ctx), vec![""]);
    }
}
