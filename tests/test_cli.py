from pathlib import Path

import pytest
from click.testing import CliRunner
from conftest import commit_file

from ctx import contexts, repos
from ctx.cli import Deps, cli
from ctx.config import Config
from ctx.contexts import Context
from ctx.multiplexer import Multiplexer


class SpyMultiplexer(Multiplexer):
    """Spy double: canned exists() answers plus a record of open/kill calls."""

    def __init__(self) -> None:
        self.running: set[str] = set()
        self.opened: list[str] = []
        self.killed: list[str] = []

    def can_open_in_place(self) -> bool:
        return True

    def exists(self, ctx: Context) -> bool:
        return ctx.qualified in self.running

    def open(self, ctx: Context) -> None:
        self.opened.append(ctx.qualified)

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
    repos.add_repo(cfg, str(origin))
    return origin


def test_help(runner: CliRunner) -> None:
    result = runner.invoke(cli, ["--help"])

    assert result.exit_code == 0


def test_version(runner: CliRunner) -> None:
    result = runner.invoke(cli, ["--version"])

    assert result.exit_code == 0


def test_new_reports_the_created_context(runner: CliRunner, deps: Deps, registered: Path) -> None:
    result = runner.invoke(cli, ["new", "origin", "feat"], obj=deps)

    assert result.exit_code == 0
    assert "created origin/feat" in result.output


def test_new_opens_a_session(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    runner.invoke(cli, ["new", "origin", "feat"], obj=deps)

    assert mux.opened == ["origin/feat"]


def test_new_rejects_an_unregistered_repo(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["new", "nope", "feat"], obj=deps)

    assert result.exit_code == 1
    assert "not registered" in result.stderr


def test_open_opens_the_context_session(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    contexts.create_context(deps.cfg, "origin", "feat")

    result = runner.invoke(cli, ["open", "feat"], obj=deps)

    assert result.exit_code == 0
    assert mux.opened == ["origin/feat"]


def test_open_rejects_an_unknown_context(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["open", "feat"], obj=deps)

    assert result.exit_code == 1
    assert "no context 'feat'" in result.stderr


def test_list_without_contexts(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["list"], obj=deps)

    assert result.output == "no contexts\n"


def test_list_shows_each_context(runner: CliRunner, deps: Deps, registered: Path) -> None:
    contexts.create_context(deps.cfg, "origin", "feat")

    result = runner.invoke(cli, ["list"], obj=deps)

    header, row = result.output.splitlines()
    assert header.split() == ["NAME", "REPO", "BRANCH", "STATUS"]
    assert row.split() == ["feat", "origin", "feat", "clean"]


def test_list_marks_dirty_contexts(runner: CliRunner, deps: Deps, registered: Path) -> None:
    ctx = contexts.create_context(deps.cfg, "origin", "feat")
    (ctx.path / "scratch.txt").write_text("x\n")

    result = runner.invoke(cli, ["list"], obj=deps)

    assert "uncommitted changes" in result.output


def test_rm_deletes_the_checkout(runner: CliRunner, deps: Deps, registered: Path) -> None:
    ctx = contexts.create_context(deps.cfg, "origin", "feat")

    result = runner.invoke(cli, ["rm", "feat"], obj=deps)

    assert result.exit_code == 0
    assert "removed origin/feat" in result.output
    assert not ctx.path.exists()


def test_rm_kills_a_running_session(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    contexts.create_context(deps.cfg, "origin", "feat")
    mux.running.add("origin/feat")

    runner.invoke(cli, ["rm", "feat"], obj=deps)

    assert mux.killed == ["origin/feat"]


def test_rm_refuses_unpushed_work(runner: CliRunner, deps: Deps, registered: Path) -> None:
    ctx = contexts.create_context(deps.cfg, "origin", "feat")
    commit_file(ctx.path, "work.txt")

    result = runner.invoke(cli, ["rm", "feat"], obj=deps)

    assert result.exit_code == 1
    assert "unpushed commit" in result.stderr
    assert ctx.path.exists()


def test_rm_force_overrides_the_guard(runner: CliRunner, deps: Deps, registered: Path) -> None:
    ctx = contexts.create_context(deps.cfg, "origin", "feat")
    commit_file(ctx.path, "work.txt")

    result = runner.invoke(cli, ["rm", "--force", "feat"], obj=deps)

    assert result.exit_code == 0
    assert not ctx.path.exists()


def test_rm_archived_deletes_the_archived_checkout(
    runner: CliRunner, deps: Deps, registered: Path
) -> None:
    ctx = contexts.create_context(deps.cfg, "origin", "feat")
    archived = contexts.archive_context(deps.cfg, ctx)

    result = runner.invoke(cli, ["rm", "--archived", "feat"], obj=deps)

    assert result.exit_code == 0
    assert not archived.path.exists()


def test_rm_archived_leaves_a_live_namesake_alone(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    contexts.archive_context(deps.cfg, contexts.create_context(deps.cfg, "origin", "feat"))
    live = contexts.create_context(deps.cfg, "origin", "feat")
    mux.running.add("origin/feat")

    result = runner.invoke(cli, ["rm", "--archived", "feat"], obj=deps)

    assert result.exit_code == 0
    assert mux.killed == []
    assert live.path.exists()


def test_rm_archived_refuses_unpushed_work(runner: CliRunner, deps: Deps, registered: Path) -> None:
    ctx = contexts.create_context(deps.cfg, "origin", "feat")
    commit_file(ctx.path, "work.txt")
    contexts.archive_context(deps.cfg, ctx)

    result = runner.invoke(cli, ["rm", "--archived", "feat"], obj=deps)

    assert result.exit_code == 1
    assert "unpushed commit" in result.stderr


def test_rm_rejects_an_unknown_context(runner: CliRunner, deps: Deps) -> None:
    result = runner.invoke(cli, ["rm", "feat"], obj=deps)

    assert result.exit_code == 1
    assert "no context 'feat'" in result.stderr


def test_archive_moves_the_context_and_kills_its_session(
    runner: CliRunner, deps: Deps, mux: SpyMultiplexer, registered: Path
) -> None:
    ctx = contexts.create_context(deps.cfg, "origin", "feat")
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
    contexts.archive_context(deps.cfg, contexts.create_context(deps.cfg, "origin", "cold"))
    contexts.create_context(deps.cfg, "origin", "hot")

    result = runner.invoke(cli, ["list", "--archived"], obj=deps)

    header, row = result.output.splitlines()
    assert header.split() == ["NAME", "REPO", "BRANCH", "STATUS"]
    assert row.split()[:2] == ["cold", "origin"]


def test_archive_empty_deletes_all_archived_contexts(
    runner: CliRunner, deps: Deps, registered: Path
) -> None:
    contexts.archive_context(deps.cfg, contexts.create_context(deps.cfg, "origin", "cold"))
    kept = contexts.create_context(deps.cfg, "origin", "hot")

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
    contexts.archive_context(deps.cfg, contexts.create_context(deps.cfg, "origin", "feat"))

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
