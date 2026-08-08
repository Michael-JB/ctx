import asyncio
import subprocess
import time
from pathlib import Path
from typing import Any

import pytest

import ctx.git
from ctx.git import git, git_async


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


def test_the_stall_config_reaches_git(origin: Path) -> None:
    """Unmocked: proves the -c options sit where git accepts them."""
    assert git("config", "--get", "http.lowSpeedLimit", cwd=origin) == "1000"


def test_git_still_reports_failure(origin: Path) -> None:
    with pytest.raises(subprocess.CalledProcessError):
        git("rev-parse", "--verify", "no-such-ref", cwd=origin)


def test_git_async_returns_output_and_reports_failure(origin: Path) -> None:
    assert asyncio.run(git_async("rev-parse", "--abbrev-ref", "HEAD", cwd=origin)) == "main"

    with pytest.raises(subprocess.CalledProcessError) as exc_info:
        asyncio.run(git_async("rev-parse", "--verify", "no-such-ref", cwd=origin))
    assert "fatal" in exc_info.value.stderr


def test_git_async_cancellation_kills_the_transport(
    origin: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Cancelling a fetch must end git and its transport, not orphan them."""
    # A transport that hangs forever; the trailing # swallows git's arguments.
    monkeypatch.setenv("GIT_SSH_COMMAND", "sleep 599 #")

    async def drive() -> None:
        fetch = asyncio.create_task(git_async("fetch", "ssh://host.invalid/x", cwd=origin))
        await asyncio.sleep(0.5)
        fetch.cancel()
        with pytest.raises(asyncio.CancelledError):
            await fetch

    start = time.perf_counter()
    asyncio.run(drive())

    assert time.perf_counter() - start < 3, "cancellation waited for the transfer"
    lingering = subprocess.run(["pgrep", "-f", "sleep 599"], capture_output=True, text=True)
    assert not lingering.stdout.strip(), "the transport outlived the cancelled fetch"
