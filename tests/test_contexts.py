import os
import shutil
import subprocess
from dataclasses import replace
from pathlib import Path

import pytest
from conftest import (
    MakeOrigin,
    add_repo,
    commit_file,
    commit_lfs_file,
    create_context,
    requires_lfs,
)

from ctx import contexts, repos
from ctx.config import Config
from ctx.git import git


@pytest.fixture
def registered(cfg: Config, origin: Path) -> Path:
    add_repo(cfg, str(origin))
    return origin


def test_create_checks_out_the_repo(cfg: Config, registered: Path) -> None:
    ctx = create_context(cfg, "origin", "feat")

    assert ctx.path == cfg.contexts_dir / "origin" / "feat"
    assert (ctx.path / "README.md").exists()


def test_create_starts_a_branch_named_after_the_context(cfg: Config, registered: Path) -> None:
    ctx = create_context(cfg, "origin", "feat")

    assert contexts.current_branch(ctx) == "feat"


def test_create_adopts_an_existing_local_branch(cfg: Config, registered: Path) -> None:
    # "main" already exists in the fresh clone as its default branch. The
    # context checks it out instead of failing to create a branch of that name.
    ctx = create_context(cfg, "origin", "main")

    assert contexts.current_branch(ctx) == "main"
    # No second branch was forked; the existing one is all there is.
    assert git("branch", "--format=%(refname:short)", cwd=ctx.path) == "main"


def test_create_applies_the_branch_prefix(cfg: Config, registered: Path) -> None:
    cfg = replace(cfg, branch_prefix="mb/")

    ctx = create_context(cfg, "origin", "feat")

    assert contexts.current_branch(ctx) == "mb/feat"


def test_create_maps_name_spaces_to_branch_dashes(cfg: Config, registered: Path) -> None:
    ctx = create_context(cfg, "origin", "two words")

    assert contexts.current_branch(ctx) == "two-words"
    assert contexts.find_context(cfg, "two words") == ctx


@requires_lfs
def test_create_smudges_lfs_files_from_the_mirror(cfg: Config, origin: Path) -> None:
    commit_lfs_file(origin, "data.bin", "payload\n")
    add_repo(cfg, str(origin))
    # Only the mirror's LFS store may serve the checkout.
    shutil.rmtree(origin / ".git" / "lfs")

    ctx = create_context(cfg, "origin", "feat")

    assert (ctx.path / "data.bin").read_text() == "payload\n"


@requires_lfs
def test_failed_clone_leaves_nothing_behind(cfg: Config, origin: Path) -> None:
    commit_lfs_file(origin, "data.bin")
    add_repo(cfg, str(origin))
    # A mirror missing LFS objects fails the clone's checkout; the explicit
    # base skips the mirror update that would repopulate the store.
    shutil.rmtree(repos.repo_path(cfg, "origin") / "lfs")

    with pytest.raises(subprocess.CalledProcessError):
        create_context(cfg, "origin", "feat", base="main")

    assert not (cfg.contexts_dir / "origin" / "feat").exists()


def test_failed_clone_spares_a_preexisting_directory(cfg: Config, registered: Path) -> None:
    path = cfg.contexts_dir / "origin" / "feat"
    path.mkdir(parents=True)
    (path / "keep.txt").write_text("x\n")

    with pytest.raises(subprocess.CalledProcessError):
        create_context(cfg, "origin", "feat")

    assert (path / "keep.txt").exists()


@pytest.mark.parametrize("name", ["", "   "])
def test_create_rejects_an_empty_name(cfg: Config, registered: Path, name: str) -> None:
    with pytest.raises(ValueError, match="must not be empty"):
        create_context(cfg, "origin", name)


@pytest.mark.parametrize("name", ["a/b", ".", ".."])
def test_create_rejects_path_like_names(cfg: Config, registered: Path, name: str) -> None:
    with pytest.raises(ValueError, match="single path component"):
        create_context(cfg, "origin", name)

    assert contexts.list_contexts(cfg) == []


def test_create_rejects_an_option_like_name(cfg: Config, registered: Path) -> None:
    with pytest.raises(ValueError, match="must not start with '-'"):
        create_context(cfg, "origin", "-feat")


@pytest.mark.parametrize("name", ["feat~1", "a..b", "what?", "tab\there", ".hidden", "feat.lock"])
def test_create_rejects_names_unfit_for_branches(cfg: Config, registered: Path, name: str) -> None:
    with pytest.raises(ValueError, match="valid branch name"):
        create_context(cfg, "origin", name)

    assert contexts.list_contexts(cfg) == []


