import json
import os
from pathlib import Path

import pytest

from ctx import claude_hook


@pytest.fixture
def checkout(tmp_path: Path) -> Path:
    (tmp_path / ".git").mkdir()
    return tmp_path


def event(name: str, **fields: str) -> str:
    return json.dumps({"hook_event_name": name, **fields})


def status_of(checkout: Path) -> str:
    return (checkout / ".git" / "agent-status").read_text()


@pytest.mark.parametrize(
    ("payload", "state"),
    [
        (event("UserPromptSubmit"), "working"),
        (event("PreToolUse", tool_name="Bash"), "working"),
        (event("PreToolUse", tool_name="Monitor"), "monitoring"),
        (event("PreToolUse", tool_name="ScheduleWakeup"), "monitoring"),
        (event("Notification"), "blocked"),
        (event("Stop"), "idle"),
    ],
)
def test_events_map_to_states(checkout: Path, payload: str, state: str) -> None:
    claude_hook.handle(payload, checkout)

    assert status_of(checkout) == f"{state}\n"


def test_session_end_removes_the_file(checkout: Path) -> None:
    claude_hook.handle(event("Stop"), checkout)
    claude_hook.handle(event("SessionEnd"), checkout)

    assert not (checkout / ".git" / "agent-status").exists()
    claude_hook.handle(event("SessionEnd"), checkout)  # idempotent


def test_unchanged_state_keeps_the_mtime(checkout: Path) -> None:
    """The file's mtime is the state's start; rewrites would reset the clock."""
    claude_hook.handle(event("UserPromptSubmit"), checkout)
    path = checkout / ".git" / "agent-status"
    started = 1_000_000_000
    os.utime(path, (started, started))

    claude_hook.handle(event("PreToolUse", tool_name="Bash"), checkout)

    assert path.stat().st_mtime == started
    claude_hook.handle(event("Stop"), checkout)
    assert path.stat().st_mtime != started


def test_outside_a_checkout_does_nothing(tmp_path: Path) -> None:
    claude_hook.handle(event("Stop"), tmp_path)

    assert not (tmp_path / ".git").exists()


def test_unusable_input_is_ignored(checkout: Path) -> None:
    claude_hook.handle("not json", checkout)
    claude_hook.handle("[1, 2]", checkout)
    claude_hook.handle(event("SomethingNew"), checkout)

    assert not (checkout / ".git" / "agent-status").exists()
