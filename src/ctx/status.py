"""Status column providers: user commands plus the built-ins."""

import os
import subprocess
import time
from pathlib import Path

from ctx.config import Config, StatusColumn
from ctx.contexts import Context, describe_status

_TIMEOUT = 2.0

_AGENT_STALE_SECONDS = 3600.0

# Default sampling interval per built-in. Callers may ask every couple of
# seconds (the TUI poll); a column's provider runs at most once per interval,
# which keeps the GitHub built-ins well inside API rate limits. 0 means a
# fresh value on every ask.
_DEFAULT_INTERVALS = {"github-checks": 30.0, "github-pr": 30.0}

_samples: dict[tuple[Path, str], tuple[float, str | None]] = {}

_ROLLUP_QUERY = """
query($owner: String!, $repo: String!, $branch: String!) {
  repository(owner: $owner, name: $repo) {
    pullRequests(headRefName: $branch, first: 1, states: OPEN) {
      nodes { commits(last: 1) { nodes { commit { statusCheckRollup { state } } } } }
    }
  }
}
"""
_ROLLUP_JQ = (
    ".data.repository.pullRequests.nodes[0].commits.nodes[0].commit.statusCheckRollup.state"
    " // empty"
)

_PR_QUERY = """
query($owner: String!, $repo: String!, $branch: String!) {
  repository(owner: $owner, name: $repo) {
    pullRequests(headRefName: $branch, first: 1, orderBy: {field: CREATED_AT, direction: DESC}) {
      nodes { state isDraft }
    }
  }
}
"""
_PR_JQ = (
    ".data.repository.pullRequests.nodes[0]"
    ' | if . == null then empty elif .state == "OPEN" and .isDraft then "DRAFT" else .state end'
)


def _run(args: list[str] | str, ctx: Context, shell: bool = False) -> str | None:
    """First line of a command's output, run in the checkout; None if it yields nothing.

    Failures (non-zero exit, timeout, missing executable) also yield None: the
    contract is "produce a status or stay quiet", so a broken or inapplicable
    provider must not break listings.
    """
    try:
        proc = subprocess.run(
            args,
            shell=shell,
            cwd=ctx.path,
            env={**os.environ, "CTX_REPO": ctx.repo, "CTX_NAME": ctx.name},
            capture_output=True,
            text=True,
            timeout=_TIMEOUT,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if proc.returncode != 0:
        return None
    lines = proc.stdout.strip().splitlines()
    return lines[0].strip() if lines else None


def command_status(ctx: Context, command: str) -> str | None:
    """The `command` provider: a user-configured shell command."""
    return _run(command, ctx, shell=True)


def agent_status(ctx: Context) -> str | None:
    """The `agent` built-in: the checkout's agent-status file.

    Agent harness hooks write a word (e.g. working/blocked/idle) to
    `.git/agent-status`. A file untouched for an hour is stale — the agent
    likely died without its hooks firing — and reads as no status.
    """
    path = ctx.path / ".git" / "agent-status"
    try:
        if time.time() - path.stat().st_mtime > _AGENT_STALE_SECONDS:
            return None
        lines = path.read_text().strip().splitlines()
    except OSError:
        return None
    return lines[0].strip() if lines else None


def github_repo(url: str) -> tuple[str, str]:
    """The (owner, repo) of a git remote URL, tolerating ssh/https/scp forms."""
    parts = url.removesuffix(".git").replace(":", "/").rsplit("/", 2)
    if len(parts) < 3 or not parts[1] or not parts[2]:
        raise ValueError(f"cannot parse owner/repo from remote URL '{url}'")
    return parts[1], parts[2]


def _github_query(ctx: Context, query: str, jq: str) -> str | None:
    """A GraphQL query via gh for the checkout's branch, lowercased.

    No `gh`, a non-GitHub remote, or an empty jq result all read as no status.
    """
    origin = _run(["git", "remote", "get-url", "origin"], ctx)
    branch = _run(["git", "branch", "--show-current"], ctx)
    if origin is None or branch is None:
        return None
    try:
        owner, repo = github_repo(origin)
    except ValueError:
        return None
    state = _run(
        [
            "gh",
            "api",
            "graphql",
            "-F",
            f"owner={owner}",
            "-F",
            f"repo={repo}",
            "-F",
            f"branch={branch}",
            "-f",
            f"query={query}",
            "--jq",
            jq,
        ],
        ctx,
    )
    return state.lower() if state else None


def github_checks_status(ctx: Context) -> str | None:
    """The `github-checks` built-in: CI state of the branch's open PR.

    GitHub aggregates a PR head commit's checks server-side into a single
    rollup state; this is the value behind the web UI's green tick. Shown as
    success/failure/pending/error; no open PR or no checks reads as no status.
    """
    return _github_query(ctx, _ROLLUP_QUERY, _ROLLUP_JQ)


def github_pr_status(ctx: Context) -> str | None:
    """The `github-pr` built-in: state of the branch's latest PR.

    Shown as open/draft/merged/closed; no PR reads as no status.
    """
    return _github_query(ctx, _PR_QUERY, _PR_JQ)


BUILTINS = {
    "agent": agent_status,
    "github-checks": github_checks_status,
    "github-pr": github_pr_status,
}

# Colours for well-known status words, keyed by value so that command
# providers speaking the same vocabulary get them too. GitHub's conventions
# for PR and check states; attention-based colours for agent states (red
# needs you now, yellow wants new instructions, green is progressing).
STATUS_STYLES = {
    "working": "green",
    "open": "green",
    "success": "green",
    "idle": "yellow",
    "pending": "yellow",
    "blocked": "red",
    "closed": "red",
    "failure": "red",
    "error": "red",
    "merged": "magenta",
    "draft": "bright_black",
}


def sample_interval(column: StatusColumn) -> float:
    """Seconds a column's value is served before its provider runs again."""
    if column.interval is not None:
        return column.interval
    return _DEFAULT_INTERVALS.get(column.builtin or "", 0.0)


def column_status(ctx: Context, column: StatusColumn) -> str | None:
    interval = sample_interval(column)
    key = (ctx.path, column.name)
    if interval > 0:
        hit = _samples.get(key)
        if hit is not None and time.time() < hit[0]:
            return hit[1]
    value = _provider_status(ctx, column)
    if interval > 0:
        _samples[key] = (time.time() + interval, value)
    return value


def _provider_status(ctx: Context, column: StatusColumn) -> str | None:
    if column.command is not None:
        return command_status(ctx, column.command)
    assert column.builtin is not None  # parse_status guarantees command xor builtin
    return BUILTINS[column.builtin](ctx)


def status_cells(cfg: Config, ctx: Context) -> list[str]:
    """The STATUS column's value plus one cell per configured status column."""
    return [describe_status(ctx)] + [column_status(ctx, col) or "" for col in cfg.status]