def test_create_points_the_remote_at_the_registered_url(cfg: Config, registered: Path) -> None:
    ctx = create_context(cfg, "origin", "feat")

    assert git("remote", "get-url", "origin", cwd=ctx.path) == str(registered)


def test_create_includes_the_latest_origin_commits(cfg: Config, registered: Path) -> None:
    commit_file(registered, "new.txt")

    ctx = create_context(cfg, "origin", "feat")

    assert (ctx.path / "new.txt").exists()


def test_create_from_a_base_branch(cfg: Config, registered: Path) -> None:
    git("branch", "other", cwd=registered)
    commit_file(registered, "on-main.txt")

    ctx = create_context(cfg, "origin", "feat", base="other")

    assert contexts.current_branch(ctx) == "feat"
    assert not (ctx.path / "on-main.txt").exists()


def test_create_with_a_missing_base_fails_cleanly(cfg: Config, registered: Path) -> None:
    with pytest.raises(FileNotFoundError, match="branch 'nope' not found"):
        create_context(cfg, "origin", "feat", base="nope")

    assert contexts.list_contexts(cfg) == []


def test_create_rejects_a_taken_name(cfg: Config, registered: Path) -> None:
    create_context(cfg, "origin", "feat")

    with pytest.raises(FileExistsError, match="already used by origin/feat"):
        create_context(cfg, "origin", "feat")


def test_create_rejects_an_unregistered_repo(cfg: Config) -> None:
    with pytest.raises(FileNotFoundError, match="not registered"):
        create_context(cfg, "nope", "feat")


def test_create_from_an_empty_repo(cfg: Config, make_origin: MakeOrigin) -> None:
    add_repo(cfg, str(make_origin("empty", empty=True)))

    ctx = create_context(cfg, "empty", "feat")

    assert contexts.current_branch(ctx) == "feat"
    # Listing must not choke on the missing reflog/index of an unborn branch.
    assert contexts.list_contexts(cfg) == [ctx]


def test_random_name_pairs_an_adjective_with_an_animal(cfg: Config) -> None:
    adjective, animal = contexts.random_name(cfg).split("-")

    assert adjective in contexts._ADJECTIVES
    assert animal in contexts._ANIMALS


