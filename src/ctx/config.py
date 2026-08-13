import os
import re
import tomllib
from dataclasses import dataclass, fields, replace
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


BUILTIN_STATUS = ("agent", "github")


@dataclass(frozen=True)
class StatusColumn:
    """A named status column in listings, filled by a command or a built-in.

    `interval` is the column's sampling period in seconds; None picks the
    provider's default.
    """

    name: str
    command: str | None = None
    builtin: str | None = None
    interval: float | None = None


@dataclass(frozen=True)
class Theme:
    """TUI colours; the defaults stick to the terminal's ANSI palette."""

    foreground: str = "ansi_default"
    selection: str = "ansi_blue"
    border_active: str = "ansi_blue"
    border_inactive: str = "ansi_default"


@dataclass(frozen=True)
class Config:
    contexts_dir: Path = _DATA_DIR / "contexts"
    repos_dir: Path = _DATA_DIR / "repos"
    archive_dir: Path = _DATA_DIR / "archive"
    branch_prefix: str = ""
    multiplexer: MultiplexerKind = MultiplexerKind.TMUX
    nerd_font: bool = True
    layout: Node = DEFAULT_LAYOUT
    status: tuple[StatusColumn, ...] = ()
    theme: Theme = Theme()


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
    if "nerd_font" in data:
        if not isinstance(data["nerd_font"], bool):
            raise ConfigError("nerd_font must be a boolean")
        cfg = replace(cfg, nerd_font=data["nerd_font"])
    if "layout" in data:
        cfg = replace(cfg, layout=parse_layout(data["layout"]))
    if "status" in data:
        cfg = replace(cfg, status=_parse_status(data["status"]))
    if "theme" in data:
        cfg = replace(cfg, theme=_parse_theme(data["theme"]))
    return cfg


# Hex only: the TUI toolkit's colour names are an implementation detail.
_COLOR_RE = re.compile(r"^#[0-9a-fA-F]{6}$")


def _parse_theme(data: object) -> Theme:
    if not isinstance(data, dict):
        raise ConfigError("theme must be a table")
    known = {f.name for f in fields(Theme)}
    unknown = data.keys() - known
    if unknown:
        raise ConfigError(f"unknown theme key(s): {', '.join(sorted(unknown))}")
    for key, value in data.items():
        if not isinstance(value, str) or not _COLOR_RE.match(value):
            raise ConfigError(f"theme {key} must be a hex colour like '#2d3f76'")
    return Theme(**data)


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
        columns.append(
            StatusColumn(
                str(entry["name"]),
                command=str(entry["command"]) if "command" in entry else None,
                builtin=str(entry["builtin"]) if "builtin" in entry else None,
                interval=float(interval) if interval is not None else None,
            )
        )
    names = [c.name for c in columns]
    if len(set(names)) != len(names):
        raise ConfigError("status names must be unique")
    return tuple(columns)
