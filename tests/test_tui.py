import asyncio
import subprocess
import time
from collections.abc import Callable, Coroutine, Mapping
from pathlib import Path
from typing import Any

import pytest
from conftest import MakeOrigin, add_repo, create_context, fake_cli, fake_gh
from textual.coordinate import Coordinate
from textual.widgets import Button, Input

from ctx import contexts, repos
from ctx.config import Config, StatusColumn
from ctx.contexts import Context
from ctx.multiplexer import Multiplexer
from ctx.tui import AlertScreen, ConfirmScreen, CtxTui, PromptScreen


class StubMultiplexer(Multiplexer):
    def can_open_in_place(self) -> bool:
        return True

    def exists(self, ctx: Context) -> bool:
        return False

    def is_current(self, ctx: Context) -> bool:
        return False

    def create(self, ctx: Context, values: Mapping[str, str] | None = None) -> None:
        pass

    def open(self, ctx: Context, values: Mapping[str, str] | None = None) -> None:
        pass

    def kill(self, ctx: Context) -> None:
        pass


class RecordingMultiplexer(StubMultiplexer):
    def __init__(self, current: str | None = None) -> None:
        self._current = current
        self.calls: list[tuple[str, str]] = []
        self.path_present_at_kill: bool | None = None

    def exists(self, ctx: Context) -> bool:
        return True

    def is_current(self, ctx: Context) -> bool:
        return ctx.name == self._current

    def open(self, ctx: Context, values: Mapping[str, str] | None = None) -> None:
        self.calls.append(("open", ctx.name))

    def kill(self, ctx: Context) -> None:
        self.path_present_at_kill = ctx.path.exists()
        self.calls.append(("kill", ctx.name))


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

            app._repos_table.focus()
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


def _run_worker(app: CtxTui, start: Callable[[], None]) -> None:
    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            start()
            await app.workers.wait_for_complete()

    run_async(drive())


def test_archiving_another_context_does_not_switch(cfg: Config, registered: Path) -> None:
    for name in ("one", "two"):
        create_context(cfg, "origin", name)
    ctx = contexts.find_context(cfg, "one")
    mux = RecordingMultiplexer(current="two")
    app = CtxTui(cfg, mux)

    _run_worker(app, lambda: app._archive_worker(ctx))

    assert mux.calls == [("kill", "one")]
    with pytest.raises(LookupError):
        contexts.find_context(cfg, "one")


def test_theme_colours_reach_the_stylesheet_variables(cfg: Config) -> None:
    from dataclasses import replace

    from ctx.config import Theme

    themed = replace(cfg, theme=Theme(selection="#2d3f76"))
    app = CtxTui(themed, StubMultiplexer())

    variables = app.get_css_variables()

    assert variables["ctx-selection"] == "#2d3f76"
    assert variables["ctx-foreground"] == "ansi_default"


def test_current_context_is_pinned_and_cursor_starts_below_it(
    cfg: Config, registered: Path
) -> None:
    for name in ("one", "two"):
        create_context(cfg, "origin", name)
    app = CtxTui(cfg, RecordingMultiplexer(current="one"))

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            table = app._contexts_table
            first = table.coordinate_to_cell_key(Coordinate(0, 0)).row_key.value
            assert first == "one", "the attached context must be the top row"
            assert table.cursor_row == 1, "the cursor must start on the next context"

    run_async(drive())


def test_cursor_starts_on_top_without_a_current_context(cfg: Config, registered: Path) -> None:
    for name in ("one", "two"):
        create_context(cfg, "origin", name)
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            assert app._contexts_table.cursor_row == 0

    run_async(drive())


