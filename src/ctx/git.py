import asyncio
import contextlib
import os
import signal
import subprocess
from pathlib import Path

# A stalled transfer otherwise hangs forever: ssh sends no keepalives by
# default, so a dead connection is never noticed, and git accepts an
# arbitrarily slow one. Both are capped to about a minute of silence.
_SSH_COMMAND = "ssh -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3"
_STALL_CONFIG = ("-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=60")


def _env() -> dict[str, str]:
    env = dict(os.environ)
    env.setdefault("GIT_SSH_COMMAND", _SSH_COMMAND)
    # A prompt would block on a terminal the caller may not be showing.
    env.setdefault("GIT_TERMINAL_PROMPT", "0")
    return env


def git(*args: str, cwd: Path | None = None) -> str:
    """Run git, letting stderr (progress, errors) stream to the terminal.

    Every call gets the timeouts, not just the ones that reach a remote: they
    are inert for local work, and marking each remote call by hand is a thing
    to get wrong.
    """
    result = subprocess.run(
        ["git", *_STALL_CONFIG, *args],
        cwd=cwd,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
        env=_env(),
    )
    return result.stdout.strip()


async def git_async(*args: str, cwd: Path | None = None) -> str:
    """Like `git`, but awaitable, quiet, and safe to cancel mid-transfer.

    stderr is captured into the exception rather than streamed: the callers
    are UIs that must not be written over. Cancellation kills git's whole
    process group — git alone would leave its ssh child holding the
    connection — so a caller that stops waiting stops the transfer too.
    """
    argv = ["git", *_STALL_CONFIG, *args]
    proc = await asyncio.create_subprocess_exec(
        *argv,
        cwd=cwd,
        env=_env(),
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = await proc.communicate()
    except asyncio.CancelledError:
        with contextlib.suppress(OSError):
            os.killpg(proc.pid, signal.SIGKILL)
        await proc.wait()
        raise
    if proc.returncode:
        raise subprocess.CalledProcessError(
            proc.returncode, argv, stdout.decode(errors="replace"), stderr.decode(errors="replace")
        )
    return stdout.decode(errors="replace").strip()
