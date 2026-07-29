import os
import subprocess
from pathlib import Path

from ctx.contexts import Context


def session_name(ctx: Context) -> str:
    raw = f"{ctx.repo}--{ctx.name}"
    # tmux forbids '.' and ':' in session names.
    return raw.replace(".", "-").replace(":", "-")


def _tmux(*args: str) -> str:
    result = subprocess.run(
        ["tmux", *args], check=True, stdout=subprocess.PIPE, text=True
    )
    return result.stdout.strip()


def session_exists(session: str) -> bool:
    result = subprocess.run(
        ["tmux", "has-session", "-t", f"={session}"], capture_output=True
    )
    return result.returncode == 0


def create_session(session: str, cwd: Path) -> None:
    """Left column: lazygit over nvim. Right column: claude."""
    left_top = _tmux(
        "new-session", "-d", "-s", session, "-c", str(cwd), "-P", "-F", "#{pane_id}"
    )
    right = _tmux(
        "split-window", "-h", "-t", left_top, "-c", str(cwd), "-P", "-F", "#{pane_id}"
    )
    left_bottom = _tmux(
        "split-window", "-v", "-t", left_top, "-c", str(cwd), "-P", "-F", "#{pane_id}"
    )
    _tmux("send-keys", "-t", left_top, "lazygit", "Enter")
    _tmux("send-keys", "-t", left_bottom, "nvim", "Enter")
    _tmux("send-keys", "-t", right, "claude", "Enter")
    _tmux("select-pane", "-t", right)


def attach(session: str) -> None:
    if os.environ.get("TMUX"):
        _tmux("switch-client", "-t", f"={session}")
    else:
        os.execvp("tmux", ["tmux", "attach-session", "-t", f"={session}"])


def kill_session(session: str) -> None:
    _tmux("kill-session", "-t", f"={session}")
