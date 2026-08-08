import asyncio
import subprocess
import sys
import time
from collections.abc import Callable, Coroutine
from pathlib import Path
from typing import Any

import pytest
from textual.widgets import Button

from ctx import contexts, repos
from ctx.config import Config, StatusColumn
from ctx.contexts import Context
from ctx.multiplexer import Multiplexer
from ctx.tui import CtxTui


class StubMultiplexer(Multiplexer):
    def can_open_in_place(self) -> bool:
        return True

    def exists(self, ctx: Context) -> bool:
        return False

    def open(self, ctx: Context) -> None:
        pass

    def kill(self, ctx: Context) -> None:
        pass


@pytest.fixture
def registered(cfg: Config, origin: Path) -> Path:
    repos.add_repo(cfg, str(origin))
    return origin


def run_async(coro: Coroutine[Any, Any, None]) -> None:
    asyncio.run(coro)


def test_quitting_does_not_wait_for_a_status_poll(cfg: Config, origin: Path) -> None:
    """Quitting joins the worker threads, so a poll must abandon itself.

    Run for real: `run_test` never reaches the executor shutdown that does
    the joining, which is exactly where the app used to wedge.
    """
    repos.add_repo(cfg, str(origin))
    for name in ("one", "two", "three", "four"):
        contexts.create_context(cfg, "origin", name)
    probe = Path(__file__).parent / "quit_probe.py"

    started = time.perf_counter()
    proc = subprocess.run(
        [sys.executable, str(probe), str(cfg.contexts_dir.parent)],
        capture_output=True,
        text=True,
        timeout=60,
    )
    took = time.perf_counter() - started

    assert proc.returncode == 0, proc.stderr
    assert "exited" in proc.stdout
    # Sampling four contexts costs >= 8s (2s cap each); quitting must not
    # sit through it.
    assert took < 7, f"quit took {took:.1f}s"


def test_quitting_does_not_wait_for_a_git_call(cfg: Config, origin: Path) -> None:
    """A stalled fetch must not hold the process open once you quit."""
    repos.add_repo(cfg, str(origin))
    contexts.create_context(cfg, "origin", "one")

    took, proc = _run_quit_probe(cfg, "git")

    assert proc.returncode == 0, proc.stderr
    assert "exited" in proc.stdout
    # The git call stalls for 30s; quitting must not sit through it.
    assert took < 10, f"quit took {took:.1f}s"


def _run_quit_probe(cfg: Config, mode: str) -> tuple[float, subprocess.CompletedProcess[str]]:
    probe = Path(__file__).parent / "quit_probe.py"
    started = time.perf_counter()
    proc = subprocess.run(
        [sys.executable, str(probe), str(cfg.contexts_dir.parent), mode],
        capture_output=True,
        text=True,
        timeout=60,
    )
    return time.perf_counter() - started, proc


def test_a_cheap_column_updates_while_a_slow_one_samples(cfg: Config, origin: Path) -> None:
    """Reading a file must not queue behind a provider that shells out."""
    cfg = Config(
        contexts_dir=cfg.contexts_dir,
        repos_dir=cfg.repos_dir,
        archive_dir=cfg.archive_dir,
        status=(
            StatusColumn("claude", builtin="agent"),
            StatusColumn("slow", command="sleep 1.5; echo hi"),
        ),
    )
    repos.add_repo(cfg, str(origin))
    for name in ("one", "two", "three"):
        ctx = contexts.create_context(cfg, "origin", name)
        (ctx.path / ".git" / "agent-status").write_text("idle\n")
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await _wait_until(pilot, lambda: _agent_cells(app) == ["idle"] * 3, timeout=30)

            for ctx in contexts.list_contexts(cfg):
                (ctx.path / ".git" / "agent-status").write_text("working\n")
            started = time.perf_counter()
            await _wait_until(pilot, lambda: _agent_cells(app) == ["working"] * 3, timeout=30)
            took = time.perf_counter() - started

            # Sampling the slow column costs 4.5s a sweep; the agent column
            # must keep to its own cadence regardless.
            assert took < 4, f"the agent column took {took:.1f}s to update"

    run_async(drive())


