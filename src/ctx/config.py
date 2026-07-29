import tomllib
from dataclasses import dataclass
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
    kwargs: dict[str, object] = {}
    for key in ("contexts_dir", "repos_dir"):
        if key in data:
            kwargs[key] = Path(data[key]).expanduser()
    if "branch_prefix" in data:
        kwargs["branch_prefix"] = data["branch_prefix"]
    return Config(**kwargs)
