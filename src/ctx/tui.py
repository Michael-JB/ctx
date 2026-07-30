import contextlib
import os
import subprocess
from collections.abc import Iterator
from dataclasses import dataclass
from typing import ClassVar

from textual import on, work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.coordinate import Coordinate
from textual.screen import ModalScreen
from textual.widgets import Button, DataTable, Footer, Input, Label

from ctx import contexts, repos
from ctx.config import Config
from ctx.contexts import Context
from ctx.multiplexer import Multiplexer


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


class PromptScreen(ModalScreen[str | None]):
    """Lazygit-style single-line prompt: enter submits, escape cancels."""

    BINDINGS: ClassVar = [Binding("escape", "cancel", show=False)]

    def __init__(self, title: str, placeholder: str = "") -> None:
        super().__init__()
        self._title = title
        self._placeholder = placeholder

    def compose(self) -> ComposeResult:
        with Vertical(id="dialog"):
            yield Label(self._title)
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
            yield Label(self._message)

    def action_close(self) -> None:
        self.dismiss(None)


_KEYBINDINGS = """\
j / k        move within panel
g / G        jump to top / bottom
h / l        switch panel (also 1 / 2)
enter        open context (also space / o)
n            new context
N            new context from a base branch
a            add repo
d            delete context or repo
r            refresh
?            this help
q / ctrl+c   quit\
"""


class HelpScreen(ModalScreen[None]):
    """Keybinding reference, dismissed like an alert."""

    BINDINGS: ClassVar = [
        Binding("escape", "close", show=False),
        Binding("enter", "close", show=False),
        Binding("question_mark", "close", show=False),
    ]

    def compose(self) -> ComposeResult:
        with Vertical(id="dialog"):
            yield Label("Keybindings")
            yield Label(_KEYBINDINGS)

    def action_close(self) -> None:
        self.dismiss(None)


class ConfirmScreen(ModalScreen[bool]):
    """Yes/no confirmation with a message."""

    BINDINGS: ClassVar = [
        Binding("escape", "cancel", show=False),
        Binding("j", "app.focus_next", show=False),
        Binding("k", "app.focus_previous", show=False),
        Binding("l", "app.focus_next", show=False),
        Binding("h", "app.focus_previous", show=False),
    ]

    def __init__(self, message: str, confirm_label: str) -> None:
        super().__init__()
        self._message = message
        self._confirm_label = confirm_label

    def compose(self) -> ComposeResult:
        with Vertical(id="dialog"):
            yield Label(self._message)
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


