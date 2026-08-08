"""Run the TUI headlessly and quit while a status poll is in flight.

A helper for test_tui, run as a subprocess: quitting waits for worker
threads, and only a real process exit shows whether that wait ends. Prints
`exited` and returns 0 if the app got all the way out.
"""

import sys
from pathlib import Path

from ctx.config import Config, StatusColumn
from ctx.contexts import Context
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


def main() -> int:
    root = Path(sys.argv[1])
    cfg = Config(
        contexts_dir=root / "contexts",
        repos_dir=root / "repos",
        archive_dir=root / "archive",
        # Stands in for the GitHub built-ins: seconds of work per context.
        status=(StatusColumn("slow", command="sleep 30; echo hi"),),
    )
    QuittingTui(cfg, Stub()).run(headless=True)
    print("exited")
    return 0


if __name__ == "__main__":
    sys.exit(main())
