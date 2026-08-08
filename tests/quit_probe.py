"""Run the TUI headlessly and quit with work in flight.

A helper for test_tui, run as a subprocess: quitting waits for worker
threads, and only a real process exit shows whether that wait ends. Prints
`exited` and returns 0 if the app got all the way out.

Usage: quit_probe.py <root> [poll|git]
"""

import sys
from pathlib import Path

from textual import work

from ctx.config import Config, StatusColumn
from ctx.contexts import Context
from ctx.git import git
from ctx.multiplexer import Multiplexer
from ctx.tui import CtxTui

QUIT_AFTER_SECONDS = 3.0


class Stub(Multiplexer):
    def can_open_in_place(self) -> bool:
        return True

    def exists(self, ctx: Context) -> bool:
        return False

    def open(self, ctx: Context) -> None:
        pass

    def kill(self, ctx: Context) -> None:
        pass


class QuittingTui(CtxTui):
    def on_mount(self) -> None:
        super().on_mount()
        self.set_timer(QUIT_AFTER_SECONDS, self.exit)


class GitQuittingTui(QuittingTui):
    """Quits with a git call in flight, as archiving or creating would be."""

    def __init__(self, cfg: Config, mux: Multiplexer, checkout: Path) -> None:
        super().__init__(cfg, mux)
        self._checkout = checkout

    def on_mount(self) -> None:
        super().on_mount()
        self._stalling_worker()

    @work(thread=True)
    def _stalling_worker(self) -> None:
        git("-c", "alias.stall=!sleep 30", "stall", cwd=self._checkout, interruptible=True)


def main() -> int:
    root = Path(sys.argv[1])
    mode = sys.argv[2] if len(sys.argv) > 2 else "poll"
    cfg = Config(
        contexts_dir=root / "contexts",
        repos_dir=root / "repos",
        archive_dir=root / "archive",
        # Stands in for the GitHub built-ins: seconds of work per context.
        status=(StatusColumn("slow", command="sleep 30; echo hi"),),
    )
    app: QuittingTui
    if mode == "git":
        checkout = next((cfg.contexts_dir / "origin").iterdir())
        app = GitQuittingTui(cfg, Stub(), checkout)
    else:
        app = QuittingTui(cfg, Stub())
    app.run(headless=True)
    print("exited")
    return 0


if __name__ == "__main__":
    sys.exit(main())
