import asyncio
import contextlib
import os
import subprocess
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from functools import partial
from typing import ClassVar

from rich.text import Text
from textual import on, work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.coordinate import Coordinate
from textual.css.query import NoMatches
from textual.screen import ModalScreen
from textual.widgets import Button, DataTable, Footer, Input, Label
from textual.widgets.data_table import CellDoesNotExist

from ctx import contexts, repos, status
from ctx.config import Config
from ctx.contexts import Context
from ctx.multiplexer import Multiplexer, MultiplexerError


@dataclass(frozen=True)
class OpenRequest:
    name: str


@dataclass(frozen=True)
class NewRequest:
    repo: str
    name: str
    base: str | None


Request = OpenRequest | NewRequest

_SPINNER_FRAMES = "|/-\\"

_STATUS_POLL_SECONDS = 2.0

# The panels' tables differ only in cell type; navigation code takes any.
_AnyTable = DataTable[str] | DataTable[str | Text]


def _styled(cell: str) -> Text | str:
    """Colour a status cell if its value is a well-known status word."""
    style = status.STATUS_STYLES.get(cell)
    return Text(cell, style=style) if style else cell


_MUTATING_ACTIONS = frozenset(
    {
        "open",
        "new",
        "new_from_base",
        "add_repo",
        "set_default_repo",
        "archive",
        "delete",
        "unarchive",
        "empty_archive",
    }
)


@contextlib.contextmanager
def _silenced_stderr() -> Iterator[None]:
    """Keep subprocess stderr (git progress) from writing over the TUI."""
    saved = os.dup(2)
    devnull = os.open(os.devnull, os.O_WRONLY)
    try:
        os.dup2(devnull, 2)
        yield
    finally:
        os.dup2(saved, 2)
        os.close(saved)
        os.close(devnull)


# One row of buttons, so either axis moves along it.
_BUTTON_NAV_BINDINGS: tuple[Binding, ...] = (
    Binding("j,l,down,right", "app.focus_next", show=False),
    Binding("k,h,up,left", "app.focus_previous", show=False),
)

# Arrow keys mirror h / l. A row cursor makes the tables' own left / right
# (cell cursor movement) invisible, so the keys are free for panel switching.
_PANEL_NAV_BINDINGS: tuple[Binding, ...] = (
    Binding("left", "app.prev_panel", show=False),
    Binding("right", "app.next_panel", show=False),
)


class PromptScreen(ModalScreen[str | None]):
    """Lazygit-style single-line prompt: enter submits, escape cancels."""

    BINDINGS: ClassVar = [Binding("escape", "cancel", show=False)]

    def __init__(self, title: str, placeholder: str = "") -> None:
        super().__init__()
        self._title = title
        self._placeholder = placeholder

    def compose(self) -> ComposeResult:
        with Vertical(id="dialog"):
            yield Label(self._title, markup=False)
            yield Input(placeholder=self._placeholder)

    @on(Input.Submitted)
    def _submit(self, event: Input.Submitted) -> None:
        value = event.value.strip()
        if value:
            self.dismiss(value)

    def action_cancel(self) -> None:
        self.dismiss(None)


class AlertScreen(ModalScreen[None]):
    """Message popup dismissed with escape or enter."""

    BINDINGS: ClassVar = [
        Binding("escape", "close", show=False),
        Binding("enter", "close", show=False),
    ]

    def __init__(self, message: str) -> None:
        super().__init__()
        self._message = message

    def compose(self) -> ComposeResult:
        with Vertical(id="dialog"):
            # Errors quote whatever failed (git argv, paths); bracketed text
            # must render verbatim, not parse as markup.
            yield Label(self._message, markup=False)

    def action_close(self) -> None:
        self.dismiss(None)


_PANEL_KEYBINDINGS: dict[str, tuple[tuple[str, str], ...]] = {
    "contexts": (
        ("enter / space / o", "open context"),
        ("n", "new context"),
        ("N", "new context from a base branch"),
        ("a", "archive context"),
        ("d", "delete context"),
    ),
    "repos": (
        ("enter / n", "new context"),
        ("N", "new context from a base branch"),
        ("a", "add repo"),
        ("s", "set / clear default repo"),
        ("d", "remove repo"),
    ),
    "archived": (
        ("enter", "unarchive and open context"),
        ("u", "unarchive context"),
        ("n", "new context"),
        ("N", "new context from a base branch"),
        ("d", "permanently delete context"),
        ("e", "empty the archive"),
    ),
}

