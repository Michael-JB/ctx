"""Pane builtins: named panes whose commands ctx composes itself.

A builtin stands in for a pane's command string, letting ctx adapt the
invocation to the occasion. Each implementation's specifics live here,
behind the builtin's name.
"""

import json
import os
import shlex
import shutil
from collections.abc import Mapping
from pathlib import Path


def _claude(args: str | None, values: Mapping[str, str] | None) -> str:
    command = "claude"
    if args:
        command += f" {args}"
    if values is None:
        # A recreated session resumes the checkout's conversation.
        command += " --continue"
    elif "prompt" in values:
        command += f" {shlex.quote(values['prompt'])}"
    return command


def _claude_config_file() -> Path:
    config_dir = os.environ.get("CLAUDE_CONFIG_DIR", "")
    return (Path(config_dir) if config_dir else Path.home()) / ".claude.json"


def _claude_prepare(path: Path) -> None:
    """Pre-answer Claude Code's folder-trust dialog for the checkout.

    Claude Code asks the user to trust each new git checkout before
    applying the repo's settings; trusting a parent folder does not carry
    over to a clone nested inside it. Registering a repo with ctx already
    answers that question, so record the answer Claude Code would store.
    An answer already on record, whichever way, is respected.
    """
    file = _claude_config_file()
    try:
        config = json.loads(file.read_text()) if file.exists() else {}
    except (OSError, json.JSONDecodeError):
        return
    if not isinstance(config, dict):
        return
    projects = config.setdefault("projects", {})
    if not isinstance(projects, dict):
        return
    entry = projects.setdefault(str(path), {})
    if not isinstance(entry, dict) or "hasTrustDialogAccepted" in entry:
        return
    entry["hasTrustDialogAccepted"] = True
    # Claude Code rewrites this file while running and keeps it private;
    # swap in a finished copy with the original's permissions.
    tmp = file.with_name(file.name + ".ctx-tmp")
    try:
        tmp.write_text(json.dumps(config, indent=2) + "\n")
        if file.exists():
            shutil.copymode(file, tmp)
        tmp.replace(file)
    except OSError:
        tmp.unlink(missing_ok=True)


_RESOLVERS = {"claude": _claude}

_PREPARERS = {"claude": _claude_prepare}

PANE_BUILTINS = tuple(_RESOLVERS)

# The values each builtin consumes at context creation.
BUILTIN_KEYS = {"claude": frozenset({"prompt"})}


def builtin_command(name: str, args: str | None, values: Mapping[str, str] | None) -> str:
    """The command a builtin pane runs.

    `values` carries creation-time key=value data; None means the session
    is being recreated for an existing context.
    """
    return _RESOLVERS[name](args, values)


def prepare_checkout(name: str, path: Path) -> None:
    """Ready a fresh checkout for a builtin's tool.

    Best-effort by contract: preparation only smooths the tool's first
    launch, so it must never fail context creation.
    """
    prepare = _PREPARERS.get(name)
    if prepare is not None:
        prepare(path)
