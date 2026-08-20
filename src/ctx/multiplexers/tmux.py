import os
import subprocess
from collections.abc import Mapping
from pathlib import Path

from ctx.contexts import Context
from ctx.layout import Node, Pane, SplitDirection, resolve_layout
from ctx.multiplexer import Multiplexer, MultiplexerError


def _session_name(ctx: Context) -> str:
    raw = f"{ctx.repo}--{ctx.name}"
    # tmux forbids '.' and ':' in session names.
    return raw.replace(".", "-").replace(":", "-")


def _tmux(*args: str) -> str:
    result = subprocess.run(["tmux", *args], check=True, capture_output=True, text=True)
    return result.stdout.strip()


def _first_pane(node: Node) -> Pane:
    """The leaf a split hands its original region to."""
    while not isinstance(node, Pane):
        node = node.panes[0]
    return node


def _build(node: Node, pane_id: str, cwd: Path) -> list[tuple[str, Pane]]:
    """Subdivide pane_id according to the layout, returning (pane_id, pane) leaves."""
    if isinstance(node, Pane):
        return [(pane_id, node)]
    match node.direction:
        case SplitDirection.ROW:
            flag = "-h"
        case SplitDirection.COLUMN:
            flag = "-v"
    regions = [pane_id]
    for child in node.panes[1:]:
        args = ["split-window", flag, "-t", regions[-1], "-c", str(cwd), "-P", "-F", "#{pane_id}"]
        command = _first_pane(child).command
        if command is not None:
            args.append(command)
        regions.append(_tmux(*args))
    leaves: list[tuple[str, Pane]] = []
    for child, region in zip(node.panes, regions, strict=True):
        leaves.extend(_build(child, region, cwd))
    return leaves


def _create_session(session: str, cwd: Path, layout: Node) -> None:
    # Commands run as the panes' start commands: delivering them by typing
    # into a shell instead races its startup, and the kernel's canonical
    # line buffer truncates what arrives too early to 1024 bytes on macOS.
    args = ["new-session", "-d", "-s", session, "-c", str(cwd), "-P", "-F", "#{pane_id}"]
    command = _first_pane(layout).command
    if command is not None:
        args.append(command)
    try:
        first = _tmux(*args)
        leaves = _build(layout, first, cwd)
    except subprocess.CalledProcessError as exc:
        subprocess.run(["tmux", "kill-session", "-t", f"={session}"], capture_output=True)
        if "command too long" in (exc.stderr or ""):
            raise MultiplexerError("a pane command exceeds tmux's ~16KB limit") from exc
        raise
    focused = next((pane_id for pane_id, pane in leaves if pane.focus), leaves[0][0])
    _tmux("select-pane", "-t", focused)


class TmuxMultiplexer(Multiplexer):
    def __init__(self, layout: Node) -> None:
        self._layout = layout

    def can_open_in_place(self) -> bool:
        # Inside tmux, open() switches the client and returns.
        return bool(os.environ.get("TMUX"))

    def exists(self, ctx: Context) -> bool:
        result = subprocess.run(
            ["tmux", "has-session", "-t", f"={_session_name(ctx)}"], capture_output=True
        )
        return result.returncode == 0

    def is_current(self, ctx: Context) -> bool:
        if not os.environ.get("TMUX"):
            return False
        return _tmux("display-message", "-p", "#S") == _session_name(ctx)

    def create(self, ctx: Context, values: Mapping[str, str] | None = None) -> None:
        if not self.exists(ctx):
            _create_session(_session_name(ctx), ctx.path, resolve_layout(self._layout, values))

    def open(self, ctx: Context, values: Mapping[str, str] | None = None) -> None:
        session = _session_name(ctx)
        self.create(ctx, values)
        if os.environ.get("TMUX"):
            _tmux("switch-client", "-t", f"={session}")
        else:
            os.execvp("tmux", ["tmux", "attach-session", "-t", f"={session}"])

    def kill(self, ctx: Context) -> None:
        _tmux("kill-session", "-t", f"={_session_name(ctx)}")
