import hashlib
import os
import shlex
import subprocess
import sys
import tempfile
from collections.abc import Mapping
from pathlib import Path

from ctx.contexts import Context
from ctx.layout import Node, Pane, SplitDirection, resolve_layout
from ctx.multiplexer import Multiplexer, MultiplexerError

# macOS caps sockaddr_un paths at 104 bytes including the terminator, and
# zellij offers no working way to relocate its socket dir, so session names
# must be short enough for the socket path to fit.
# See https://github.com/zellij-org/zellij/issues/5081.
_SOCKET_PATH_MAX = 103


def _session_name_budget() -> int | None:
    """Longest session name whose zellij socket path still fits, if capped."""
    if sys.platform != "darwin":
        return None
    # zellij 0.44 places sockets in <tmp>/zellij-<uid>/<contract version>/<name>.
    sock_dir = Path(tempfile.gettempdir()) / f"zellij-{os.getuid()}" / "contract_version_1"
    return _SOCKET_PATH_MAX - len(str(sock_dir)) - 1


def _session_name(ctx: Context) -> str:
    name = f"{ctx.repo}--{ctx.name}".replace(".", "-").replace(":", "-")
    budget = _session_name_budget()
    if budget is None or len(name) <= budget:
        return name
    # Truncate over-budget names; a digest of the full name keeps them unique.
    digest = hashlib.sha256(name.encode()).hexdigest()[:6]
    return f"{name[: max(budget - 7, 1)]}-{digest}"


def _kdl_string(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")
    return f'"{escaped}"'


def _render_node(node: Node, cwd: Path, indent: int) -> str:
    pad = "    " * indent
    if isinstance(node, Pane):
        argv = shlex.split(node.command) if node.command else []
        line = f"{pad}pane"
        if argv:
            line += f" command={_kdl_string(argv[0])}"
        line += f" cwd={_kdl_string(str(cwd))}"
        if node.focus:
            line += " focus=true"
        if len(argv) > 1:
            args = " ".join(_kdl_string(arg) for arg in argv[1:])
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
        )
        # Nonzero means no zellij server is running.
        if result.returncode != 0:
            return False
        return _session_name(ctx) in result.stdout.splitlines()

    def is_current(self, ctx: Context) -> bool:
        return os.environ.get("ZELLIJ_SESSION_NAME") == _session_name(ctx)

    def create(self, ctx: Context, values: Mapping[str, str] | None = None) -> None:
        if self.exists(ctx):
            return
        session = _session_name(ctx)
        layout_file = self._write_layout_file(ctx, values)
        # Inside a session, zellij turns any --layout invocation into new
        # tabs of the current session and never reaches the attach
        # subcommand; hide the session env so the command runs as if from
        # outside.
        env = {k: v for k, v in os.environ.items() if not k.startswith("ZELLIJ")}
        result = subprocess.run(
            ["zellij", "--layout", layout_file, "attach", "--create-background", session],
            capture_output=True,
            text=True,
            env=env,
        )
        if result.returncode != 0:
            detail = result.stderr.strip() or result.stdout.strip()
            message = f"zellij could not create '{session}'"
            raise MultiplexerError(f"{message}: {detail}" if detail else message)

    def open(self, ctx: Context, values: Mapping[str, str] | None = None) -> None:
        session = _session_name(ctx)
        exists = self.exists(ctx)
        if os.environ.get("ZELLIJ"):
            # A nested `zellij attach` cannot run inside a session, so re-point
            # the already-attached client instead (zellij >= 0.44). The layout
            # only takes effect when the target session doesn't exist yet.
            command = ["zellij", "action", "switch-session", session]
            if not exists:
                command += ["--layout", self._write_layout_file(ctx, values)]
            result = subprocess.run(command, capture_output=True, text=True)
            if result.returncode != 0:
                detail = result.stderr.strip() or result.stdout.strip()
                message = f"zellij could not switch to '{session}'"
                raise MultiplexerError(f"{message}: {detail}" if detail else message)
            return
        if exists:
            os.execvp("zellij", ["zellij", "attach", session])
        layout_file = self._write_layout_file(ctx, values)
        os.execvp(
            "zellij",
            ["zellij", "--session", session, "--new-session-with-layout", layout_file],
        )

    def _write_layout_file(self, ctx: Context, values: Mapping[str, str] | None) -> str:
        fd, layout_file = tempfile.mkstemp(prefix="ctx-", suffix=".kdl")
        os.close(fd)
        layout = resolve_layout(self._layout, values)
        Path(layout_file).write_text(_render_layout(layout, ctx.path))
        return layout_file

    def kill(self, ctx: Context) -> None:
        subprocess.run(
            ["zellij", "delete-session", "--force", _session_name(ctx)],
            check=True,
            stdout=subprocess.DEVNULL,
        )
