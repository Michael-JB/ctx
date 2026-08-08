import asyncio
import shutil
from pathlib import Path

from ctx.config import Config
from ctx.git import git, git_async


def repo_path(cfg: Config, name: str) -> Path:
    return cfg.repos_dir / f"{name}.git"


def repo_names(cfg: Config) -> list[str]:
    if not cfg.repos_dir.is_dir():
        return []
    return sorted(p.name.removesuffix(".git") for p in cfg.repos_dir.glob("*.git"))


def name_from_url(url: str) -> str:
    return url.rstrip("/").rsplit("/", 1)[-1].removesuffix(".git")


async def add_repo(cfg: Config, url: str, name: str | None = None) -> str:
    name = name or name_from_url(url)
    path = repo_path(cfg, name)
    if path.exists():
        raise FileExistsError(f"repo '{name}' already registered at {path}")
    cfg.repos_dir.mkdir(parents=True, exist_ok=True)
    try:
        await git_async("clone", "--bare", "--single-branch", url, str(path))
        # Bare clones get no fetch refspec; mirror only the default branch.
        branch = await git_async("symbolic-ref", "--short", "HEAD", cwd=path)
        await git_async(
            "config", "remote.origin.fetch", f"+refs/heads/{branch}:refs/heads/{branch}", cwd=path
        )
    except asyncio.CancelledError:
        # A half-cloned mirror would squat on the name; leave it unregistered.
        shutil.rmtree(path, ignore_errors=True)
        raise
    # The repo's contexts directory is part of its registration: users may
    # place files there (e.g. an .envrc) before any context exists.
    (cfg.contexts_dir / name).mkdir(parents=True, exist_ok=True)
    return name


def remove_repo(cfg: Config, name: str) -> None:
    path = repo_path(cfg, name)
    if not path.exists():
        raise FileNotFoundError(f"repo '{name}' is not registered")
    shutil.rmtree(path)


async def update_repo(cfg: Config, name: str) -> None:
    """Refresh only the default branch; contexts fetch other branches from origin on demand."""
    path = repo_path(cfg, name)
    branch = await default_branch(cfg, name)
    # A branch unborn on both ends (empty repo) has nothing to fetch, and
    # fetching it would fail; the local check keeps the common case one roundtrip.
    if not await git_async(
        "for-each-ref", f"refs/heads/{branch}", cwd=path
    ) and not await git_async("ls-remote", "--heads", "origin", f"refs/heads/{branch}", cwd=path):
        return
    await git_async("fetch", "origin", f"+refs/heads/{branch}:refs/heads/{branch}", cwd=path)


def repo_url(cfg: Config, name: str) -> str:
    return git("remote", "get-url", "origin", cwd=repo_path(cfg, name))


async def default_branch(cfg: Config, name: str) -> str:
    return await git_async("symbolic-ref", "--short", "HEAD", cwd=repo_path(cfg, name))
