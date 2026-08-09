import os
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass, field, replace
from pathlib import Path

from ctx.layout import DEFAULT_LAYOUT, Node, parse_layout
from ctx.multiplexer import MultiplexerKind


def _xdg_dir(variable: str, fallback: Path) -> Path:
    value = os.environ.get(variable, "")
    return Path(value) if value else Path.home() / fallback


CONFIG_PATH = _xdg_dir("XDG_CONFIG_HOME", Path(".config")) / "ctx" / "config.toml"
_DATA_DIR = _xdg_dir("XDG_DATA_HOME", Path(".local") / "share") / "ctx"


class ConfigError(Exception):
    pass


BUILTIN_STATUS = ("agent", "github", "github-checks", "github-pr")


@dataclass(frozen=True)
class StatusColumn:
    """A named status column in listings, filled by a command or a built-in.

    `interval` is the column's sampling period in seconds; None picks the
    provider's default. `icons` maps status words to display forms.
    """

    name: str
    command: str | None = None
    builtin: str | None = None
    interval: float | None = None
    icons: Mapping[str, str] = field(default_factory=dict)


# Configuring [[status]] replaces the default entirely.
DEFAULT_STATUS = (StatusColumn("pr", builtin="github"),)


@dataclass(frozen=True)
class Config:
    contexts_dir: Path = _DATA_DIR / "contexts"
    repos_dir: Path = _DATA_DIR / "repos"
    archive_dir: Path = _DATA_DIR / "archive"
    branch_prefix: str = ""
    multiplexer: MultiplexerKind = MultiplexerKind.TMUX
    layout: Node = DEFAULT_LAYOUT
    status: tuple[StatusColumn, ...] = DEFAULT_STATUS


def load_config(path: Path = CONFIG_PATH) -> Config:
    if not path.exists():
        return Config()
    data = tomllib.loads(path.read_text())
    cfg = Config()
    if "contexts_dir" in data:
        cfg = replace(cfg, contexts_dir=Path(data["contexts_dir"]).expanduser())
    if "repos_dir" in data:
        cfg = replace(cfg, repos_dir=Path(data["repos_dir"]).expanduser())
    if "archive_dir" in data:
        cfg = replace(cfg, archive_dir=Path(data["archive_dir"]).expanduser())
    if "branch_prefix" in data:
        cfg = replace(cfg, branch_prefix=str(data["branch_prefix"]))
    if "multiplexer" in data:
        raw = str(data["multiplexer"])
        try:
            kind = MultiplexerKind(raw)
        except ValueError as exc:
            valid = ", ".join(k.value for k in MultiplexerKind)
            raise ConfigError(f"unknown multiplexer '{raw}' (supported: {valid})") from exc
        cfg = replace(cfg, multiplexer=kind)
    if "layout" in data:
        cfg = replace(cfg, layout=parse_layout(data["layout"]))
    if "status" in data:
        cfg = replace(cfg, status=_parse_status(data["status"]))
    return cfg


def _parse_status(data: object) -> tuple[StatusColumn, ...]:
    if not isinstance(data, list):
        raise ConfigError("status must be an array of tables ([[status]])")
    columns = []
    for entry in data:
        if not isinstance(entry, dict) or "name" not in entry:
            raise ConfigError("each [[status]] needs a name")
        if ("command" in entry) == ("builtin" in entry):
            raise ConfigError("each [[status]] needs either a command or a builtin")
        if "builtin" in entry and entry["builtin"] not in BUILTIN_STATUS:
            valid = ", ".join(BUILTIN_STATUS)
            raise ConfigError(f"unknown status builtin '{entry['builtin']}' (supported: {valid})")
        interval = entry.get("interval")
        if interval is not None and (
            isinstance(interval, bool) or not isinstance(interval, int | float) or interval < 0
        ):
            raise ConfigError("status interval must be a non-negative number of seconds")
        icons = entry.get("icons", {})
        if not isinstance(icons, dict) or not all(
            isinstance(key, str) and isinstance(value, str) for key, value in icons.items()
        ):
            raise ConfigError("status icons must be a table of strings")
        columns.append(
            StatusColumn(
                str(entry["name"]),
                command=str(entry["command"]) if "command" in entry else None,
                builtin=str(entry["builtin"]) if "builtin" in entry else None,
                interval=float(interval) if interval is not None else None,
                icons=icons,
            )
        )
    names = [c.name for c in columns]
    if len(set(names)) != len(names):
        raise ConfigError("status names must be unique")
    return tuple(columns)
