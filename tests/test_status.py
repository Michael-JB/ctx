import asyncio
import os
from collections.abc import Coroutine
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from conftest import add_repo, commit_file, create_context

from ctx import config, contexts, status
from ctx.config import Config, StatusColumn


def run[T](coro: Coroutine[Any, Any, T]) -> T:
    return asyncio.run(coro)


@pytest.fixture
def registered(cfg: Config, origin: Path) -> Path:
    add_repo(cfg, str(origin))
    return origin


@pytest.fixture
def ctx(cfg: Config, registered: Path) -> contexts.Context:
    return create_context(cfg, "origin", "feat")


def fake_gh(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, script: str) -> None:
    """Shadow gh on PATH with a stub script."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=True)
    gh = bin_dir / "gh"
    gh.write_text(f"#!/bin/sh\n{script}\n")
    gh.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bin_dir}:{os.environ['PATH']}")


def test_builtins_match_the_config_allowlist() -> None:
    assert set(status.BUILTINS) == set(config.BUILTIN_STATUS)


@pytest.mark.parametrize(
    ("raw", "state"),
    [
        ("merged false mergeable none", "merged"),
        ("closed false conflicting failure", "closed"),
        ("open false conflicting success", "conflicts"),
        ("open true mergeable failure", "failing"),
        ("open false mergeable error", "failing"),
        ("open true mergeable success", "draft"),
        ("open false mergeable pending", "pending"),
        ("open false unknown success", "ready"),
        ("open false mergeable none", "ready"),
        ("garbage", None),
    ],
)
def test_github_state_collapses_to_the_most_urgent_fact(raw: str, state: str | None) -> None:
    assert status._github_state(raw) == state


def test_github_status_combines_the_query_fields(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, "echo 'OPEN false MERGEABLE FAILURE'")

    assert run(status.github_status(ctx)) == "failing"


def test_github_status_without_a_pr_is_empty(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, "exit 0")

    assert run(status.github_status(ctx)) is None


def test_github_cells_render_as_icons() -> None:
    column = StatusColumn("pr", builtin="github")

    assert status.cell_icon(column, "merged") == "◆"
    assert status.cell_icon(column, "ready") == "✔"


def test_configured_icons_override_the_defaults() -> None:
    column = StatusColumn("pr", builtin="github", icons={"merged": "M"})

    assert status.cell_icon(column, "merged") == "M"
    assert status.cell_icon(column, "ready") == "✔"


def test_command_cells_show_their_word_unless_icons_are_configured() -> None:
    column = StatusColumn("claude", command="echo working", icons={"working": "▶"})

    assert status.cell_icon(column, "working") == "▶"
    assert status.cell_icon(StatusColumn("claude", command="echo working"), "working") == "working"


def test_command_status_returns_the_first_output_line(ctx: contexts.Context) -> None:
    assert run(status.command_status(ctx, "printf 'working\\nextra'")) == "working"


def test_command_status_runs_in_the_checkout(ctx: contexts.Context) -> None:
    (ctx.path / ".git" / "agent-status").write_text("blocked\n")

    assert run(status.command_status(ctx, "cat .git/agent-status")) == "blocked"


def test_command_status_exposes_the_context_in_env(ctx: contexts.Context) -> None:
    assert run(status.command_status(ctx, 'echo "$CTX_REPO/$CTX_NAME"')) == "origin/feat"


def test_command_status_swallows_failures_and_silence(ctx: contexts.Context) -> None:
    assert run(status.command_status(ctx, "cat .git/agent-status")) is None
    assert run(status.command_status(ctx, "true")) is None


def test_agent_status_reads_the_status_file(ctx: contexts.Context) -> None:
    (ctx.path / ".git" / "agent-status").write_text("working\n")

    assert run(status.agent_status(ctx)) == "working"


def test_agent_status_without_a_file_is_empty(ctx: contexts.Context) -> None:
    assert run(status.agent_status(ctx)) is None


def test_agent_status_ignores_stale_files(ctx: contexts.Context) -> None:
    path = ctx.path / ".git" / "agent-status"
    path.write_text("working\n")
    os.utime(path, (1_000, 1_000))

    assert run(status.agent_status(ctx)) is None


@pytest.mark.parametrize(
    "url",
    [
        "git@github.com:jane/tool.git",
        "https://github.com/jane/tool.git",
        "https://github.com/jane/tool",
        "ssh://git@github.com/jane/tool.git",
    ],
)
def test_github_repo_parses_remote_url_forms(url: str) -> None:
    assert status.github_repo(url) == ("jane", "tool")


def test_github_repo_rejects_unparseable_urls() -> None:
    with pytest.raises(ValueError, match="cannot parse"):
        status.github_repo("nonsense")


def test_github_builtin_defaults_to_a_coarse_interval() -> None:
    assert status.refresh_interval(StatusColumn("pr", builtin="github")) == 30.0


def test_other_columns_default_to_every_ask() -> None:
    assert status.refresh_interval(StatusColumn("a", builtin="agent")) == 0.0
    assert status.refresh_interval(StatusColumn("c", command="echo hi")) == 0.0


def test_a_user_interval_overrides_the_default() -> None:
    assert status.refresh_interval(StatusColumn("pr", builtin="github", interval=5)) == 5.0
    assert status.refresh_interval(StatusColumn("c", command="echo hi", interval=60)) == 60.0


def test_github_swallows_gh_failures(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, "exit 1")

    assert run(status.github_status(ctx)) is None


def test_column_status_dispatches_on_the_column_kind(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, "echo 'OPEN false MERGEABLE FAILURE'")
    (ctx.path / ".git" / "agent-status").write_text("idle\n")

    assert run(status.column_status(ctx, StatusColumn("c", command="echo hi"))) == "hi"
    assert run(status.column_status(ctx, StatusColumn("a", builtin="agent"))) == "idle"
    assert run(status.column_status(ctx, StatusColumn("g", builtin="github"))) == "failing"


def test_git_state_is_empty_for_a_clean_checkout(ctx: contexts.Context) -> None:
    assert run(status.git_state(ctx)) == ""


def test_git_state_marks_dirty_and_unpushed_work(ctx: contexts.Context) -> None:
    commit_file(ctx.path, "work.txt")
    (ctx.path / "scratch.txt").write_text("x\n")

    assert run(status.git_state(ctx)) == "* ↑1"


def test_status_cells_hold_git_state_and_column_output(cfg: Config, ctx: contexts.Context) -> None:
    cfg = replace(
        cfg,
        status=(StatusColumn("claude", builtin="agent"), StatusColumn("ci", command="false")),
    )
    (ctx.path / ".git" / "agent-status").write_text("working\n")
    (ctx.path / "scratch.txt").write_text("x\n")

    assert run(status.status_cells(cfg, ctx)) == ["*", "working", ""]


def test_status_cells_without_columns_report_git_state(cfg: Config, ctx: contexts.Context) -> None:
    assert run(status.status_cells(cfg, ctx)) == [""]
