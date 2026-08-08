import asyncio
import subprocess
import time
from collections.abc import Coroutine
from pathlib import Path
from typing import Any

import pytest
from conftest import add_repo, create_context
from textual.widgets import Button

from ctx import contexts
from ctx.config import Config, StatusColumn
from ctx.contexts import Context
from ctx.multiplexer import Multiplexer
from ctx.tui import AlertScreen, CtxTui


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
    add_repo(cfg, str(origin))
    return origin


def run_async(coro: Coroutine[Any, Any, None]) -> None:
    asyncio.run(coro)


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
    add_repo(cfg, str(origin))
    for name in ("one", "two"):
        create_context(cfg, "origin", name)
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
    add_repo(cfg, str(origin))
    for name in ("one", "two"):
        create_context(cfg, "origin", name)
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


def test_quit_does_not_wait_for_a_running_create(
    cfg: Config, origin: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Quitting must cancel an in-flight create, not wait out its fetch."""
    add_repo(cfg, str(origin))

    async def stuck_create(*args: object, **kwargs: object) -> Context:
        await asyncio.sleep(30)
        raise AssertionError("cancelled create kept running")

    monkeypatch.setattr(contexts, "create_context", stuck_create)
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.press("n")
            await pilot.press("x")
            await pilot.press("enter")
            await pilot.pause()
            await pilot.press("q")

    start = time.perf_counter()
    run_async(drive())

    assert time.perf_counter() - start < 5, "quit waited for the create"


def test_alerts_show_bracketed_error_text_verbatim(cfg: Config, origin: Path) -> None:
    """Errors often quote a git command; its brackets must not parse as markup."""
    add_repo(cfg, str(origin))
    app = CtxTui(cfg, StubMultiplexer())
    failure = subprocess.CalledProcessError(
        128, ["git", "-c", "http.lowSpeedLimit=1000", "fetch", "origin"]
    )
    message = str(failure)

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            app.push_screen(AlertScreen(message))
            await pilot.pause()
            assert isinstance(app.screen, AlertScreen)

    run_async(drive())


def test_reload_keeps_the_ui_responsive(cfg: Config, origin: Path) -> None:
    """A slow status provider must not stall the event loop."""
    cfg = _slow_status(cfg)
    add_repo(cfg, str(origin))
    for name in ("one", "two"):
        create_context(cfg, "origin", name)
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
