from dataclasses import dataclass
from typing import Any


class LayoutError(Exception):
    pass


@dataclass(frozen=True)
class Pane:
    command: str | None = None  # None means a plain shell
    focus: bool = False


@dataclass(frozen=True)
class Split:
    direction: str  # "row" places panes side by side, "column" stacks them
    panes: tuple["Node", ...]


Node = Pane | Split

DEFAULT_LAYOUT: Node = Pane()


def parse_layout(data: dict[str, Any]) -> Node:
    node = _parse_node(data)
    if _count_focus(node) > 1:
        raise LayoutError("at most one pane may set focus")
    return node


def _parse_node(data: dict[str, Any]) -> Node:
    if "split" in data:
        unknown = data.keys() - {"split", "panes"}
        if unknown:
            raise LayoutError(f"unknown split key(s): {', '.join(sorted(unknown))}")
        direction = data["split"]
        if direction not in ("row", "column"):
            raise LayoutError(f"split must be 'row' or 'column', got {direction!r}")
        panes = data.get("panes")
        if not isinstance(panes, list) or not panes:
            raise LayoutError("a split needs a non-empty 'panes' list")
        return Split(direction, tuple(_parse_node(pane) for pane in panes))
    unknown = data.keys() - {"command", "focus"}
    if unknown:
        raise LayoutError(f"unknown pane key(s): {', '.join(sorted(unknown))}")
    command = str(data["command"]) if "command" in data else None
    return Pane(command, focus=bool(data.get("focus", False)))


def _count_focus(node: Node) -> int:
    if isinstance(node, Pane):
        return int(node.focus)
    return sum(_count_focus(pane) for pane in node.panes)
