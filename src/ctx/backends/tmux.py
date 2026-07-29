import os
import subprocess
from pathlib import Path

from ctx.contexts import Context


def _session_name(ctx: Context) -> str:
    raw = f"{ctx.repo}--{ctx.name}"
    # tmux forbids '.' and ':' in session names.
    return raw.replace(".", "-").replace(":", "-")


def _tmux(*args: str) -> str:
    result = subprocess.run(["tmux", *args], check=True, stdout=subprocess.PIPE, text=True)
    return result.stdout.strip()


def _create_session(session: str, cwd: Path) -> None:
    """Left column: lazygit over nvim. Right column: claude."""
    left_top = _tmux("new-session", "-d", "-s", session, "-c", str(cwd), "-P", "-F", "#{pane_id}")
    right = _tmux("split-window", "-h", "-t", left_top, "-c", str(cwd), "-P", "-F", "#{pane_id}")
    left_bottom = _tmux(
        "split-window", "-v", "-t", left_top, "-c", str(cwd), "-P", "-F", "#{pane_id}"
    )
    _tmux("send-keys", "-t", left_top, "lazygit", "Enter")
    _tmux("send-keys", "-t", left_bottom, "nvim", "Enter")
    _tmux("send-keys", "-t", right, "claude", "Enter")
    _tmux("select-pane", "-t", right)


class TmuxBackend:
    def exists(self, ctx: Context) -> bool:
        result = subprocess.run(
            ["tmux", "has-session", "-t", f"={_session_name(ctx)}"], capture_output=True
        )
        return result.returncode == 0

    def open(self, ctx: Context) -> None:
        session = _session_name(ctx)
        if not self.exists(ctx):
            _create_session(session, ctx.path)
        if os.environ.get("TMUX"):
            _tmux("switch-client", "-t", f"={session}")
        else:
            os.execvp("tmux", ["tmux", "attach-session", "-t", f"={session}"])

    def kill(self, ctx: Context) -> None:
        _tmux("kill-session", "-t", f"={_session_name(ctx)}")
