import contextlib
import os
import signal
import subprocess
import threading
from pathlib import Path

# A stalled transfer otherwise hangs forever: ssh sends no keepalives by
# default, so a dead connection is never noticed, and git accepts an
# arbitrarily slow one. Both are capped to about a minute of silence.
_SSH_COMMAND = "ssh -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3"
_STALL_CONFIG = ("-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=60")

_running: set[subprocess.Popen[str]] = set()
_running_lock = threading.Lock()


def _env() -> dict[str, str]:
    env = dict(os.environ)
    env.setdefault("GIT_SSH_COMMAND", _SSH_COMMAND)
    # A prompt would block on a terminal the caller may not be showing.
    env.setdefault("GIT_TERMINAL_PROMPT", "0")
    return env


def _kill(proc: subprocess.Popen[str]) -> None:
    """Kill a git call's whole process group, transport included.

    Signalling git alone would leave its ssh child running against the
    remote, holding a connection open with nothing left to read it.
    """
    with contextlib.suppress(OSError):
        os.killpg(proc.pid, signal.SIGKILL)


def terminate_running() -> None:
    """Kill the interruptible git calls in flight, freeing their callers.

    Quitting joins the worker threads, which cannot be interrupted, so a
    thread waiting on git holds the process open until git returns.
    """
    with _running_lock:
        for proc in list(_running):
            _kill(proc)


def git(*args: str, cwd: Path | None = None, interruptible: bool = False) -> str:
    """Run git, letting stderr (progress, errors) stream to the terminal.

    `interruptible` marks a call that may be killed part-way: a fetch writes
    nothing a later run would trip over. Clones are not marked, since killing
    one strands a half-written directory under the name it claimed.
    """
    argv = ["git", *_STALL_CONFIG, *args]
    if not interruptible:
        result = subprocess.run(
            argv, cwd=cwd, check=True, stdout=subprocess.PIPE, text=True, env=_env()
        )
        return result.stdout.strip()

    # Its own process group, so that a kill reaches the transport too.
    proc = subprocess.Popen(
        argv, cwd=cwd, stdout=subprocess.PIPE, text=True, env=_env(), start_new_session=True
    )
    with _running_lock:
        _running.add(proc)
    try:
        out, _ = proc.communicate()
    except BaseException:
        # Interrupted (ctrl-c, most likely): the child sits outside the
        # terminal's process group now, so nothing else will end it.
        _kill(proc)
        proc.wait()
        raise
    finally:
        with _running_lock:
            _running.discard(proc)
    if proc.returncode != 0:
        raise subprocess.CalledProcessError(proc.returncode, argv, output=out)
    return out.strip()
