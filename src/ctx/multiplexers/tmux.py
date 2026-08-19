import os
import subprocess
from collections.abc import Mapping
from pathlib import Path

from ctx.contexts import Context
from ctx.layout import Node, Pane, SplitDirection, resolve_layout
from ctx.multiplexer import Multiplexer


def _session_name(ctx: Context) -> str:
    raw = f"{ctx.repo}--{ctx.name}"
    # tmux forbids '.' and ':' in session names.
    return raw.replace(".", "-").replace(":", "-")


def _tmux(*args: str) -> str:
    result = subprocess.run(["tmux", *args], check=True, stdout=subprocess.PIPE, text=True)
    return result.stdout.strip()


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
    for _ in node.panes[1:]:
        regions.append(
            _tmux("split-window", flag, "-t", regions[-1], "-c", str(cwd), "-P", "-F", "#{pane_id}")
        )
    leaves: list[tuple[str, Pane]] = []
    for child, region in zip(node.panes, regions, strict=True):
        leaves.extend(_build(child, region, cwd))
    return leaves


def _create_session(session: str, cwd: Path, layout: Node) -> None:
    first = _tmux("new-session", "-d", "-s", session, "-c", str(cwd), "-P", "-F", "#{pane_id}")
    leaves = _build(layout, first, cwd)
    for pane_id, pane in leaves:
        if pane.command is not None:
            _tmux("send-keys", "-t", pane_id, pane.command, "Enter")
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
