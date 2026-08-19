from collections.abc import Mapping
from dataclasses import replace
from pathlib import Path

import pytest
from click.testing import CliRunner
from conftest import add_repo, commit_file, create_context

from ctx import contexts
from ctx.cli import Deps, cli
from ctx.config import Config, StatusColumn
from ctx.contexts import Context
from ctx.layout import Pane
from ctx.multiplexer import Multiplexer


class SpyMultiplexer(Multiplexer):
    """Spy double: canned exists() answers plus a record of open/kill calls."""

    def __init__(self) -> None:
        self.running: set[str] = set()
        self.opened: list[str] = []
        self.created: list[str] = []
        self.killed: list[str] = []
        self.values: list[Mapping[str, str] | None] = []

    def can_open_in_place(self) -> bool:
        return True

    def exists(self, ctx: Context) -> bool:
        return ctx.qualified in self.running

    def is_current(self, ctx: Context) -> bool:
        return False

    def create(self, ctx: Context, values: Mapping[str, str] | None = None) -> None:
        self.created.append(ctx.qualified)
        self.values.append(values)

    def open(self, ctx: Context, values: Mapping[str, str] | None = None) -> None:
        self.opened.append(ctx.qualified)
        self.values.append(values)

    def kill(self, ctx: Context) -> None:
        self.killed.append(ctx.qualified)


@pytest.fixture
def mux() -> SpyMultiplexer:
    return SpyMultiplexer()


@pytest.fixture
def deps(cfg: Config, mux: SpyMultiplexer) -> Deps:
    return Deps(cfg, mux)


@pytest.fixture
def runner() -> CliRunner:
    return CliRunner()


@pytest.fixture
def registered(cfg: Config, origin: Path) -> Path:
    add_repo(cfg, str(origin))
    return origin


def test_help(runner: CliRunner) -> None:
    result = runner.invoke(cli, ["--help"])

    assert result.exit_code == 0


def test_version(runner: CliRunner) -> None:
    result = runner.invoke(cli, ["--version"])

    assert result.exit_code == 0


def test_changelog_prints_release_sections(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["changelog"], obj=deps)

    assert result.exit_code == 0
    assert result.output.startswith("# Changelog")
    assert "## [0" in result.output


