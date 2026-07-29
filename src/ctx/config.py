import tomllib
from dataclasses import dataclass, replace
from pathlib import Path

CONFIG_PATH = Path.home() / ".config" / "ctx" / "config.toml"


@dataclass(frozen=True)
class Config:
    contexts_dir: Path = Path.home() / "dev" / "contexts"
    repos_dir: Path = Path.home() / ".local" / "share" / "ctx" / "repos"
    branch_prefix: str = "mb/"


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
    return cfg
