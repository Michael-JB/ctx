"""Claude Code trust seeder: pre-trusts a directory in Claude's user config.

Claude Code gates each workspace behind a trust dialog and records the
answer per directory in its user config; recording the answer before launch
keeps a fresh checkout's session from stopping at the dialog. This module
is the Claude Code side of that contract.
"""

import json
import os
import shutil
from pathlib import Path


def _config_file() -> Path:
    config_dir = os.environ.get("CLAUDE_CONFIG_DIR", "")
    return (Path(config_dir) if config_dir else Path.home()) / ".claude.json"


def trust(cwd: Path) -> None:
    """Record `cwd` as trusted in Claude Code's user config.

    Best effort: an unusable config is left alone, and an answer already
    on record is respected, whichever way — worst case the trust dialog
    shows, which must never break launching the agent.
    """
    file = _config_file()
    try:
        data = json.loads(file.read_text()) if file.exists() else {}
    except (OSError, json.JSONDecodeError):
        return
    if not isinstance(data, dict):
        return
    projects = data.setdefault("projects", {})
    if not isinstance(projects, dict):
        return
    entry = projects.setdefault(str(cwd), {})
    if not isinstance(entry, dict) or "hasTrustDialogAccepted" in entry:
        return
    entry["hasTrustDialogAccepted"] = True
    # Claude Code rewrites this file while running and keeps it private:
    # swap in a finished copy carrying the original's permissions.
    tmp = file.with_name(file.name + ".ctx-tmp")
    try:
        tmp.write_text(json.dumps(data, indent=2, ensure_ascii=False))
        if file.exists():
            shutil.copymode(file, tmp)
        tmp.replace(file)
    except OSError:
        tmp.unlink(missing_ok=True)
