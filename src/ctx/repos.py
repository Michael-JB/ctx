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
    git("clone", "--bare", "--single-branch", url, str(path))
    # Bare clones get no fetch refspec; mirror only the default branch.
    branch = git("symbolic-ref", "--short", "HEAD", cwd=path)
    git("config", "remote.origin.fetch", f"+refs/heads/{branch}:refs/heads/{branch}", cwd=path)
    # The repo's contexts directory is part of its registration: users may
    # place files there (e.g. an .envrc) before any context exists.
    (cfg.contexts_dir / name).mkdir(parents=True, exist_ok=True)
    return name


def remove_repo(cfg: Config, name: str) -> None:
    import shutil

    path = repo_path(cfg, name)
    if not path.exists():
        raise FileNotFoundError(f"repo '{name}' is not registered")
    shutil.rmtree(path)


def update_repo(cfg: Config, name: str) -> None:
    """Refresh only the default branch; contexts fetch other branches from origin on demand."""
    path = repo_path(cfg, name)
    branch = default_branch(cfg, name)
    # A branch unborn on both ends (empty repo) has nothing to fetch, and
    # fetching it would fail; the local check keeps the common case one roundtrip.
    if not git("for-each-ref", f"refs/heads/{branch}", cwd=path) and not git(
        "ls-remote", "--heads", "origin", f"refs/heads/{branch}", cwd=path, interruptible=True
    ):
        return
    git(
        "fetch",
        "origin",
        f"+refs/heads/{branch}:refs/heads/{branch}",
        cwd=path,
        interruptible=True,
    )


def repo_url(cfg: Config, name: str) -> str:
    return git("remote", "get-url", "origin", cwd=repo_path(cfg, name))


def default_branch(cfg: Config, name: str) -> str:
    return git("symbolic-ref", "--short", "HEAD", cwd=repo_path(cfg, name))
