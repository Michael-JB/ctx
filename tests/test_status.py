import os
import time
from dataclasses import replace
from pathlib import Path

import pytest

from ctx import config, contexts, repos, status
from ctx.config import Config, StatusColumn


@pytest.fixture(autouse=True)
def fresh_samples() -> None:
    status._samples.clear()


@pytest.fixture
def registered(cfg: Config, origin: Path) -> Path:
    repos.add_repo(cfg, str(origin))
    return origin


@pytest.fixture
def ctx(cfg: Config, registered: Path) -> contexts.Context:
    return contexts.create_context(cfg, "origin", "feat")


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


def test_command_status_returns_the_first_output_line(ctx: contexts.Context) -> None:
    assert status.command_status(ctx, "printf 'working\\nextra'") == "working"


def test_command_status_runs_in_the_checkout(ctx: contexts.Context) -> None:
    (ctx.path / ".git" / "agent-status").write_text("blocked\n")

    assert status.command_status(ctx, "cat .git/agent-status") == "blocked"


def test_command_status_exposes_the_context_in_env(ctx: contexts.Context) -> None:
    assert status.command_status(ctx, 'echo "$CTX_REPO/$CTX_NAME"') == "origin/feat"


def test_command_status_swallows_failures_and_silence(ctx: contexts.Context) -> None:
    assert status.command_status(ctx, "cat .git/agent-status") is None
    assert status.command_status(ctx, "true") is None


def test_agent_status_reads_the_status_file(ctx: contexts.Context) -> None:
    (ctx.path / ".git" / "agent-status").write_text("working\n")

    assert status.agent_status(ctx) == "working"


def test_agent_status_without_a_file_is_empty(ctx: contexts.Context) -> None:
    assert status.agent_status(ctx) is None


def test_agent_status_ignores_stale_files(ctx: contexts.Context) -> None:
    path = ctx.path / ".git" / "agent-status"
    path.write_text("working\n")
    os.utime(path, (1_000, 1_000))

    assert status.agent_status(ctx) is None


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


def test_github_checks_lowercases_the_rollup_state(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, "echo SUCCESS")

    assert status.github_checks_status(ctx) == "success"


def test_github_pr_lowercases_the_pr_state(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, "echo MERGED")

    assert status.github_pr_status(ctx) == "merged"


def test_columns_are_sampled_not_refetched(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, f"echo run >> {tmp_path}/calls; echo SUCCESS")
    column = StatusColumn("ci", builtin="github-checks")

    assert status.column_status(ctx, column) == "success"
    assert status.column_status(ctx, column) == "success"

    assert (tmp_path / "calls").read_text().splitlines() == ["run"]


def test_sampling_expires(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, f"echo run >> {tmp_path}/calls; echo SUCCESS")
    column = StatusColumn("ci", builtin="github-checks")
    status.column_status(ctx, column)

    later = time.time() + status.sample_interval(column) + 1
    monkeypatch.setattr(status.time, "time", lambda: later)
    status.column_status(ctx, column)

    assert (tmp_path / "calls").read_text().splitlines() == ["run", "run"]


def test_a_zero_interval_disables_sampling(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, f"echo run >> {tmp_path}/calls; echo SUCCESS")
    column = StatusColumn("ci", builtin="github-checks", interval=0)

    status.column_status(ctx, column)
    status.column_status(ctx, column)

    assert (tmp_path / "calls").read_text().splitlines() == ["run", "run"]


def test_command_columns_can_be_sampled(ctx: contexts.Context, tmp_path: Path) -> None:
    command = f"echo run >> {tmp_path}/calls; echo hi"
    column = StatusColumn("mine", command=command, interval=60)

    assert status.column_status(ctx, column) == "hi"
    assert status.column_status(ctx, column) == "hi"

    assert (tmp_path / "calls").read_text().splitlines() == ["run"]


def test_github_pr_without_a_pr_is_empty(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, "exit 0")

    assert status.github_pr_status(ctx) is None


def test_github_checks_swallows_gh_failures(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, "exit 1")

    assert status.github_checks_status(ctx) is None


def test_github_checks_without_a_pr_is_empty(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # gh prints nothing when the branch has no open PR (jq's `// empty`).
    fake_gh(tmp_path, monkeypatch, "exit 0")

    assert status.github_checks_status(ctx) is None


def test_column_status_dispatches_on_the_column_kind(
    ctx: contexts.Context, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, "echo FAILURE")
    (ctx.path / ".git" / "agent-status").write_text("idle\n")

    assert status.column_status(ctx, StatusColumn("c", command="echo hi")) == "hi"
    assert status.column_status(ctx, StatusColumn("a", builtin="agent")) == "idle"
    assert status.column_status(ctx, StatusColumn("g", builtin="github-checks")) == "failure"


def test_status_cells_hold_git_state_and_column_output(cfg: Config, ctx: contexts.Context) -> None:
    cfg = replace(
        cfg,
        status=(StatusColumn("claude", builtin="agent"), StatusColumn("ci", command="false")),
    )
    (ctx.path / ".git" / "agent-status").write_text("working\n")
    (ctx.path / "scratch.txt").write_text("x\n")

    assert status.status_cells(cfg, ctx) == ["*", "working", ""]


def test_status_cells_without_columns_report_git_state(cfg: Config, ctx: contexts.Context) -> None:
    assert status.status_cells(cfg, ctx) == [""]
