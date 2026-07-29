import os
import subprocess
import tempfile
from pathlib import Path

from ctx.contexts import Context
from ctx.multiplexer import MultiplexerError

_LAYOUT = """\
layout {{
    default_tab_template {{
        pane size=1 borderless=true {{
            plugin location="zellij:tab-bar"
        }}
        children
        pane size=2 borderless=true {{
            plugin location="zellij:status-bar"
        }}
    }}
    tab {{
        pane split_direction="vertical" {{
            pane split_direction="horizontal" {{
                pane command="lazygit" cwd="{cwd}"
                pane command="nvim" cwd="{cwd}"
            }}
            pane command="claude" cwd="{cwd}" focus=true
        }}
    }}
}}
"""


def _session_name(ctx: Context) -> str:
    raw = f"{ctx.repo}--{ctx.name}"
    return raw.replace(".", "-").replace(":", "-")


class ZellijBackend:
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
        fd, layout = tempfile.mkstemp(prefix="ctx-", suffix=".kdl")
        os.close(fd)
        Path(layout).write_text(_LAYOUT.format(cwd=ctx.path))
        os.execvp("zellij", ["zellij", "--session", session, "--new-session-with-layout", layout])

    def kill(self, ctx: Context) -> None:
        subprocess.run(
            ["zellij", "delete-session", "--force", _session_name(ctx)],
            check=True,
            stdout=subprocess.DEVNULL,
        )