def test_new_prompt_prefills_a_generated_name(
    cfg: Config, registered: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(contexts, "random_name", lambda cfg: "holy-tiger")
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            await pilot.press("n")
            prompt_input = app.screen.query_one(Input)
            assert prompt_input.value == "holy-tiger"
            await pilot.press("enter")
            await app.workers.wait_for_complete()

    run_async(drive())

    assert contexts.find_context(cfg, "holy-tiger")


def test_typing_replaces_the_prefilled_name(
    cfg: Config, registered: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(contexts, "random_name", lambda cfg: "holy-tiger")
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            await pilot.press("n")
            await pilot.press("x")
            assert app.screen.query_one(Input).value == "x"

    run_async(drive())


def test_new_context_uses_the_default_repo_off_the_repos_panel(
    cfg: Config, registered: Path, make_origin: MakeOrigin
) -> None:
    add_repo(cfg, str(make_origin("other")))
    create_context(cfg, "origin", "one")
    repos.set_default_repo(cfg, "other")
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            assert app._repo_for_new() == "other", "contexts panel must use the default"
            app._repos_table.focus()
            await pilot.pause()
            await pilot.press("j")
            assert app._repo_for_new() == "origin", "repos panel must use the hovered repo"

    run_async(drive())


def test_default_repo_sorts_first(cfg: Config, registered: Path, make_origin: MakeOrigin) -> None:
    add_repo(cfg, str(make_origin("aaa")))
    repos.set_default_repo(cfg, "origin")
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            assert app._selected_key(app._repos_table) == "origin", "default must be the top row"

    run_async(drive())


def test_s_toggles_the_default_repo(cfg: Config, registered: Path) -> None:
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            app._repos_table.focus()
            await pilot.press("s")
            assert repos.default_repo(cfg) == "origin"
            await pilot.press("s")
            assert repos.default_repo(cfg) is None

    run_async(drive())


def test_o_opens_the_pr_in_the_browser(
    cfg: Config, registered: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ctx = create_context(cfg, "origin", "one")
    log = tmp_path / "gh-args"
    fake_gh(tmp_path, monkeypatch, f'echo "$@" > {log}')
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            await pilot.press("o")
            await app.workers.wait_for_complete()

    run_async(drive())

    assert log.read_text().strip() == "pr view --web"
    assert contexts.find_context(cfg, "one").path == ctx.path


def test_o_uses_the_forge_from_the_remote(
    cfg: Config, registered: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ctx = create_context(cfg, "origin", "one")
    subprocess.run(
        ["git", "remote", "set-url", "origin", "git@gitlab.com:jane/tool.git"],
        cwd=ctx.path,
        check=True,
    )
    log = tmp_path / "glab-args"
    fake_cli(tmp_path, monkeypatch, "glab", f'echo "$@" > {log}')
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            await pilot.press("o")
            await app.workers.wait_for_complete()

    run_async(drive())

    assert log.read_text().strip() == "mr view --web"


def test_archive_key_archives_without_a_prompt(cfg: Config, registered: Path) -> None:
    create_context(cfg, "origin", "one")
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            await pilot.press("d")
            await app.workers.wait_for_complete()

    run_async(drive())

    assert contexts.find_archived(cfg, "one")


def test_delete_key_asks_for_confirmation(cfg: Config, registered: Path) -> None:
    ctx = create_context(cfg, "origin", "one")
    contexts.archive_context(cfg, ctx)
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            app._archived_table.focus()
            await pilot.press("d")
            await pilot.pause()
            assert isinstance(app.screen, ConfirmScreen)
            await pilot.press("escape")
            await app.workers.wait_for_complete()

    run_async(drive())

    assert contexts.find_archived(cfg, "one")


def test_shift_delete_key_on_contexts_asks_for_confirmation(cfg: Config, registered: Path) -> None:
    create_context(cfg, "origin", "one")
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            await pilot.press("D")
            await pilot.pause()
            assert isinstance(app.screen, ConfirmScreen)
            await pilot.press("escape")
            await app.workers.wait_for_complete()

    run_async(drive())

    assert contexts.find_context(cfg, "one")


def test_delete_context_worker_removes_the_checkout(cfg: Config, registered: Path) -> None:
    ctx = create_context(cfg, "origin", "one")
    app = CtxTui(cfg, StubMultiplexer())

    _run_worker(app, lambda: app._delete_context_worker(ctx))

    assert not ctx.path.exists()
    with pytest.raises(LookupError):
        contexts.find_context(cfg, "one")


def test_add_repo_key_is_local_to_the_repos_panel(cfg: Config, registered: Path) -> None:
    """`a` opens the add-repo prompt only while the repos panel is focused."""
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            app._repos_table.focus()
            await pilot.press("a")
            await pilot.pause()
            assert isinstance(app.screen, PromptScreen)

    run_async(drive())


def test_archiving_the_current_context_switches_away_and_kills_last(
    cfg: Config, registered: Path
) -> None:
    for name in ("one", "two"):
        create_context(cfg, "origin", name)
    ctx = contexts.find_context(cfg, "one")
    mux = RecordingMultiplexer(current="one")
    app = CtxTui(cfg, mux)

    _run_worker(app, lambda: app._archive_worker(ctx))

    assert mux.calls == [("open", "two"), ("kill", "one")]
    assert mux.path_present_at_kill is False, "the move must finish before the kill"
    assert contexts.find_archived(cfg, "one")


def test_archiving_the_current_context_leaves_no_stale_busy_state(
    cfg: Config, registered: Path
) -> None:
    """A TUI in a tmux popup outlives its session's kill; it must repaint."""
    for name in ("one", "two"):
        create_context(cfg, "origin", name)
    ctx = contexts.find_context(cfg, "one")
    app = CtxTui(cfg, RecordingMultiplexer(current="one"))

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            app._start_busy("contexts")
            app._archive_worker(ctx)
            await app.workers.wait_for_complete()
            await pilot.pause()
            assert not app._busy, "the panel stayed dimmed after the archive"
            assert app._contexts_table.row_count == 1

    run_async(drive())


def test_slash_filters_and_enter_opens_the_match(cfg: Config, registered: Path) -> None:
    for name in ("alpha", "beta"):
        create_context(cfg, "origin", name)
    mux = RecordingMultiplexer()
    app = CtxTui(cfg, mux)

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            await pilot.press("slash", "b", "t")
            assert app._contexts_table.row_count == 1, "only the fuzzy match may remain"
            await pilot.press("enter")
            await pilot.pause()
            assert ("open", "beta") in mux.calls
            assert app._contexts_table.row_count == 2, "the filter must clear after opening"

    run_async(drive())


def test_escape_clears_the_filter(cfg: Config, registered: Path) -> None:
    for name in ("alpha", "beta"):
        create_context(cfg, "origin", name)
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            await pilot.press("slash", "b")
            assert app._contexts_table.row_count == 1
            await pilot.press("escape")
            assert app._contexts_table.row_count == 2
            assert app.focused is app._contexts_table

    run_async(drive())


def test_enter_with_no_matches_keeps_filtering(cfg: Config, registered: Path) -> None:
    create_context(cfg, "origin", "alpha")
    mux = RecordingMultiplexer()
    app = CtxTui(cfg, mux)

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            await pilot.press("slash", "z")
            assert app._contexts_table.row_count == 0
            await pilot.press("enter")
            assert mux.calls == []
            assert app._contexts_table.row_count == 0, "the filter must stay active"

    run_async(drive())


def test_filter_matches_the_repo_too(
    cfg: Config, registered: Path, make_origin: MakeOrigin
) -> None:
    add_repo(cfg, str(make_origin("other")))
    create_context(cfg, "origin", "alpha")
    create_context(cfg, "other", "beta")
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            await pilot.press("slash", "o", "t", "h")
            table = app._contexts_table
            assert table.row_count == 1
            assert app._selected_key(table) == "beta"

    run_async(drive())


def test_filter_is_panel_scoped(cfg: Config, registered: Path, make_origin: MakeOrigin) -> None:
    add_repo(cfg, str(make_origin("other")))
    create_context(cfg, "origin", "alpha")
    app = CtxTui(cfg, StubMultiplexer())

    async def drive() -> None:
        async with app.run_test() as pilot:
            await pilot.pause()
            app._repos_table.focus()
            await pilot.press("slash", "x")
            assert app._repos_table.row_count == 0
            assert app._contexts_table.row_count == 1, "other panels must keep their rows"
            await pilot.press("escape")
            assert app._repos_table.row_count == 2
            assert app.focused is app._repos_table

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
