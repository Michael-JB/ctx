"""Claude Code hook adapter: maps hook events onto the agent-status file.

The `agent` status builtin reads a state word from `.git/agent-status`;
this module is the Claude Code side of that contract. Other agent systems
can feed the same file directly.
"""

import json
from pathlib import Path

# Tools that watch or sleep rather than act.
_MONITORING_TOOLS = frozenset({"Monitor", "ScheduleWakeup"})


def _state(payload: dict[str, object]) -> str | None:
    match payload.get("hook_event_name"):
        case "UserPromptSubmit":
            return "working"
        case "PreToolUse":
            tool = payload.get("tool_name")
            return "monitoring" if tool in _MONITORING_TOOLS else "working"
        case "Notification":
            return "blocked"
        case "Stop":
            return "idle"
        case _:
            return None


def handle(raw: str, cwd: Path) -> None:
    """Apply one hook event (the JSON Claude Code pipes to hooks) in `cwd`.

    Anything unusable — no `.git` directory, malformed JSON, an unknown
    event — is ignored: a status hook must never break the agent driving it.
    """
    git_dir = cwd / ".git"
    if not git_dir.is_dir():
        return
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError:
        return
    if not isinstance(payload, dict):
        return
    path = git_dir / "agent-status"
    if payload.get("hook_event_name") == "SessionEnd":
        path.unlink(missing_ok=True)
        return
    state = _state(payload)
    if state is None:
        return
    try:
        current = path.read_text().strip()
    except OSError:
        current = None
    # Rewrite only on change: the file's mtime is the state's start.
    if current != state:
        path.write_text(f"{state}\n")
