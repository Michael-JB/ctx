import asyncio
import os
import shutil
from collections.abc import Callable
from pathlib import Path

import pytest

from ctx import contexts, repos
from ctx.config import Config
from ctx.git import git

MakeOrigin = Callable[..., Path]

requires_lfs = pytest.mark.skipif(shutil.which("git-lfs") is None, reason="git-lfs not installed")


# Sync conveniences over the async API, for test setup and assertions.
def create_context(cfg: Config, repo: str, name: str, base: str | None = None) -> contexts.Context:
    return asyncio.run(contexts.create_context(cfg, repo, name, base))


def add_repo(cfg: Config, url: str, name: str | None = None) -> str:
    return asyncio.run(repos.add_repo(cfg, url, name))


def update_repo(cfg: Config, name: str) -> None:
    asyncio.run(repos.update_repo(cfg, name))


@pytest.fixture(autouse=True)
def isolated_git(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Point git at a throwaway global config so tests ignore the host's."""
    gitconfig = tmp_path / "gitconfig"
    gitconfig.write_text(
        "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n"
    )
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(gitconfig))
    monkeypatch.setenv("GIT_CONFIG_SYSTEM", "/dev/null")


@pytest.fixture
def cfg(tmp_path: Path) -> Config:
    return Config(
        contexts_dir=tmp_path / "contexts",
        repos_dir=tmp_path / "repos",
        archive_dir=tmp_path / "archive",
    )


@pytest.fixture
def make_origin(tmp_path: Path) -> MakeOrigin:
    """Factory for local repos that stand in for remote origins."""

    def _make(name: str = "origin", empty: bool = False) -> Path:
        path = tmp_path / name
        path.mkdir()
        git("init", cwd=path)
        if not empty:
            commit_file(path, "README.md", "hello\n")
        return path

    return _make


@pytest.fixture
def origin(make_origin: MakeOrigin) -> Path:
    return make_origin()


def fake_cli(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, name: str, script: str) -> None:
    """Shadow an executable on PATH with a stub script."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=True)
    stub = bin_dir / name
    stub.write_text(f"#!/bin/sh\n{script}\n")
    stub.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bin_dir}:{os.environ['PATH']}")


def fake_gh(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, script: str) -> None:
    """Shadow gh on PATH with a stub script."""
    fake_cli(tmp_path, monkeypatch, "gh", script)


def commit_file(repo: Path, name: str, content: str = "x\n") -> None:
    (repo / name).write_text(content)
    git("add", name, cwd=repo)
    git("commit", "-m", f"add {name}", cwd=repo)


def commit_lfs_file(repo: Path, name: str, content: str = "payload\n") -> None:
    """Commit a file tracked by git-lfs, installing its filters in the isolated config."""
    git("lfs", "install", cwd=repo)
    git("lfs", "track", name, cwd=repo)
    (repo / name).write_text(content)
    git("add", ".gitattributes", name, cwd=repo)
    git("commit", "-m", f"add {name}", cwd=repo)
