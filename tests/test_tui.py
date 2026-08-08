import asyncio
import time
from collections.abc import Coroutine
from pathlib import Path
from typing import Any

import pytest

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
