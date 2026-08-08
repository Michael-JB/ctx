import os
from dataclasses import replace
from pathlib import Path

import pytest
from conftest import MakeOrigin, commit_file

from ctx import contexts, repos
from ctx.config import Config
from ctx.git import git


@pytest.fixture
def registered(cfg: Config, origin: Path) -> Path:
    repos.add_repo(cfg, str(origin))
    return origin


def test_create_checks_out_the_repo(cfg: Config, registered: Path) -> None:
    ctx = contexts.create_context(cfg, "origin", "feat")

    assert ctx.path == cfg.contexts_dir / "origin" / "feat"
    assert (ctx.path / "README.md").exists()


def test_create_starts_a_branch_named_after_the_context(cfg: Config, registered: Path) -> None:
    ctx = contexts.create_context(cfg, "origin", "feat")

    assert contexts.current_branch(ctx) == "feat"


def test_create_applies_the_branch_prefix(cfg: Config, registered: Path) -> None:
    cfg = replace(cfg, branch_prefix="mb/")

    ctx = contexts.create_context(cfg, "origin", "feat")

    assert contexts.current_branch(ctx) == "mb/feat"


def test_create_points_the_remote_at_the_registered_url(cfg: Config, registered: Path) -> None:
    ctx = contexts.create_context(cfg, "origin", "feat")

    assert git("remote", "get-url", "origin", cwd=ctx.path) == str(registered)


def test_create_includes_the_latest_origin_commits(cfg: Config, registered: Path) -> None:
    commit_file(registered, "new.txt")

    ctx = contexts.create_context(cfg, "origin", "feat")

    assert (ctx.path / "new.txt").exists()


def test_create_from_a_base_branch(cfg: Config, registered: Path) -> None:
    git("branch", "other", cwd=registered)
    commit_file(registered, "on-main.txt")

    ctx = contexts.create_context(cfg, "origin", "feat", base="other")

    assert contexts.current_branch(ctx) == "feat"
    assert not (ctx.path / "on-main.txt").exists()


def test_create_with_a_missing_base_fails_cleanly(cfg: Config, registered: Path) -> None:
    with pytest.raises(FileNotFoundError, match="branch 'nope' not found"):
        contexts.create_context(cfg, "origin", "feat", base="nope")

    assert contexts.list_contexts(cfg) == []


def test_create_rejects_a_taken_name(cfg: Config, registered: Path) -> None:
    contexts.create_context(cfg, "origin", "feat")

    with pytest.raises(FileExistsError, match="already used by origin/feat"):
        contexts.create_context(cfg, "origin", "feat")


def test_create_rejects_an_unregistered_repo(cfg: Config) -> None:
    with pytest.raises(FileNotFoundError, match="not registered"):
        contexts.create_context(cfg, "nope", "feat")


def test_create_from_an_empty_repo(cfg: Config, make_origin: MakeOrigin) -> None:
    repos.add_repo(cfg, str(make_origin("empty", empty=True)))

    ctx = contexts.create_context(cfg, "empty", "feat")

    assert contexts.current_branch(ctx) == "feat"
    # Listing must not choke on the missing reflog/index of an unborn branch.
    assert contexts.list_contexts(cfg) == [ctx]


def test_list_contexts_returns_created_contexts(cfg: Config, registered: Path) -> None:
    created = contexts.create_context(cfg, "origin", "feat")

    assert contexts.list_contexts(cfg) == [created]


def _set_activity(ctx: contexts.Context, when: int) -> None:
    for rel in (".git/logs/HEAD", ".git/index"):
        os.utime(ctx.path / rel, (when, when))


def test_list_contexts_sorts_most_recently_active_first(cfg: Config, registered: Path) -> None:
    older = contexts.create_context(cfg, "origin", "older")
    newer = contexts.create_context(cfg, "origin", "newer")
    _set_activity(older, 1_000)
    _set_activity(newer, 2_000)

    assert contexts.list_contexts(cfg) == [newer, older]

    _set_activity(older, 3_000)

    assert contexts.list_contexts(cfg) == [older, newer]


def test_index_activity_alone_counts_as_recency(cfg: Config, registered: Path) -> None:
    quiet = contexts.create_context(cfg, "origin", "quiet")
    staged = contexts.create_context(cfg, "origin", "staged")
    _set_activity(quiet, 1_000)
    _set_activity(staged, 1_000)

    os.utime(staged.path / ".git" / "index", (2_000, 2_000))

    assert contexts.list_contexts(cfg) == [staged, quiet]


def test_find_context_resolves_by_name(cfg: Config, registered: Path) -> None:
    created = contexts.create_context(cfg, "origin", "feat")

    assert contexts.find_context(cfg, "feat") == created