_COMMON_KEYBINDINGS = (
    ("j / k / ↓ / ↑", "move within panel"),
    ("g / G", "jump to top / bottom"),
    ("h / l / ← / →", "switch panel"),
    ("1 / 2 / 3", "jump to panel"),
    ("r", "refresh"),
    ("?", "this help"),
    ("q / ctrl+c", "quit"),
)


class HelpScreen(ModalScreen[None]):
    """Keybinding reference for one panel, dismissed like an alert."""

    BINDINGS: ClassVar = [
        Binding("escape", "close", show=False),
        Binding("enter", "close", show=False),
        Binding("question_mark", "close", show=False),
    ]

    def __init__(self, panel: str) -> None:
        super().__init__()
        self._panel = panel

    def compose(self) -> ComposeResult:
        bindings = _PANEL_KEYBINDINGS[self._panel] + _COMMON_KEYBINDINGS
        with Vertical(id="dialog"):
            yield Label(f"Keybindings ({self._panel})")
            yield Label("\n".join(f"{key:<16}{desc}" for key, desc in bindings))

    def action_close(self) -> None:
        self.dismiss(None)


class ConfirmScreen(ModalScreen[bool]):
    """Yes/no confirmation with a message."""

    BINDINGS: ClassVar = [
        Binding("escape", "cancel", show=False),
        *_BUTTON_NAV_BINDINGS,
    ]

    def __init__(self, message: str, confirm_label: str) -> None:
        super().__init__()
        self._message = message
        self._confirm_label = confirm_label

    def compose(self) -> ComposeResult:
        with Vertical(id="dialog"):
            yield Label(self._message, markup=False)
            with Horizontal(id="buttons"):
                yield Button(self._confirm_label, variant="error", id="confirm")
                yield Button("Cancel", id="cancel")

    @on(Button.Pressed, "#confirm")
    def _confirm(self) -> None:
        self.dismiss(True)

    @on(Button.Pressed, "#cancel")
    def _cancel(self) -> None:
        self.dismiss(False)

    def action_cancel(self) -> None:
        self.dismiss(False)


class ContextsTable(DataTable[str | Text]):
    """Contexts panel; its bindings surface in the footer while focused."""

    BINDINGS: ClassVar = [
        ("o", "app.open", "Open"),
        Binding("space", "app.open", show=False),
        ("a", "app.archive", "Archive"),
        ("d", "app.delete", "Delete"),
        *_PANEL_NAV_BINDINGS,
    ]


class ReposTable(DataTable[str]):
    """Repos panel; its bindings surface in the footer while focused."""

    BINDINGS: ClassVar = [
        ("a", "app.add_repo", "Add repo"),
        ("s", "app.set_default_repo", "Set default"),
        ("d", "app.delete", "Remove repo"),
        *_PANEL_NAV_BINDINGS,
    ]


class ArchivedTable(DataTable[str]):
    """Archived panel; its bindings surface in the footer while focused."""

    BINDINGS: ClassVar = [
        ("u", "app.unarchive", "Unarchive"),
        ("d", "app.delete", "Delete"),
        ("e", "app.empty_archive", "Empty"),
        *_PANEL_NAV_BINDINGS,
    ]


