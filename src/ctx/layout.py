from dataclasses import dataclass
from typing import Any


class LayoutError(Exception):
    pass


@dataclass(frozen=True)
class Pane:
    command: str
    focus: bool = False


@dataclass(frozen=True)
class Split:
    direction: str  # "row" places panes side by side, "column" stacks them
    panes: tuple["Node", ...]


Node = Pane | Split

DEFAULT_LAYOUT: Node = Split(
    "row",
    (
        Split("column", (Pane("lazygit"), Pane("nvim"))),
        Pane("claude", focus=True),
    ),
)


def parse_layout(data: dict[str, Any]) -> Node:
    node = _parse_node(data)
    if _count_focus(node) > 1:
        raise LayoutError("at most one pane may set focus")
    return node


def _parse_node(data: dict[str, Any]) -> Node:
    if "command" in data and "split" in data:
        raise LayoutError("a layout node is either a 'command' pane or a 'split', not both")
    if "command" in data:
        return Pane(str(data["command"]), focus=bool(data.get("focus", False)))
    if "split" in data:
        direction = data["split"]
        if direction not in ("row", "column"):
            raise LayoutError(f"split must be 'row' or 'column', got {direction!r}")
        panes = data.get("panes")
        if not isinstance(panes, list) or not panes:
            raise LayoutError("a split needs a non-empty 'panes' list")
        return Split(direction, tuple(_parse_node(pane) for pane in panes))
    raise LayoutError("a layout node needs either 'command' or 'split'")


def _count_focus(node: Node) -> int:
    if isinstance(node, Pane):
        return int(node.focus)
    return sum(_count_focus(pane) for pane in node.panes)
