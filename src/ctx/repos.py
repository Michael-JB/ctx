from pathlib import Path

from ctx.config import Config
from ctx.git import git


def repo_path(cfg: Config, name: str) -> Path:
    return cfg.repos_dir / f"{name}.git"


def repo_names(cfg: Config) -> list[str]:
    if not cfg.repos_dir.is_dir():
        return []
    return sorted(p.name.removesuffix(".git") for p in cfg.repos_dir.glob("*.git"))


def name_from_url(url: str) -> str:
    return url.rstrip("/").rsplit("/", 1)[-1].removesuffix(".git")


def add_repo(cfg: Config, url: str, name: str | None = None) -> str:
    name = name or name_from_url(url)
    path = repo_path(cfg, name)
    if path.exists():
        raise FileExistsError(f"repo '{name}' already registered at {path}")
    cfg.repos_dir.mkdir(parents=True, exist_ok=True)
    git("clone", "--bare", url, str(path))
    # Bare clones get no fetch refspec; mirror branches so updates work.
    git("config", "remote.origin.fetch", "+refs/heads/*:refs/heads/*", cwd=path)
    return name


def remove_repo(cfg: Config, name: str) -> None:
    import shutil

    path = repo_path(cfg, name)
    if not path.exists():
        raise FileNotFoundError(f"repo '{name}' is not registered")
    shutil.rmtree(path)


def update_repo(cfg: Config, name: str) -> None:
    path = repo_path(cfg, name)
    git("fetch", "--prune", "origin", cwd=path)


def repo_url(cfg: Config, name: str) -> str:
    return git("remote", "get-url", "origin", cwd=repo_path(cfg, name))


def default_branch(cfg: Config, name: str) -> str:
    return git("symbolic-ref", "--short", "HEAD", cwd=repo_path(cfg, name))