def test_claude_hook_feeds_the_status_file_from_stdin(
    runner: CliRunner, deps: Deps, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (tmp_path / ".git").mkdir()
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(cli, ["claude-hook"], obj=deps, input='{"hook_event_name": "Stop"}')

    assert result.exit_code == 0
    assert (tmp_path / ".git" / "agent-status").read_text() == "idle\n"


def test_new_reports_the_created_context(runner: CliRunner, deps: Deps, registered: Path) -> None:
    result = runner.invoke(cli, ["new", "origin", "feat"], obj=deps)

    assert result.exit_code == 0
    assert "created origin/feat" in result.output


def test_new_without_a_name_generates_one(
    runner: CliRunner, deps: Deps, monkeypatch: pytest.MonkeyPatch, registered: Path
) -> None:
    monkeypatch.setattr(contexts, "random_name", lambda cfg: "holy-tiger")

    result = runner.invoke(cli, ["new", "origin"], obj=deps)

    assert result.exit_code == 0
    assert "created origin/holy-tiger" in result.output


def test_new_opens_a_session(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    runner.invoke(cli, ["new", "origin", "feat"], obj=deps)

    assert mux.opened == ["origin/feat"]


def test_new_rejects_an_unregistered_repo(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["new", "nope", "feat"], obj=deps)

    assert result.exit_code == 1
    assert "not registered" in result.stderr


def test_new_detach_creates_the_session_without_opening(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    result = runner.invoke(cli, ["new", "origin", "feat", "--detach"], obj=deps)

    assert result.exit_code == 0
    assert mux.created == ["origin/feat"]
    assert mux.opened == []
    assert mux.values == [{}]


def test_new_marks_the_session_as_fresh(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    runner.invoke(cli, ["new", "origin", "feat"], obj=deps)

    assert mux.values == [{}]


def test_open_marks_the_session_as_recreated(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    create_context(deps.cfg, "origin", "feat")

    runner.invoke(cli, ["open", "feat"], obj=deps)

    assert mux.values == [None]


def test_new_set_passes_values_to_the_session(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    deps = Deps(replace(deps.cfg, layout=Pane(builtin="claude")), mux)

    result = runner.invoke(cli, ["new", "origin", "feat", "--set", "prompt=explore x"], obj=deps)

    assert result.exit_code == 0
    assert mux.values == [{"prompt": "explore x"}]


def test_new_set_rejects_a_key_no_builtin_accepts(
    runner: CliRunner, deps: Deps, registered: Path
) -> None:
    result = runner.invoke(cli, ["new", "origin", "feat", "--set", "prompt=x"], obj=deps)

    assert result.exit_code == 1
    assert "no builtin pane in the layout accepts 'prompt'" in result.stderr


def test_new_set_rejects_a_malformed_assignment(
    runner: CliRunner, deps: Deps, registered: Path
) -> None:
    result = runner.invoke(cli, ["new", "origin", "feat", "--set", "prompt"], obj=deps)

    assert result.exit_code == 1
    assert "--set needs KEY=VALUE" in result.stderr


def test_new_set_rejects_a_repeated_key(runner: CliRunner, deps: Deps, registered: Path) -> None:
    deps = Deps(replace(deps.cfg, layout=Pane(builtin="claude")), deps.mux)

    result = runner.invoke(
        cli, ["new", "origin", "feat", "--set", "prompt=a", "--set", "prompt=b"], obj=deps
    )

    assert result.exit_code == 1
    assert "'prompt' twice" in result.stderr


def test_new_rejects_an_invalid_name(runner: CliRunner, deps: Deps, registered: Path) -> None:
    result = runner.invoke(cli, ["new", "origin", "feat~1"], obj=deps)

    assert result.exit_code == 1
    assert "valid branch name" in result.stderr


def test_open_opens_the_context_session(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    create_context(deps.cfg, "origin", "feat")

    result = runner.invoke(cli, ["open", "feat"], obj=deps)

    assert result.exit_code == 0
    assert mux.opened == ["origin/feat"]


def test_open_unarchives_an_archived_context(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    contexts.archive_context(deps.cfg, create_context(deps.cfg, "origin", "feat"))

    result = runner.invoke(cli, ["open", "feat"], obj=deps)

    assert result.exit_code == 0
    assert "unarchived origin/feat" in result.output
    assert mux.opened == ["origin/feat"]
    assert contexts.list_archived(deps.cfg) == []
    assert contexts.find_context(deps.cfg, "feat").path.exists()


def test_open_rejects_an_unknown_context(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["open", "feat"], obj=deps)

    assert result.exit_code == 1
    assert "no context 'feat'" in result.stderr


def test_list_without_contexts(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["list"], obj=deps)

    assert result.output == "no contexts\n"


def test_list_shows_each_context(runner: CliRunner, deps: Deps, registered: Path) -> None:
    create_context(deps.cfg, "origin", "feat")

    result = runner.invoke(cli, ["list"], obj=deps)

    header, row = result.output.splitlines()
    assert header.split() == ["NAME", "REPO", "BRANCH", "STATUS"]
    assert row.split() == ["feat", "origin", "feat"]


def test_list_adds_a_column_per_status_column(
    runner: CliRunner, deps: Deps, registered: Path
) -> None:
    columns = (StatusColumn("claude", command="echo working"),)
    deps = Deps(replace(deps.cfg, status=columns), deps.mux)
    create_context(deps.cfg, "origin", "feat")

    result = runner.invoke(cli, ["list"], obj=deps)

    header, row = result.output.splitlines()
    assert header.split() == ["NAME", "REPO", "BRANCH", "STATUS", "CLAUDE"]
    assert row.split() == ["feat", "origin", "feat", "working"]


def test_list_marks_dirty_contexts(runner: CliRunner, deps: Deps, registered: Path) -> None:
    ctx = create_context(deps.cfg, "origin", "feat")
    (ctx.path / "scratch.txt").write_text("x\n")

    result = runner.invoke(cli, ["list"], obj=deps)

    assert "*" in result.output


def test_rm_deletes_the_checkout(runner: CliRunner, deps: Deps, registered: Path) -> None:
    ctx = create_context(deps.cfg, "origin", "feat")

    result = runner.invoke(cli, ["rm", "feat"], obj=deps)

    assert result.exit_code == 0
    assert "removed origin/feat" in result.output
    assert not ctx.path.exists()


def test_rm_kills_a_running_session(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    create_context(deps.cfg, "origin", "feat")
    mux.running.add("origin/feat")

    runner.invoke(cli, ["rm", "feat"], obj=deps)

    assert mux.killed == ["origin/feat"]


def test_rm_refuses_unpushed_work(runner: CliRunner, deps: Deps, registered: Path) -> None:
    ctx = create_context(deps.cfg, "origin", "feat")
    commit_file(ctx.path, "work.txt")

    result = runner.invoke(cli, ["rm", "feat"], obj=deps)

    assert result.exit_code == 1
    assert "unpushed commit" in result.stderr
    assert ctx.path.exists()


def test_rm_force_overrides_the_guard(runner: CliRunner, deps: Deps, registered: Path) -> None:
    ctx = create_context(deps.cfg, "origin", "feat")
    commit_file(ctx.path, "work.txt")

    result = runner.invoke(cli, ["rm", "--force", "feat"], obj=deps)

    assert result.exit_code == 0
    assert not ctx.path.exists()


def test_rm_archived_deletes_the_archived_checkout(
    runner: CliRunner, deps: Deps, registered: Path
) -> None:
    ctx = create_context(deps.cfg, "origin", "feat")
    archived = contexts.archive_context(deps.cfg, ctx)

    result = runner.invoke(cli, ["rm", "feat"], obj=deps)

    assert result.exit_code == 0
    assert not archived.path.exists()


def test_rm_archived_kills_a_lingering_session(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    contexts.archive_context(deps.cfg, create_context(deps.cfg, "origin", "feat"))
    mux.running.add("origin/feat")

    result = runner.invoke(cli, ["rm", "feat"], obj=deps)

    assert result.exit_code == 0
    assert mux.killed == ["origin/feat"]


def test_rm_archived_refuses_unpushed_work(runner: CliRunner, deps: Deps, registered: Path) -> None:
    ctx = create_context(deps.cfg, "origin", "feat")
    commit_file(ctx.path, "work.txt")
    contexts.archive_context(deps.cfg, ctx)

    result = runner.invoke(cli, ["rm", "feat"], obj=deps)

    assert result.exit_code == 1
    assert "unpushed commit" in result.stderr


def test_rm_rejects_an_unknown_context(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["rm", "feat"], obj=deps)

    assert result.exit_code == 1
    assert "no context 'feat'" in result.stderr


def test_archive_moves_the_context_and_kills_its_session(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    ctx = create_context(deps.cfg, "origin", "feat")
    mux.running.add("origin/feat")

    result = runner.invoke(cli, ["archive", "feat"], obj=deps)

    assert result.exit_code == 0
    assert "archived origin/feat" in result.output
    assert mux.killed == ["origin/feat"]
    assert not ctx.path.exists()
    assert contexts.find_archived(deps.cfg, "feat").path.exists()


def test_archive_rejects_an_unknown_context(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["archive", "feat"], obj=deps)

    assert result.exit_code == 1
    assert "no context 'feat'" in result.stderr


def test_list_archived_without_archived_contexts(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["list", "--archived"], obj=deps)

    assert result.output == "no archived contexts\n"


def test_list_archived_shows_archived_contexts_only(
    runner: CliRunner, deps: Deps, registered: Path
) -> None:
    contexts.archive_context(deps.cfg, create_context(deps.cfg, "origin", "cold"))
    create_context(deps.cfg, "origin", "hot")

    result = runner.invoke(cli, ["list", "--archived"], obj=deps)

    header, row = result.output.splitlines()
    assert header.split() == ["NAME", "REPO", "BRANCH", "STATUS"]
    assert row.split()[:2] == ["cold", "origin"]


def test_archive_empty_deletes_all_archived_contexts(
    runner: CliRunner, deps: Deps, registered: Path
) -> None:
    contexts.archive_context(deps.cfg, create_context(deps.cfg, "origin", "cold"))
    kept = create_context(deps.cfg, "origin", "hot")

    result = runner.invoke(cli, ["archive", "--empty"], obj=deps)

    assert result.exit_code == 0
    assert "emptied archive (1 context(s))" in result.output
    assert contexts.list_archived(deps.cfg) == []
    assert kept.path.exists()


def test_archive_empty_rejects_names(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["archive", "--empty", "feat"], obj=deps)

    assert result.exit_code == 2


def test_unarchive_restores_the_context_without_opening(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    contexts.archive_context(deps.cfg, create_context(deps.cfg, "origin", "feat"))

    result = runner.invoke(cli, ["unarchive", "feat"], obj=deps)

    assert result.exit_code == 0
    assert "unarchived origin/feat" in result.output
    assert mux.opened == []
    assert contexts.find_context(deps.cfg, "feat").path.exists()


def test_unarchive_rejects_an_unknown_context(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["unarchive", "feat"], obj=deps)

    assert result.exit_code == 1
    assert "no archived context 'feat'" in result.stderr


def test_repo_add_registers(runner: CliRunner, deps: Deps, origin: Path) -> None:
    result = runner.invoke(cli, ["repo", "add", str(origin)], obj=deps)

    assert result.exit_code == 0
    assert "registered 'origin'" in result.output


def test_repo_list_shows_name_and_url(runner: CliRunner, deps: Deps, registered: Path) -> None:
    result = runner.invoke(cli, ["repo", "list"], obj=deps)

    assert result.output == f"origin\t{registered}\n"


def test_repo_rm_unregisters(runner: CliRunner, deps: Deps, registered: Path) -> None:
    result = runner.invoke(cli, ["repo", "rm", "origin"], obj=deps)

    assert result.exit_code == 0
    assert runner.invoke(cli, ["repo", "list"], obj=deps).output == ""


def test_repo_rm_rejects_unregistered(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["repo", "rm", "nope"], obj=deps)

    assert result.exit_code == 1
    assert "not registered" in result.stderr


def test_repo_default_sets_and_shows(runner: CliRunner, deps: Deps, registered: Path) -> None:
    result = runner.invoke(cli, ["repo", "default", "origin"], obj=deps)

    assert result.exit_code == 0
    assert runner.invoke(cli, ["repo", "default"], obj=deps).output == "origin\n"


def test_repo_default_clear(runner: CliRunner, deps: Deps, registered: Path) -> None:
    runner.invoke(cli, ["repo", "default", "origin"], obj=deps)

    result = runner.invoke(cli, ["repo", "default", "--clear"], obj=deps)

    assert result.exit_code == 0
    assert runner.invoke(cli, ["repo", "default"], obj=deps).output == "no default repo\n"


def test_repo_default_rejects_unregistered(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["repo", "default", "nope"], obj=deps)

    assert result.exit_code == 1
    assert "not registered" in result.stderr
