from collections.abc import Callable
from pathlib import Path

import pytest

from ctx.config import Config
from ctx.git import git

MakeOrigin = Callable[..., Path]


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


def commit_file(repo: Path, name: str, content: str = "x\n") -> None:
    (repo / name).write_text(content)
    git("add", name, cwd=repo)
    git("commit", "-m", f"add {name}", cwd=repo)