class CtxTui(App[Request | None]):
    """Interactive manager for contexts and repos, lazygit-style."""

    # No command palette: it exposes theme switching and search we don't want.
    ENABLE_COMMAND_PALETTE: ClassVar[bool] = False

    CSS = """
    #contexts { height: 2fr; }
    #repos { height: 1fr; }
    #contexts, #repos {
        border: round $foreground;
    }
    #contexts:focus, #repos:focus {
        border: round $primary;
    }
    #dialog {
        padding: 1 2;
        width: 60;
        height: auto;
        border: round $foreground;
        background: $background;
        align-horizontal: center;
    }
    #dialog Label { margin-bottom: 1; }
    #buttons { height: auto; align-horizontal: right; }
    #buttons Button { margin-left: 2; }
    PromptScreen, ConfirmScreen, AlertScreen, HelpScreen { align: center middle; }
    """

    BINDINGS: ClassVar = [
        Binding("1", "focus_contexts", show=False),
        Binding("2", "focus_repos", show=False),
        ("o", "open", "Open"),
        Binding("space", "open", show=False),
        ("n", "new", "New context"),
        Binding("N", "new_from_base", show=False),
        ("a", "add_repo", "Add repo"),
        ("d", "delete", "Delete"),
        ("r", "refresh", "Refresh"),
        ("q", "quit", "Quit"),
        Binding("question_mark", "help", "Help", key_display="?"),
        Binding("ctrl+c", "quit", show=False, priority=True),
        Binding("h", "switch_panel", show=False),
        Binding("l", "switch_panel", show=False),
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

    def compose(self) -> ComposeResult:
        yield DataTable(id="contexts", cursor_type="row")
        yield DataTable(id="repos", cursor_type="row")
        yield Footer()

    def on_mount(self) -> None:
        ctx_table = self._contexts_table
        ctx_table.add_columns("NAME", "REPO", "BRANCH", "STATUS")
        repo_table = self._repos_table
        repo_table.add_columns("NAME", "URL")
        self._update_titles()
        self._reload()
        ctx_table.focus()
        self.set_interval(0.1, self._spin)

    @property
    def _contexts_table(self) -> DataTable[str]:
        return self.query_one("#contexts", DataTable)

    @property
    def _repos_table(self) -> DataTable[str]:
        return self.query_one("#repos", DataTable)

    def _reload(self) -> None:
        ctx_table = self._contexts_table
        ctx_table.clear()
        for ctx in contexts.list_contexts(self._cfg):
            status = []
            if contexts.is_dirty(ctx):
                status.append("uncommitted changes")
            unpushed = contexts.unpushed_commits(ctx)
            if unpushed:
                status.append(f"{len(unpushed)} unpushed commit(s)")
            ctx_table.add_row(
                ctx.name,
                ctx.repo,
                contexts.current_branch(ctx),
                ", ".join(status) or "clean",
                key=ctx.name,
            )
        repo_table = self._repos_table
        repo_table.clear()
        for name in repos.repo_names(self._cfg):
            repo_table.add_row(name, repos.repo_url(self._cfg, name), key=name)

    def _spin(self) -> None:
        if self._busy:
            self._spinner_frame += 1
            self._update_titles()

    def _update_titles(self) -> None:
        frame = _SPINNER_FRAMES[self._spinner_frame % len(_SPINNER_FRAMES)]
        for table_id, title in (("contexts", "[1] Contexts"), ("repos", "[2] Repos")):
            suffix = f" {frame}" if table_id in self._busy else ""
            self.query_one(f"#{table_id}", DataTable).border_title = title + suffix

    def _start_busy(self, table_id: str) -> None:
        self._busy.add(table_id)
        self._update_titles()

    def _finish_busy(self) -> None:
        self._busy.clear()
        self._update_titles()

    def _active_table(self) -> DataTable[str]:
        if self.focused is self._repos_table:
            return self._repos_table
        return self._contexts_table

    def _selected_key(self, table: DataTable[str]) -> str | None:
        if table.row_count == 0:
            return None
        return table.coordinate_to_cell_key(Coordinate(table.cursor_row, 0)).row_key.value

    @on(DataTable.RowSelected, "#contexts")
    def _context_selected(self, event: DataTable.RowSelected) -> None:
        key = event.row_key.value
        if key:
            self._open(key)

    def _open(self, name: str) -> None:
        """Switch to the context's session in place if possible, else exit to attach."""
        if not self._mux.can_open_in_place():
            self.exit(OpenRequest(name))
            return
        with _silenced_stderr():
            self._mux.open(contexts.find_context(self._cfg, name))
        if self._exit_on_open:
            self.exit()

    @on(DataTable.RowSelected, "#repos")
    def _repo_selected(self, event: DataTable.RowSelected) -> None:
        self.action_new()

    def action_focus_contexts(self) -> None:
        self._contexts_table.focus()

    def action_focus_repos(self) -> None:
        self._repos_table.focus()

    def action_switch_panel(self) -> None:
        if self._active_table() is self._contexts_table:
            self._repos_table.focus()
        else:
            self._contexts_table.focus()

    def action_open(self) -> None:
        key = self._selected_key(self._contexts_table)
        if key:
            self._open(key)

    def _repo_for_new(self) -> str | None:
        """The repo the selection points at: a selected repo, or a context's repo."""
        if self._active_table() is self._contexts_table:
            key = self._selected_key(self._contexts_table)
            if key is not None:
                return contexts.find_context(self._cfg, key).repo
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

    @work(thread=True)
    def _create_worker(self, repo: str, name: str, base: str | None) -> None:
        try:
            with _silenced_stderr():
                ctx = contexts.create_context(self._cfg, repo, name, base)
        except (FileExistsError, FileNotFoundError, subprocess.CalledProcessError) as exc:
            self.call_from_thread(self._finish_busy)
            self.call_from_thread(self.push_screen, AlertScreen(str(exc)))
            return
        self.call_from_thread(self._reload)
        self.call_from_thread(self._finish_busy)
        with _silenced_stderr():
            self._mux.open(ctx)
        if self._exit_on_open:
            self.call_from_thread(self.exit)

    def action_add_repo(self) -> None:
        def submitted(url: str | None) -> None:
            if url:
                self._start_busy("repos")
                self._add_repo_worker(url)

        self.push_screen(PromptScreen("Add repo", "clone URL"), submitted)

    @work(thread=True)
    def _add_repo_worker(self, url: str) -> None:
        try:
            with _silenced_stderr():
                repos.add_repo(self._cfg, url)
        except (FileExistsError, subprocess.CalledProcessError) as exc:
            self.call_from_thread(self.push_screen, AlertScreen(str(exc)))
        self.call_from_thread(self._reload)
        self.call_from_thread(self._finish_busy)

    def action_delete(self) -> None:
        if self._active_table() is self._repos_table:
            self._delete_repo()
        else:
            self._delete_context()

    def _delete_context(self) -> None:
        key = self._selected_key(self._contexts_table)
        if key is None:
            return
        ctx = contexts.find_context(self._cfg, key)
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
    def _delete_context_worker(self, ctx: Context) -> None:
        try:
            if self._mux.exists(ctx):
                self._mux.kill(ctx)
            contexts.remove_context(ctx)
        except (OSError, subprocess.CalledProcessError) as exc:
            self.call_from_thread(self.push_screen, AlertScreen(str(exc)))
        self.call_from_thread(self._reload)
        self.call_from_thread(self._finish_busy)

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
        self.push_screen(HelpScreen())

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
