import os
import subprocess
import threading
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

import ctx.git
from ctx.git import git


class Recorder:
    """Stands in for subprocess.run, capturing how git was invoked."""

    def __init__(self) -> None:
        self.argv: list[str] = []
        self.env: dict[str, str] = {}

    def __call__(self, argv: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
        self.argv = argv
        self.env = kwargs["env"]
        return subprocess.CompletedProcess(argv, 0, stdout="")


@pytest.fixture
def recorder(monkeypatch: pytest.MonkeyPatch) -> Recorder:
    spy = Recorder()
    monkeypatch.setattr(ctx.git.subprocess, "run", spy)
    return spy


def test_calls_cap_stalled_transfers(recorder: Recorder, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("GIT_SSH_COMMAND", raising=False)

    git("fetch", "origin")

    assert recorder.argv[:3] == ["git", "-c", "http.lowSpeedLimit=1000"]
    assert "ServerAliveInterval" in recorder.env["GIT_SSH_COMMAND"]
    assert recorder.env["GIT_TERMINAL_PROMPT"] == "0"


def test_a_configured_ssh_command_wins(recorder: Recorder, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("GIT_SSH_COMMAND", "my-ssh")

    git("fetch", "origin")

    assert recorder.env["GIT_SSH_COMMAND"] == "my-ssh"


def _run_in_thread(target: Callable[[], None]) -> threading.Thread:
    thread = threading.Thread(target=target)
    thread.start()
    return thread


def _wait_for(condition: Callable[[], bool], timeout: float = 10.0) -> None:
    deadline = time.perf_counter() + timeout
    while time.perf_counter() < deadline:
        if condition():
            return
        time.sleep(0.01)
    raise AssertionError("condition never held")


def test_terminate_running_frees_an_interruptible_call(origin: Path) -> None:
    outcome: list[BaseException] = []

    def call() -> None:
        try:
            git("-c", "alias.stall=!sleep 30", "stall", cwd=origin, interruptible=True)
        except BaseException as exc:
            outcome.append(exc)

    caller = _run_in_thread(call)
    _wait_for(lambda: bool(ctx.git._running))
    pids = [proc.pid for proc in ctx.git._running]

    ctx.git.terminate_running()
    caller.join(timeout=10)

    assert not caller.is_alive(), "the git call outlived its kill"
    assert isinstance(outcome[0], subprocess.CalledProcessError)
    assert not ctx.git._running
    for pid in pids:
        # The group, not just git: a hung ssh must not outlive it.
        with pytest.raises(ProcessLookupError):
            os.killpg(pid, 0)


def test_other_calls_are_not_interruptible(origin: Path) -> None:
    """Killing a clone would leave a half-written checkout behind."""
    caller = _run_in_thread(lambda: git("-c", "alias.nap=!sleep 2", "nap", cwd=origin))

    _wait_for(lambda: not caller.is_alive(), timeout=10)

    assert not ctx.git._running


def test_the_stall_config_reaches_git(origin: Path) -> None:
    """Unmocked: proves the -c options sit where git accepts them."""
    assert git("config", "--get", "http.lowSpeedLimit", cwd=origin) == "1000"


def test_git_still_reports_failure(origin: Path) -> None:
    with pytest.raises(subprocess.CalledProcessError):
        git("rev-parse", "--verify", "no-such-ref", cwd=origin)