def test_find_context_rejects_unknown_names(cfg: Config) -> None:
    with pytest.raises(LookupError, match="no context 'feat'"):
        contexts.find_context(cfg, "feat")


def test_fresh_context_is_clean(cfg: Config, registered: Path) -> None:
    ctx = contexts.create_context(cfg, "origin", "feat")

    assert not contexts.is_dirty(ctx)
    assert contexts.unpushed_commits(ctx) == []


def test_uncommitted_file_makes_context_dirty(cfg: Config, registered: Path) -> None:
    ctx = contexts.create_context(cfg, "origin", "feat")

    (ctx.path / "scratch.txt").write_text("x\n")

    assert contexts.is_dirty(ctx)


def test_local_commit_counts_as_unpushed(cfg: Config, registered: Path) -> None:
    ctx = contexts.create_context(cfg, "origin", "feat")

    commit_file(ctx.path, "work.txt")

    assert len(contexts.unpushed_commits(ctx)) == 1


def test_describe_status_is_empty_for_a_clean_checkout(cfg: Config, registered: Path) -> None:
    ctx = contexts.create_context(cfg, "origin", "feat")

    assert contexts.describe_status(ctx) == ""


def test_describe_status_marks_dirty_and_unpushed_work(cfg: Config, registered: Path) -> None:
    ctx = contexts.create_context(cfg, "origin", "feat")
    commit_file(ctx.path, "work.txt")
    (ctx.path / "scratch.txt").write_text("x\n")

    assert contexts.describe_status(ctx) == "* ↑1"


def test_archive_moves_the_checkout_out_of_the_contexts(cfg: Config, registered: Path) -> None:
    ctx = contexts.create_context(cfg, "origin", "feat")

    archived = contexts.archive_context(cfg, ctx)

    assert archived.path == cfg.archive_dir / "origin" / "feat"
    assert (archived.path / "README.md").exists()
    assert contexts.list_contexts(cfg) == []
    assert contexts.list_archived(cfg) == [archived]


def test_archive_frees_the_context_name(cfg: Config, registered: Path) -> None:
    contexts.archive_context(cfg, contexts.create_context(cfg, "origin", "feat"))

    recreated = contexts.create_context(cfg, "origin", "feat")

    assert contexts.list_contexts(cfg) == [recreated]


def test_archive_rejects_an_already_archived_name(cfg: Config, registered: Path) -> None:
    contexts.archive_context(cfg, contexts.create_context(cfg, "origin", "feat"))
    ctx = contexts.create_context(cfg, "origin", "feat")

    with pytest.raises(FileExistsError, match="already archived"):
        contexts.archive_context(cfg, ctx)

    assert contexts.list_contexts(cfg) == [ctx]


def test_find_archived_resolves_by_name(cfg: Config, registered: Path) -> None:
    archived = contexts.archive_context(cfg, contexts.create_context(cfg, "origin", "feat"))

    assert contexts.find_archived(cfg, "feat") == archived


def test_find_archived_rejects_unknown_names(cfg: Config) -> None:
    with pytest.raises(LookupError, match="no archived context 'feat'"):
        contexts.find_archived(cfg, "feat")


def test_unarchive_restores_the_context(cfg: Config, registered: Path) -> None:
    created = contexts.create_context(cfg, "origin", "feat")
    archived = contexts.archive_context(cfg, created)

    restored = contexts.unarchive_context(cfg, archived)

    assert restored == created
    assert contexts.list_contexts(cfg) == [created]
    assert contexts.list_archived(cfg) == []


def test_unarchive_rejects_a_name_taken_meanwhile(cfg: Config, registered: Path) -> None:
    archived = contexts.archive_context(cfg, contexts.create_context(cfg, "origin", "feat"))
    contexts.create_context(cfg, "origin", "feat")

    with pytest.raises(FileExistsError, match="already used by origin/feat"):
        contexts.unarchive_context(cfg, archived)

    assert contexts.list_archived(cfg) == [archived]


def test_remove_context_deletes_the_checkout(cfg: Config, registered: Path) -> None:
    ctx = contexts.create_context(cfg, "origin", "feat")

    contexts.remove_context(ctx)

    assert contexts.list_contexts(cfg) == []


def test_empty_archive_deletes_all_archived_contexts(cfg: Config, registered: Path) -> None:
    contexts.archive_context(cfg, contexts.create_context(cfg, "origin", "one"))
    contexts.archive_context(cfg, contexts.create_context(cfg, "origin", "two"))
    kept = contexts.create_context(cfg, "origin", "live")

    contexts.empty_archive(cfg)

    assert contexts.list_archived(cfg) == []
    assert contexts.list_contexts(cfg) == [kept]
