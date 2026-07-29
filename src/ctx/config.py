import os
import tomllib
from dataclasses import dataclass, replace
from pathlib import Path

from ctx.layout import DEFAULT_LAYOUT, Node, parse_layout
from ctx.multiplexer import MultiplexerKind


def _xdg_dir(variable: str, fallback: str) -> Path:
    value = os.environ.get(variable, "")
    return Path(value) if value else Path.home() / fallback


CONFIG_PATH = _xdg_dir("XDG_CONFIG_HOME", ".config") / "ctx" / "config.toml"
_DATA_DIR = _xdg_dir("XDG_DATA_HOME", ".local/share") / "ctx"


class ConfigError(Exception):
    pass


@dataclass(frozen=True)
class Config:
    contexts_dir: Path = _DATA_DIR / "contexts"
    repos_dir: Path = _DATA_DIR / "repos"
    branch_prefix: str = ""
    multiplexer: MultiplexerKind = MultiplexerKind.TMUX
    layout: Node = DEFAULT_LAYOUT


def load_config() -> Config:
    if not CONFIG_PATH.exists():
        return Config()
    data = tomllib.loads(CONFIG_PATH.read_text())
    cfg = Config()
    if "contexts_dir" in data:
        cfg = replace(cfg, contexts_dir=Path(data["contexts_dir"]).expanduser())
    if "repos_dir" in data:
        cfg = replace(cfg, repos_dir=Path(data["repos_dir"]).expanduser())
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
    return cfg