class CtxTui(App[Request | None]):
    """Interactive manager for contexts and repos, lazygit-style."""

    # No command palette: it exposes theme switching and search we don't want.
    ENABLE_COMMAND_PALETTE: ClassVar[bool] = False

    CSS = """
    #contexts { height: 7fr; }
    #bottom { height: 3fr; }
    #repos { width: 1fr; height: 100%; }
    #archived { width: 1fr; height: 100%; }
    #contexts, #repos, #archived {
        border: round $foreground;
        /* A row cursor offers no horizontal scrolling, so wide content
           just clips; a scrollbar would only take up a row. */
        overflow-x: hidden;
        scrollbar-size-vertical: 1;
        scrollbar-background: ansi_default;
        scrollbar-color: ansi_bright_black;
        scrollbar-color-hover: ansi_white;
        scrollbar-color-active: ansi_white;
    }
    #contexts:focus, #repos:focus, #archived:focus {
        border: round $primary;
    }
    #contexts.busy, #repos.busy, #archived.busy {
        text-style: dim;
    }
    /* Textual's ansi default is white-on-bright-blue, unreadable in many
       terminal palettes; stick to the terminal's own colours. */
    #contexts > .datatable--header, #repos > .datatable--header, #archived > .datatable--header {
        background: ansi_default;
        color: ansi_default;
        text-style: bold;
    }
    #dialog {
        padding: 1 2;
        width: 60;
        height: auto;
        border: round $foreground;
        background: $background;
        align-horizontal: center;
    }
    #dialog Label { margin-bottom: 1; width: 100%; }
    #buttons { height: auto; align-horizontal: right; }
    #buttons Button { margin-left: 2; }
    PromptScreen, ConfirmScreen, AlertScreen, HelpScreen { align: center middle; }
    """

    BINDINGS: ClassVar = [
        Binding("1", "focus_contexts", show=False),
        Binding("2", "focus_repos", show=False),
        Binding("3", "focus_archived", show=False),
        ("n", "new", "New context"),
        Binding("N", "new_from_base", show=False),
        ("r", "refresh", "Refresh"),
        ("q", "quit", "Quit"),
        Binding("question_mark", "help", "Help", key_display="?"),
        Binding("ctrl+c", "quit", show=False, priority=True),
        Binding("h", "prev_panel", show=False),
        Binding("l", "next_panel", show=False),
        Binding("j", "cursor_down", show=False),
        Binding("k", "cursor_up", show=False),
        Binding("g", "cursor_top", show=False),
        Binding("G", "cursor_bottom", show=False),
    ]

    def __init__(self, cfg: Config, mux: Multiplexer, exit_on_open: bool = False) -> None:
        # Render with the terminal's own ANSI palette rather than a
        # truecolor theme.
        super().__init__(ansi_color=True)
        self.theme = "ansi-dark"
        self._cfg = cfg
        self._mux = mux
        self._exit_on_open = exit_on_open
        self._busy: set[str] = set()
        self._spinner_frame = 0
        self._fetching: set[int] = set()

    def compose(self) -> ComposeResult:
        # Contexts gets the full width: it grows status columns. Panels
        # appear in reading order matching the 1/2/3 keys and the h/l ring.
        with Vertical():
            # Renderable priority keeps status colours visible under the
            # cursor; the cursor still paints its background.
            yield ContextsTable(
                id="contexts", cursor_type="row", cursor_foreground_priority="renderable"
            )
            with Horizontal(id="bottom"):
                yield ReposTable(id="repos", cursor_type="row")
                yield ArchivedTable(id="archived", cursor_type="row")
        yield Footer()

    def on_mount(self) -> None:
        ctx_table = self._contexts_table
        status_names = tuple(s.name.upper() for s in self._cfg.status)
        columns = ctx_table.add_columns("NAME", "REPO", "BRANCH", "STATUS", *status_names)
        self._status_columns = columns[3:]
        repo_table = self._repos_table
        repo_table.add_columns("NAME", "URL")
        self._archived_table.add_columns("NAME", "REPO", "BRANCH")
        self._update_titles()
        self._reload()
        ctx_table.focus()
        self.set_interval(0.1, self._spin)
        if self._cfg.status:
            for index, interval in enumerate(self._poll_intervals()):
                self.set_interval(interval, partial(self._poll_column, index))

    @property
    def _contexts_table(self) -> DataTable[str | Text]:
        return self.query_one("#contexts", DataTable)

    @property
    def _repos_table(self) -> DataTable[str]:
        return self.query_one("#repos", DataTable)

    @property
    def _archived_table(self) -> DataTable[str]:
        return self.query_one("#archived", DataTable)

    def _reload(self) -> None:
        """Repaint the panels from what is cheap to read; statuses fill in after.

        A status provider may take seconds per context (the GitHub built-ins
        shell out to `gh`), which is more than the panels can wait for and far
        more than the interface can stop responding for.
        """
        blanks = [""] * (1 + len(self._cfg.status))
        ctx_table = self._contexts_table
        ctx_table.clear()
        ctxs = contexts.list_contexts(self._cfg)
        # Pin the attached context on top: recency is keyed on git activity,
        # so a busy background session often outranks the one being viewed.
        current = next((c for c in ctxs if self._mux.is_current(c)), None)
        if current is not None:
            ctxs.remove(current)
            ctxs.insert(0, current)
        for ctx in ctxs:
            name = Text(ctx.name, style="green") if ctx is current else ctx.name
            ctx_table.add_row(name, ctx.repo, contexts.current_branch(ctx), *blanks, key=ctx.name)
        # Land the cursor on the most recent other context: the common reason
        # to open the TUI is switching away, not reopening the same session.
        if current is not None and ctx_table.row_count > 1:
            ctx_table.move_cursor(row=1)
        repo_table = self._repos_table
        repo_table.clear()
        default = repos.default_repo(self._cfg)
        for name in sorted(repos.repo_names(self._cfg), key=lambda name: (name != default, name)):
            label = f"{name} *" if name == default else name
            repo_table.add_row(label, repos.repo_url(self._cfg, name), key=name)
        archived_table = self._archived_table
        archived_table.clear()
        for ctx in contexts.list_archived(self._cfg):
            archived_table.add_row(ctx.name, ctx.repo, contexts.current_branch(ctx), key=ctx.name)
        self._refresh_statuses()

    def _poll_intervals(self) -> list[float]:
        """Each status column's poll cadence: its provider's refresh interval.

        The STATUS column and columns without an interval ride the base tick;
        nothing polls faster than it.
        """
        return [_STATUS_POLL_SECONDS] + [
            max(status.refresh_interval(col), _STATUS_POLL_SECONDS) for col in self._cfg.status
        ]

    def _poll_column(self, index: int) -> None:
        """Keep one status column live without a full (cursor-resetting) reload."""
        if not self._busy:
            self._refresh_column(index)

    def _refresh_statuses(self) -> None:
        for index in range(1 + len(self._cfg.status)):
            self._refresh_column(index)

    def _refresh_column(self, index: int) -> None:
        if index in self._fetching:
            return
        self._fetching.add(index)
        self._fetch_column_worker(index)

    @work
    async def _fetch_column_worker(self, index: int) -> None:
        """Fetch one column's cells concurrently, painting each as it lands."""
        try:
            fetches = (self._fetch_cell(ctx, index) for ctx in contexts.list_contexts(self._cfg))
            await asyncio.gather(*fetches)
        finally:
            self._fetching.discard(index)

    async def _fetch_cell(self, ctx: Context, index: int) -> None:
        if index == 0:
            cell = await status.git_state(ctx)
        else:
            cell = await status.column_status(ctx, self._cfg.status[index - 1]) or ""
        # The context may have been deleted or archived since the fetch
        # started, and the table itself is gone when the app is closing.
        with contextlib.suppress(CellDoesNotExist, NoMatches):
            self._contexts_table.update_cell(
                ctx.name, self._status_columns[index], _styled(cell), update_width=True
            )

    def _spin(self) -> None:
        if self._busy:
            self._spinner_frame += 1
            self._update_titles()

    def _update_titles(self) -> None:
        frame = _SPINNER_FRAMES[self._spinner_frame % len(_SPINNER_FRAMES)]
        titles = (
            ("contexts", "[1] Contexts"),
            ("repos", "[2] Repos"),
            ("archived", "[3] Archived"),
        )
        for table_id, title in titles:
            busy = table_id in self._busy
            table = self.query_one(f"#{table_id}", DataTable)
            table.border_title = title + (f" {frame}" if busy else "")
            table.set_class(busy, "busy")

    def _start_busy(self, table_id: str) -> None:
        self._busy.add(table_id)
        self._update_titles()

    def _finish_busy(self) -> None:
        self._busy.clear()
        self._update_titles()

    def _active_table(self) -> _AnyTable:
        for table in (self._repos_table, self._archived_table):
            if self.focused is table:
                return table
        return self._contexts_table

    def _selected_key(self, table: _AnyTable) -> str | None:
        if table.row_count == 0:
            return None
        return table.coordinate_to_cell_key(Coordinate(table.cursor_row, 0)).row_key.value

    def _selected_context(self) -> Context | None:
        """Resolve the cursor's context from disk; reloads and yields None if stale."""
        key = self._selected_key(self._contexts_table)
        if key is None:
            return None
        try:
            return contexts.find_context(self._cfg, key)
        except LookupError:
            # The row went stale, e.g. the context was deleted externally.
            self._reload()
            return None

    def _selected_archived(self) -> Context | None:
        """Resolve the archived panel's cursor from disk; reloads and yields None if stale."""
        key = self._selected_key(self._archived_table)
        if key is None:
            return None
        try:
            return contexts.find_archived(self._cfg, key)
        except LookupError:
            self._reload()
            return None

    def check_action(self, action: str, parameters: tuple[object, ...]) -> bool | None:
        """Centrally disable mutating actions while a worker runs or a popup is open."""
        if action in _MUTATING_ACTIONS and (self._busy or isinstance(self.screen, ModalScreen)):
            return None
        return True

    @on(DataTable.RowSelected, "#contexts")
    def _context_selected(self) -> None:
        # Enter on the table bypasses key bindings, so apply the same policy.
        if self.check_action("open", ()):
            self.action_open()

    @on(DataTable.RowSelected, "#repos")
    def _repo_selected(self) -> None:
        if self.check_action("new", ()):
            self.action_new()

    @on(DataTable.RowSelected, "#archived")
    def _archived_selected(self) -> None:
        if self.check_action("unarchive", ()):
            self._open_archived()

    def action_focus_contexts(self) -> None:
        self._contexts_table.focus()

    def action_focus_repos(self) -> None:
        self._repos_table.focus()

    def action_focus_archived(self) -> None:
        self._archived_table.focus()

    def _cycle_panel(self, step: int) -> None:
        tables: list[_AnyTable] = [self._contexts_table, self._repos_table, self._archived_table]
        current = tables.index(self._active_table())
        tables[(current + step) % len(tables)].focus()

    def action_next_panel(self) -> None:
        self._cycle_panel(1)

    def action_prev_panel(self) -> None:
        self._cycle_panel(-1)

    def action_open(self) -> None:
        ctx = self._selected_context()
        if ctx is None:
            return
        if not self._mux.can_open_in_place():
            self.exit(OpenRequest(ctx.name))
            return
        try:
            with _silenced_stderr():
                self._mux.open(ctx)
        except (MultiplexerError, subprocess.CalledProcessError) as exc:
            self.push_screen(AlertScreen(str(exc)))
            return
        if self._exit_on_open:
            self.exit()

    def _repo_for_new(self) -> str | None:
        """The target repo: the hovered repo on the repos panel, else the default.

        With no default set, fall back to the hovered row's repo.
        """
        active = self._active_table()
        if active is self._repos_table:
            return self._selected_key(self._repos_table)
        default = repos.default_repo(self._cfg)
        if default is not None:
            return default
        if active is self._contexts_table:
            ctx = self._selected_context()
            if ctx is not None:
                return ctx.repo
        else:
            ctx = self._selected_archived()
            if ctx is not None:
                return ctx.repo
        return self._selected_key(self._repos_table)

    def action_new(self) -> None:
        repo = self._repo_for_new()
        if repo is None:
            self.push_screen(AlertScreen("no repos registered; press a to add one"))
            return

        def named(name: str | None) -> None:
            if name:
                self._create(repo, name, None)

        self.push_screen(PromptScreen(f"New context for {repo}", "name"), named)

    def action_new_from_base(self) -> None:
        repo = self._repo_for_new()
        if repo is None:
            self.push_screen(AlertScreen("no repos registered; press a to add one"))
            return

        def named(name: str | None) -> None:
            if not name:
                return

            def based(base: str | None) -> None:
                if base:
                    self._create(repo, name, base)

            self.push_screen(PromptScreen(f"Base branch for {name}", "branch"), based)

        self.push_screen(PromptScreen(f"New context for {repo}", "name"), named)

    def _create(self, repo: str, name: str, base: str | None) -> None:
        """Create in the background if we can stay running, else exit to the CLI."""
        if not self._mux.can_open_in_place():
            self.exit(NewRequest(repo, name, base))
            return
        self._start_busy("contexts")
        self._create_worker(repo, name, base)

    @work
    async def _create_worker(self, repo: str, name: str, base: str | None) -> None:
        try:
            ctx = await contexts.create_context(self._cfg, repo, name, base)
        except (
            ValueError,
            FileExistsError,
            FileNotFoundError,
            subprocess.CalledProcessError,
        ) as exc:
            self._finish_busy()
            self.push_screen(AlertScreen(str(exc)))
            return
        self._reload()
        self._finish_busy()
        try:
            with _silenced_stderr():
                self._mux.open(ctx)
        except (MultiplexerError, subprocess.CalledProcessError) as exc:
            self.push_screen(AlertScreen(str(exc)))
            return
        if self._exit_on_open:
            self.exit()

    def action_set_default_repo(self) -> None:
        """Toggle the selected repo as the default for new contexts."""
        if self._active_table() is not self._repos_table:
            return
        name = self._selected_key(self._repos_table)
        if name is None:
            return
        current = repos.default_repo(self._cfg)
        repos.set_default_repo(self._cfg, None if name == current else name)
        self._reload()

    def action_add_repo(self) -> None:
        def submitted(url: str | None) -> None:
            if url:
                self._start_busy("repos")
                self._add_repo_worker(url)

        self.push_screen(PromptScreen("Add repo", "clone URL"), submitted)

    @work
    async def _add_repo_worker(self, url: str) -> None:
        try:
            await repos.add_repo(self._cfg, url)
        except (FileExistsError, subprocess.CalledProcessError) as exc:
            self.push_screen(AlertScreen(str(exc)))
        self._reload()
        self._finish_busy()

    def action_unarchive(self) -> None:
        if self._active_table() is not self._archived_table:
            return
        ctx = self._selected_archived()
        if ctx is None:
            return
        self._start_busy("archived")
        self._unarchive_worker(ctx, open_after=False)

    def _open_archived(self) -> None:
        """Enter on an archived context: unarchive it and open its session."""
        ctx = self._selected_archived()
        if ctx is None:
            return
        if not self._mux.can_open_in_place():
            try:
                contexts.unarchive_context(self._cfg, ctx)
            except OSError as exc:
                self.push_screen(AlertScreen(str(exc)))
                return
            self.exit(OpenRequest(ctx.name))
            return
        self._start_busy("archived")
        self._unarchive_worker(ctx, open_after=True)

    @work(thread=True)
    def _unarchive_worker(self, ctx: Context, open_after: bool) -> None:
        try:
            restored = contexts.unarchive_context(self._cfg, ctx)
        except (OSError, subprocess.CalledProcessError) as exc:
            self.call_from_thread(self._finish_busy)
            self.call_from_thread(self.push_screen, AlertScreen(str(exc)))
            return
        self.call_from_thread(self._reload)
        self.call_from_thread(self._finish_busy)
        if not open_after:
            return
        try:
            with _silenced_stderr():
                self._mux.open(restored)
        except (MultiplexerError, subprocess.CalledProcessError) as exc:
            self.call_from_thread(self.push_screen, AlertScreen(str(exc)))
            return
        if self._exit_on_open:
            self.call_from_thread(self.exit)

    def action_empty_archive(self) -> None:
        if self._active_table() is not self._archived_table:
            return
        archived = contexts.list_archived(self._cfg)
        if not archived:
            return

        def confirmed(empty: bool | None) -> None:
            if empty:
                self._start_busy("archived")
                self._empty_archive_worker()

        message = f"Permanently delete all {len(archived)} archived context(s)?"
        self.push_screen(ConfirmScreen(message, "Empty"), confirmed)

    @work(thread=True)
    def _empty_archive_worker(self) -> None:
        try:
            contexts.empty_archive(self._cfg)
        except OSError as exc:
            self.call_from_thread(self.push_screen, AlertScreen(str(exc)))
        self.call_from_thread(self._reload)
        self.call_from_thread(self._finish_busy)

    def action_delete(self) -> None:
        active = self._active_table()
        if active is self._repos_table:
            self._delete_repo()
        elif active is self._archived_table:
            self._delete_archived()
        else:
            self._delete_context()

    def _delete_archived(self) -> None:
        ctx = self._selected_archived()
        if ctx is None:
            return

        def confirmed(delete: bool | None) -> None:
            if delete:
                self._start_busy("archived")
                self._delete_archived_worker(ctx)

        message = f"Permanently delete {ctx.qualified}?"
        self.push_screen(ConfirmScreen(message, "Delete"), confirmed)

    @work(thread=True)
    def _delete_archived_worker(self, ctx: Context) -> None:
        try:
            contexts.remove_context(ctx)
        except OSError as exc:
            self.call_from_thread(self.push_screen, AlertScreen(str(exc)))
        self.call_from_thread(self._reload)
        self.call_from_thread(self._finish_busy)

    def action_archive(self) -> None:
        """Archive the selected context straight away; it is cheap to undo."""
        if self._active_table() is not self._contexts_table:
            return
        ctx = self._selected_context()
        if ctx is None:
            return
        self._start_busy("contexts")
        self._archive_worker(ctx)

    def _delete_context(self) -> None:
        ctx = self._selected_context()
        if ctx is None:
            return
        problems = []
        if contexts.is_dirty(ctx):
            problems.append("uncommitted changes")
        if contexts.unpushed_commits(ctx):
            problems.append("unpushed commits")
        if problems:
            message = f"{ctx.qualified} has {' and '.join(problems)}. Delete anyway?"
            label = "Force delete"
        else:
            message = f"Delete {ctx.qualified}?"
            label = "Delete"

        def confirmed(delete: bool | None) -> None:
            if delete:
                self._start_busy("contexts")
                self._delete_context_worker(ctx)

        self.push_screen(ConfirmScreen(message, label), confirmed)

    @work(thread=True)
    def _archive_worker(self, ctx: Context) -> None:
        self._teardown(ctx, lambda: contexts.archive_context(self._cfg, ctx))

    @work(thread=True)
    def _delete_context_worker(self, ctx: Context) -> None:
        self._teardown(ctx, lambda: contexts.remove_context(ctx))

    def _teardown(self, ctx: Context, remove: Callable[[], object]) -> None:
        try:
            if self._mux.exists(ctx) and self._mux.is_current(ctx):
                # Killing our own session takes the TUI (and the client) down
                # with it, so land the client elsewhere and kill last.
                self._switch_away(ctx)
                remove()
                self._mux.kill(ctx)
                return
            if self._mux.exists(ctx):
                self._mux.kill(ctx)
            remove()
        except (OSError, subprocess.CalledProcessError) as exc:
            self.call_from_thread(self.push_screen, AlertScreen(str(exc)))
        self.call_from_thread(self._reload)
        self.call_from_thread(self._finish_busy)

    def _switch_away(self, ctx: Context) -> None:
        """Re-point the client at the most recent other running session."""
        for other in contexts.list_contexts(self._cfg):
            if other.name == ctx.name or not self._mux.exists(other):
                continue
            with contextlib.suppress(MultiplexerError, subprocess.CalledProcessError):
                with _silenced_stderr():
                    self._mux.open(other)
                return

    def _delete_repo(self) -> None:
        name = self._selected_key(self._repos_table)
        if name is None:
            return

        def confirmed(delete: bool | None) -> None:
            if delete:
                self._start_busy("repos")
                self._delete_repo_worker(name)

        message = f"Remove repo '{name}'? Its contexts are left alone."
        self.push_screen(ConfirmScreen(message, "Remove"), confirmed)

    @work(thread=True)
    def _delete_repo_worker(self, name: str) -> None:
        try:
            repos.remove_repo(self._cfg, name)
        except (OSError, subprocess.CalledProcessError) as exc:
            self.call_from_thread(self.push_screen, AlertScreen(str(exc)))
        self.call_from_thread(self._reload)
        self.call_from_thread(self._finish_busy)

    def action_refresh(self) -> None:
        self._reload()

    def action_help(self) -> None:
        self.push_screen(HelpScreen(self._active_table().id or "contexts"))

    def action_cursor_down(self) -> None:
        table = self._active_table()
        table.move_cursor(row=table.cursor_row + 1)

    def action_cursor_up(self) -> None:
        table = self._active_table()
        table.move_cursor(row=table.cursor_row - 1)

    def action_cursor_top(self) -> None:
        self._active_table().move_cursor(row=0)

    def action_cursor_bottom(self) -> None:
        table = self._active_table()
        table.move_cursor(row=table.row_count - 1)