def test_random_name_avoids_live_and_archived_names(
    cfg: Config, registered: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(contexts, "_ADJECTIVES", ("holy",))
    monkeypatch.setattr(contexts, "_ANIMALS", ("tiger", "otter"))
    contexts.archive_context(cfg, create_context(cfg, "origin", "holy-tiger"))

    assert contexts.random_name(cfg) == "holy-otter"


def test_random_name_fails_when_all_names_are_taken(
    cfg: Config, registered: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(contexts, "_ADJECTIVES", ("holy",))
    monkeypatch.setattr(contexts, "_ANIMALS", ("tiger",))
    create_context(cfg, "origin", "holy-tiger")

    with pytest.raises(FileExistsError, match="all generated names are taken"):
        contexts.random_name(cfg)


def test_list_contexts_returns_created_contexts(cfg: Config, registered: Path) -> None:
    created = create_context(cfg, "origin", "feat")

    assert contexts.list_contexts(cfg) == [created]


def _set_activity(ctx: contexts.Context, when: int) -> None:
    for rel in (".git/logs/HEAD", ".git/index"):
        os.utime(ctx.path / rel, (when, when))


def test_list_contexts_sorts_most_recently_active_first(cfg: Config, registered: Path) -> None:
    older = create_context(cfg, "origin", "older")
    newer = create_context(cfg, "origin", "newer")
    _set_activity(older, 1_000)
    _set_activity(newer, 2_000)

    assert contexts.list_contexts(cfg) == [newer, older]

    _set_activity(older, 3_000)

    assert contexts.list_contexts(cfg) == [older, newer]


def test_index_activity_alone_counts_as_recency(cfg: Config, registered: Path) -> None:
    quiet = create_context(cfg, "origin", "quiet")
    staged = create_context(cfg, "origin", "staged")
    _set_activity(quiet, 1_000)
    _set_activity(staged, 1_000)

    os.utime(staged.path / ".git" / "index", (2_000, 2_000))

    assert contexts.list_contexts(cfg) == [staged, quiet]


def test_find_context_resolves_by_name(cfg: Config, registered: Path) -> None:
    created = create_context(cfg, "origin", "feat")

    assert contexts.find_context(cfg, "feat") == created


def test_find_context_rejects_unknown_names(cfg: Config) -> None:
    with pytest.raises(LookupError, match="no context 'feat'"):
        contexts.find_context(cfg, "feat")


def test_fresh_context_is_clean(cfg: Config, registered: Path) -> None:
    ctx = create_context(cfg, "origin", "feat")

    assert not contexts.is_dirty(ctx)
    assert contexts.unpushed_commits(ctx) == []


def test_uncommitted_file_makes_context_dirty(cfg: Config, registered: Path) -> None:
    ctx = create_context(cfg, "origin", "feat")

    (ctx.path / "scratch.txt").write_text("x\n")

    assert contexts.is_dirty(ctx)


def test_local_commit_counts_as_unpushed(cfg: Config, registered: Path) -> None:
    ctx = create_context(cfg, "origin", "feat")

    commit_file(ctx.path, "work.txt")

    assert len(contexts.unpushed_commits(ctx)) == 1


def test_archive_moves_the_checkout_out_of_the_contexts(cfg: Config, registered: Path) -> None:
    ctx = create_context(cfg, "origin", "feat")

    archived = contexts.archive_context(cfg, ctx)

    assert archived.path == cfg.archive_dir / "origin" / "feat"
    assert (archived.path / "README.md").exists()
    assert contexts.list_contexts(cfg) == []
    assert contexts.list_archived(cfg) == [archived]


def test_archive_keeps_the_context_name_reserved(cfg: Config, registered: Path) -> None:
    archived = contexts.archive_context(cfg, create_context(cfg, "origin", "feat"))

    with pytest.raises(FileExistsError, match="already used by archived origin/feat"):
        create_context(cfg, "origin", "feat")

    assert contexts.list_archived(cfg) == [archived]


def test_archive_refuses_to_move_onto_an_occupied_path(cfg: Config, registered: Path) -> None:
    ctx = create_context(cfg, "origin", "feat")
    (cfg.archive_dir / "origin" / "feat").mkdir(parents=True)

    with pytest.raises(FileExistsError, match="already archived"):
        contexts.archive_context(cfg, ctx)

    assert contexts.list_contexts(cfg) == [ctx]


def test_find_archived_resolves_by_name(cfg: Config, registered: Path) -> None:
    archived = contexts.archive_context(cfg, create_context(cfg, "origin", "feat"))

    assert contexts.find_archived(cfg, "feat") == archived


def test_find_any_resolves_live_and_archived_contexts(cfg: Config, registered: Path) -> None:
    archived = contexts.archive_context(cfg, create_context(cfg, "origin", "cold"))
    live = create_context(cfg, "origin", "hot")

    assert contexts.find_any(cfg, "cold") == archived
    assert contexts.find_any(cfg, "hot") == live


def test_find_any_rejects_unknown_names(cfg: Config) -> None:
    with pytest.raises(LookupError, match="no context 'feat'"):
        contexts.find_any(cfg, "feat")


def test_find_archived_rejects_unknown_names(cfg: Config) -> None:
    with pytest.raises(LookupError, match="no archived context 'feat'"):
        contexts.find_archived(cfg, "feat")


def test_unarchive_restores_the_context(cfg: Config, registered: Path) -> None:
    created = create_context(cfg, "origin", "feat")
    archived = contexts.archive_context(cfg, created)

    restored = contexts.unarchive_context(cfg, archived)

    assert restored == created
    assert contexts.list_contexts(cfg) == [created]
    assert contexts.list_archived(cfg) == []


def test_unarchive_rejects_a_name_taken_by_a_live_context(cfg: Config, registered: Path) -> None:
    stale = contexts.archive_context(cfg, create_context(cfg, "origin", "old"))
    live = create_context(cfg, "origin", "feat")
    # Archives predating global uniqueness can still clash with a live name.
    clash = stale.path.with_name("feat")
    stale.path.rename(clash)

    with pytest.raises(FileExistsError, match="already used by origin/feat"):
        contexts.unarchive_context(cfg, contexts.Context("origin", "feat", clash))

    assert live.path.exists()
    assert clash.exists()


def test_remove_context_deletes_the_checkout(cfg: Config, registered: Path) -> None:
    ctx = create_context(cfg, "origin", "feat")

    contexts.remove_context(ctx)

    assert contexts.list_contexts(cfg) == []


def test_empty_archive_deletes_all_archived_contexts(cfg: Config, registered: Path) -> None:
    contexts.archive_context(cfg, create_context(cfg, "origin", "one"))
    contexts.archive_context(cfg, create_context(cfg, "origin", "two"))
    kept = create_context(cfg, "origin", "live")

    contexts.empty_archive(cfg)

    assert contexts.list_archived(cfg) == []
    assert contexts.list_contexts(cfg) == [kept]
