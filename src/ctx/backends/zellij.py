import os
import shlex
import subprocess
import tempfile
from pathlib import Path

from ctx.contexts import Context
from ctx.layout import Node, Pane
from ctx.multiplexer import Multiplexer, MultiplexerError


def _session_name(ctx: Context) -> str:
    raw = f"{ctx.repo}--{ctx.name}"
    return raw.replace(".", "-").replace(":", "-")


def _render_node(node: Node, cwd: Path, indent: int) -> str:
    pad = "    " * indent
    if isinstance(node, Pane):
        argv = shlex.split(node.command)
        line = f'{pad}pane command="{argv[0]}" cwd="{cwd}"'
        if node.focus:
            line += " focus=true"
        if len(argv) > 1:
            args = " ".join(f'"{arg}"' for arg in argv[1:])
            line += f" {{\n{pad}    args {args}\n{pad}}}"
        return line
    # Zellij's split_direction names the split axis, not the arrangement:
    # "vertical" puts panes side by side, "horizontal" stacks them.
    direction = "vertical" if node.direction == "row" else "horizontal"
    children = "\n".join(_render_node(pane, cwd, indent + 1) for pane in node.panes)
    return f'{pad}pane split_direction="{direction}" {{\n{children}\n{pad}}}'


def _render_layout(layout: Node, cwd: Path) -> str:
    return (
        "layout {\n"
        "    default_tab_template {\n"
        "        pane size=1 borderless=true {\n"
        '            plugin location="zellij:tab-bar"\n'
        "        }\n"
        "        children\n"
        "        pane size=2 borderless=true {\n"
        '            plugin location="zellij:status-bar"\n'
        "        }\n"
        "    }\n"
        "    tab {\n"
        f"{_render_node(layout, cwd, 2)}\n"
        "    }\n"
        "}\n"
    )


class ZellijBackend(Multiplexer):
    def __init__(self, layout: Node) -> None:
        self._layout = layout

    def exists(self, ctx: Context) -> bool:
        result = subprocess.run(
            ["zellij", "list-sessions", "--short"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        # Nonzero means no zellij server is running.
        if result.returncode != 0:
            return False
        return _session_name(ctx) in result.stdout.splitlines()

    def open(self, ctx: Context) -> None:
        if os.environ.get("ZELLIJ"):
            raise MultiplexerError(
                "zellij cannot switch sessions from inside a session; detach first"
            )
        session = _session_name(ctx)
        if self.exists(ctx):
            os.execvp("zellij", ["zellij", "attach", session])
        fd, layout_file = tempfile.mkstemp(prefix="ctx-", suffix=".kdl")
        os.close(fd)
        Path(layout_file).write_text(_render_layout(self._layout, ctx.path))
        os.execvp(
            "zellij", ["zellij", "--session", session, "--new-session-with-layout", layout_file]
        )

    def kill(self, ctx: Context) -> None:
        subprocess.run(
            ["zellij", "delete-session", "--force", _session_name(ctx)],
            check=True,
            stdout=subprocess.DEVNULL,
        )
