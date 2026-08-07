import os
import shlex
import subprocess
import tempfile
from pathlib import Path

from ctx.contexts import Context
from ctx.layout import Node, Pane, SplitDirection
from ctx.multiplexer import Multiplexer, MultiplexerError


def _env() -> dict[str, str]:
    # XXX: macOS caps unix socket paths at 103 bytes and its $TMPDIR is long,
    # so zellij's default socket path overflows for longer session names.
    # Point zellij at a short socket dir instead (it creates the dir itself).
    # Remove once https://github.com/zellij-org/zellij/issues/5081 is fixed.
    env = os.environ.copy()
    env.setdefault("ZELLIJ_SOCKET_DIR", f"/tmp/zellij-{os.getuid()}")
    return env


def _session_name(ctx: Context) -> str:
    raw = f"{ctx.repo}--{ctx.name}"
    return raw.replace(".", "-").replace(":", "-")


def _render_node(node: Node, cwd: Path, indent: int) -> str:
    pad = "    " * indent
    if isinstance(node, Pane):
        argv = shlex.split(node.command) if node.command else []
        line = f"{pad}pane"
        if argv:
            line += f' command="{argv[0]}"'
        line += f' cwd="{cwd}"'
        if node.focus:
            line += " focus=true"
        if len(argv) > 1:
            args = " ".join(f'"{arg}"' for arg in argv[1:])
            line += f" {{\n{pad}    args {args}\n{pad}}}"
        return line
    # Zellij's split_direction names the split axis, not the arrangement:
    # "vertical" puts panes side by side, "horizontal" stacks them.
    match node.direction:
        case SplitDirection.ROW:
            direction = "vertical"
        case SplitDirection.COLUMN:
            direction = "horizontal"
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


class ZellijMultiplexer(Multiplexer):
    def __init__(self, layout: Node) -> None:
        self._layout = layout

    def can_open_in_place(self) -> bool:
        # Inside zellij, open() re-points the current client and returns.
        return bool(os.environ.get("ZELLIJ"))

    def exists(self, ctx: Context) -> bool:
        result = subprocess.run(
            ["zellij", "list-sessions", "--short"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            env=_env(),
        )
        # Nonzero means no zellij server is running.
        if result.returncode != 0:
            return False
        return _session_name(ctx) in result.stdout.splitlines()

    def open(self, ctx: Context) -> None:
        session = _session_name(ctx)
        exists = self.exists(ctx)
        if os.environ.get("ZELLIJ"):
            # A nested `zellij attach` cannot run inside a session, so re-point
            # the already-attached client instead (zellij >= 0.44). The layout
            # only takes effect when the target session doesn't exist yet.
            command = ["zellij", "action", "switch-session", session]
            if not exists:
                command += ["--layout", self._write_layout_file(ctx)]
            result = subprocess.run(command, capture_output=True, text=True, env=_env())
            if result.returncode != 0:
                detail = result.stderr.strip() or result.stdout.strip()
                message = f"zellij could not switch to '{session}'"
                raise MultiplexerError(f"{message}: {detail}" if detail else message)
            return
        if exists:
            os.execvpe("zellij", ["zellij", "attach", session], _env())
        layout_file = self._write_layout_file(ctx)
        os.execvpe(
            "zellij",
            ["zellij", "--session", session, "--new-session-with-layout", layout_file],
            _env(),
        )

    def _write_layout_file(self, ctx: Context) -> str:
        fd, layout_file = tempfile.mkstemp(prefix="ctx-", suffix=".kdl")
        os.close(fd)
        Path(layout_file).write_text(_render_layout(self._layout, ctx.path))
        return layout_file

    def kill(self, ctx: Context) -> None:
        subprocess.run(
            ["zellij", "delete-session", "--force", _session_name(ctx)],
            check=True,
            stdout=subprocess.DEVNULL,
            env=_env(),
        )
