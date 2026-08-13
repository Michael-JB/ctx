from pathlib import Path

import pytest
from conftest import MakeOrigin, add_repo, commit_file, update_repo

from ctx import repos
from ctx.config import Config
from ctx.git import git


@pytest.mark.parametrize(
    ("url", "name"),
    [
        ("https://github.com/foo/bar.git", "bar"),
        ("https://github.com/foo/bar/", "bar"),
        ("git@github.com:foo/bar.git", "bar"),
        ("/local/path/bar", "bar"),
    ],
)
def test_name_from_url(url: str, name: str) -> None:
    assert repos.name_from_url(url) == name


def test_add_repo_registers_under_derived_name(cfg: Config, origin: Path) -> None:
    name = add_repo(cfg, str(origin))

    assert name == "origin"
    assert repos.repo_names(cfg) == ["origin"]


def test_add_repo_registers_under_given_name(cfg: Config, origin: Path) -> None:
    name = add_repo(cfg, str(origin), name="custom")

    assert name == "custom"
    assert repos.repo_names(cfg) == ["custom"]


def test_add_repo_creates_the_contexts_dir(cfg: Config, origin: Path) -> None:
    add_repo(cfg, str(origin))

    assert (cfg.contexts_dir / "origin").is_dir()


def test_add_repo_mirrors_only_the_default_branch(cfg: Config, origin: Path) -> None:
    git("branch", "other", cwd=origin)

    add_repo(cfg, str(origin))

    mirror = repos.repo_path(cfg, "origin")
    branches = git("for-each-ref", "--format=%(refname:short)", "refs/heads", cwd=mirror)
    assert branches == "main"


def test_add_repo_rejects_duplicates(cfg: Config, origin: Path) -> None:
    add_repo(cfg, str(origin))

    with pytest.raises(FileExistsError, match="already registered"):
        add_repo(cfg, str(origin))


def test_repo_names_empty_without_repos(cfg: Config) -> None:
    assert repos.repo_names(cfg) == []


def test_repo_names_sorted(cfg: Config, origin: Path) -> None:
    add_repo(cfg, str(origin), name="beta")
    add_repo(cfg, str(origin), name="alpha")

    assert repos.repo_names(cfg) == ["alpha", "beta"]


def test_repo_url_is_the_registered_url(cfg: Config, origin: Path) -> None:
    add_repo(cfg, str(origin))

    assert repos.repo_url(cfg, "origin") == str(origin)


def test_update_repo_picks_up_new_origin_commits(cfg: Config, origin: Path) -> None:
    add_repo(cfg, str(origin))
    commit_file(origin, "new.txt")

    update_repo(cfg, "origin")

    mirror = repos.repo_path(cfg, "origin")
    assert git("rev-parse", "main", cwd=mirror) == git("rev-parse", "main", cwd=origin)


def test_update_repo_tolerates_an_empty_repo(cfg: Config, make_origin: MakeOrigin) -> None:
    add_repo(cfg, str(make_origin("empty", empty=True)))

    update_repo(cfg, "empty")


def test_remove_repo_unregisters(cfg: Config, origin: Path) -> None:
    add_repo(cfg, str(origin))

    repos.remove_repo(cfg, "origin")

    assert repos.repo_names(cfg) == []


def test_remove_repo_rejects_unregistered(cfg: Config) -> None:
    with pytest.raises(FileNotFoundError, match="not registered"):
        repos.remove_repo(cfg, "nope")


def test_default_repo_is_unset_initially(cfg: Config) -> None:
    assert repos.default_repo(cfg) is None


def test_default_repo_round_trips(cfg: Config, origin: Path) -> None:
    add_repo(cfg, str(origin))

    repos.set_default_repo(cfg, "origin")
    assert repos.default_repo(cfg) == "origin"

    repos.set_default_repo(cfg, None)
    assert repos.default_repo(cfg) is None


def test_set_default_repo_rejects_unregistered(cfg: Config) -> None:
    with pytest.raises(FileNotFoundError, match="not registered"):
        repos.set_default_repo(cfg, "nope")


def test_remove_repo_clears_the_default(cfg: Config, origin: Path) -> None:
    add_repo(cfg, str(origin))
    repos.set_default_repo(cfg, "origin")

    repos.remove_repo(cfg, "origin")

    assert repos.default_repo(cfg) is None
    add_repo(cfg, str(origin))
    assert repos.default_repo(cfg) is None, "a re-added repo must not resurrect the default"