async def _wait_until(pilot: Any, condition: Callable[[], bool], timeout: float) -> None:
    deadline = time.perf_counter() + timeout
    while time.perf_counter() < deadline and not condition():
        await pilot.pause(0.02)
    assert condition(), "condition never held"


def _agent_cells(app: CtxTui) -> list[str]:
    table = app._contexts_table
    column = app._status_columns[1]
    return sorted(str(table.get_cell(row.value, column)) for row in table.rows)


def _slow_status(cfg: Config) -> Config:
    return Config(
        contexts_dir=cfg.contexts_dir,
        repos_dir=cfg.repos_dir,
        archive_dir=cfg.archive_dir,
        status=(StatusColumn("slow", command="sleep 0.5; echo hi"),),
    )


def test_panels_are_populated_before_the_statuses_are(cfg: Config, origin: Path) -> None:
    """Rows must be there to act on straight away, slow providers or not."""
    cfg = _slow_status(cfg)
    repos.add_repo(cfg, str(origin))
    for name in ("one", "two"):
        contexts.create_context(cfg, "origin", name)
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()

            assert app._contexts_table.row_count == 2
            assert app._repos_table.row_count == 1
            assert _slow_cells(app) == ["", ""]

            deadline = time.perf_counter() + 8
            while time.perf_counter() < deadline and _slow_cells(app) != ["hi", "hi"]:
                await pilot.pause(0.05)

            assert _slow_cells(app) == ["hi", "hi"], "statuses never filled in"

    run_async(drive())


def test_arrow_keys_navigate_like_the_vim_keys(cfg: Config, origin: Path) -> None:
    repos.add_repo(cfg, str(origin))
    for name in ("one", "two"):
        contexts.create_context(cfg, "origin", name)
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()

            await pilot.press("down")
            assert app._contexts_table.cursor_row == 1
            await pilot.press("up")
            assert app._contexts_table.cursor_row == 0

            await pilot.press("right")
            assert app.focused is app._repos_table
            await pilot.press("right")
            assert app.focused is app._archived_table
            await pilot.press("left")
            assert app.focused is app._repos_table

            app._contexts_table.focus()
            await pilot.press("d")
            await pilot.pause()
            first = _focused_button_index(app)
            await pilot.press("right")
            assert _focused_button_index(app) == first + 1
            await pilot.press("up")
            assert _focused_button_index(app) == first

    run_async(drive())


def _focused_button_index(app: CtxTui) -> int:
    focused = app.focused
    assert isinstance(focused, Button)
    return list(app.screen.query(Button)).index(focused)


def _slow_cells(app: CtxTui) -> list[str]:
    table = app._contexts_table
    column = app._status_columns[-1]
    return [str(table.get_cell(row.value, column)) for row in table.rows]


def test_reload_keeps_the_ui_responsive(cfg: Config, origin: Path) -> None:
    """A slow status provider must not stall the event loop."""
    cfg = _slow_status(cfg)
    repos.add_repo(cfg, str(origin))
    for name in ("one", "two"):
        contexts.create_context(cfg, "origin", name)
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            worst = 0.0
            deadline = time.perf_counter() + 8
            while time.perf_counter() < deadline and _slow_cells(app) != ["hi", "hi"]:
                start = time.perf_counter()
                await pilot.pause(0.01)
                worst = max(worst, time.perf_counter() - start)
            assert _slow_cells(app) == ["hi", "hi"], "statuses never arrived"
            # Sampling both contexts takes >= 1s; doing it on the event loop
            # is the freeze this guards against.
            assert worst < 0.4, f"event loop stalled for {worst:.2f}s"

    run_async(drive())
