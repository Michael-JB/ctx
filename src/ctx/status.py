"""Status column providers: user commands plus the built-ins."""

import asyncio
import os
import time

from ctx.config import Config, StatusColumn
from ctx.contexts import Context

_TIMEOUT = 2.0

_AGENT_STALE_SECONDS = 3600.0

# Default refresh interval per built-in: how often a caller should re-run the
# provider. Keeps the GitHub built-in well inside API rate limits when the
# caller polls every couple of seconds. 0 means every ask.
_DEFAULT_INTERVALS = {"github": 30.0}

_GITHUB_QUERY = """
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
"""
_GITHUB_JQ = (
    ".data.repository.pullRequests.nodes[0]"
    " | if . == null then empty else"
    " [.state, (.isDraft | tostring), .mergeable,"
    ' (.commits.nodes[0].commit.statusCheckRollup.state // "NONE")]'
    ' | join(" ") end'
)


async def _run(args: list[str] | str, ctx: Context) -> str | None:
    """First line of a command's output, run in the checkout; None if it yields nothing.

    A string runs through the shell, a list directly. Failures (non-zero exit,
    timeout, missing executable) also yield None: the contract is "produce a
    status or stay quiet", so a broken or inapplicable provider must not break
    listings.
    """
    env = {**os.environ, "CTX_REPO": ctx.repo, "CTX_NAME": ctx.name}
    pipe, devnull = asyncio.subprocess.PIPE, asyncio.subprocess.DEVNULL
    try:
        if isinstance(args, str):
            proc = await asyncio.create_subprocess_shell(
                args, cwd=ctx.path, env=env, stdout=pipe, stderr=devnull
            )
        else:
            proc = await asyncio.create_subprocess_exec(
                *args, cwd=ctx.path, env=env, stdout=pipe, stderr=devnull
            )
    except OSError:
        return None
    try:
        stdout, _ = await asyncio.wait_for(proc.communicate(), _TIMEOUT)
    except TimeoutError:
        proc.kill()
        await proc.wait()
        return None
    if proc.returncode != 0:
        return None
    lines = stdout.decode(errors="replace").strip().splitlines()
    return lines[0].strip() if lines else None


async def command_status(ctx: Context, command: str) -> str | None:
    """The `command` provider: a user-configured shell command."""
    return await _run(command, ctx)


async def agent_status(ctx: Context) -> str | None:
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


async def _github_query(ctx: Context, query: str, jq: str) -> str | None:
    """A GraphQL query via gh for the checkout's branch, lowercased.

    No `gh`, a non-GitHub remote, or an empty jq result all read as no status.
    """
    origin, branch = await asyncio.gather(
        _run(["git", "remote", "get-url", "origin"], ctx),
        _run(["git", "branch", "--show-current"], ctx),
    )
    if origin is None or branch is None:
        return None
    try:
        owner, repo = github_repo(origin)
    except ValueError:
        return None
    state = await _run(
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


async def github_status(ctx: Context) -> str | None:
    """The `github` built-in: the branch's latest PR collapsed into one cell.

    Shows the most urgent fact about the PR: merged / closed / conflicts /
    failing / draft / pending / ready. No PR reads as no status.
    """
    raw = await _github_query(ctx, _GITHUB_QUERY, _GITHUB_JQ)
    return _github_state(raw) if raw else None


def _github_state(raw: str) -> str | None:
    """Collapse '<state> <draft> <mergeable> <ci>' into the most urgent fact."""
    try:
        state, draft, mergeable, ci = raw.split()
    except ValueError:
        return None
    if state in ("merged", "closed"):
        return state
    if mergeable == "conflicting":
        return "conflicts"
    if ci in ("failure", "error"):
        return "failing"
    if draft == "true":
        return "draft"
    if ci in ("pending", "expected"):
        return "pending"
    return "ready"


BUILTINS = {
    "agent": agent_status,
    "github": github_status,
}

# Compact display forms per built-in; colour still keys on the status word.
_DEFAULT_ICONS: dict[str, dict[str, str]] = {
    "github": {
        "merged": "◆",
        "closed": "⊘",
        "conflicts": "⚠",
        "failing": "✖",
        "draft": "✎",
        "pending": "◌",
        "ready": "✔",
    },
}


def cell_icon(column: StatusColumn, cell: str) -> str:
    """A cell's display form: the column's icon for it, else the cell itself."""
    icons = {**_DEFAULT_ICONS.get(column.builtin or "", {}), **column.icons}
    return icons.get(cell, cell)


# Colours for well-known status words, keyed by value so that command
# providers speaking the same vocabulary get them too. GitHub's conventions
# for PR and check states; attention-based colours for agent states (red
# needs you now, yellow wants new instructions, green is progressing).
STATUS_STYLES = {
    "working": "bold bright_green",
    "open": "bold bright_green",
    "success": "bold bright_green",
    "idle": "bold bright_yellow",
    "pending": "bold bright_yellow",
    "blocked": "bold bright_red",
    "closed": "bold bright_red",
    "failure": "bold bright_red",
    "error": "bold bright_red",
    "failing": "bold bright_red",
    "conflicts": "bold bright_yellow",
    "ready": "bold bright_green",
    "merged": "bold bright_magenta",
    "draft": "bright_black",
}


def refresh_interval(column: StatusColumn) -> float:
    """Seconds between runs of a column's provider; 0 means every ask."""
    if column.interval is not None:
        return column.interval
    return _DEFAULT_INTERVALS.get(column.builtin or "", 0.0)


async def column_status(ctx: Context, column: StatusColumn) -> str | None:
    if column.command is not None:
        return await command_status(ctx, column.command)
    assert column.builtin is not None  # parse_status guarantees command xor builtin
    return await BUILTINS[column.builtin](ctx)


async def git_state(ctx: Context) -> str:
    """Compact git state: `*` for uncommitted changes, `↑n` for unpushed commits."""
    dirty, unpushed = await asyncio.gather(
        _run(["git", "status", "--porcelain"], ctx),
        _run(["git", "rev-list", "--count", "--branches", "--not", "--remotes"], ctx),
    )
    parts = []
    if dirty:
        parts.append("*")
    if unpushed and unpushed != "0":
        parts.append(f"↑{unpushed}")
    return " ".join(parts)


async def status_cells(cfg: Config, ctx: Context) -> list[str]:
    """The STATUS column's value plus one display cell per configured column."""
    state, *cells = await asyncio.gather(
        git_state(ctx), *(column_status(ctx, col) for col in cfg.status)
    )
    return [state or ""] + [
        cell_icon(col, cell) if cell else "" for col, cell in zip(cfg.status, cells, strict=True)
    ]
