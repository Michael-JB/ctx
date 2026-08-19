from pathlib import Path

from ctx.contexts import Context
from ctx.layout import Pane, Split, SplitDirection
from ctx.multiplexers import tmux, zellij


def test_tmux_session_name_replaces_forbidden_characters() -> None:
    ctx = Context(repo="my.repo", name="a:b", path=Path("/w"))

    assert tmux._session_name(ctx) == "my-repo--a-b"


def test_zellij_session_name_replaces_forbidden_characters() -> None:
    ctx = Context(repo="my.repo", name="a:b", path=Path("/w"))

    assert zellij._session_name(ctx) == "my-repo--a-b"


def test_zellij_session_name_within_budget_is_unchanged(monkeypatch) -> None:
    monkeypatch.setattr(zellij, "_session_name_budget", lambda: 20)
    ctx = Context(repo="repo", name="short", path=Path("/w"))

    assert zellij._session_name(ctx) == "repo--short"


def test_zellij_session_name_over_budget_is_shortened(monkeypatch) -> None:
    monkeypatch.setattr(zellij, "_session_name_budget", lambda: 20)
    ctx = Context(repo="repo", name="a-very-long-context-name", path=Path("/w"))

    name = zellij._session_name(ctx)

    assert len(name) == 20
    assert name.startswith("repo--a-very-")


def test_zellij_shortened_session_names_stay_unique(monkeypatch) -> None:
    monkeypatch.setattr(zellij, "_session_name_budget", lambda: 20)
    first = Context(repo="repo", name="a-very-long-context-name", path=Path("/w"))
    second = Context(repo="repo", name="a-very-long-context-nam2", path=Path("/w"))

    assert zellij._session_name(first) != zellij._session_name(second)


def test_zellij_is_current_matches_the_session_env(monkeypatch) -> None:
    ctx = Context(repo="repo", name="a", path=Path("/w"))
    mux = zellij.ZellijMultiplexer(Pane())

    monkeypatch.setenv("ZELLIJ_SESSION_NAME", zellij._session_name(ctx))
    assert mux.is_current(ctx)

    monkeypatch.setenv("ZELLIJ_SESSION_NAME", "elsewhere")
    assert not mux.is_current(ctx)


def test_tmux_is_current_is_false_outside_tmux(monkeypatch) -> None:
    monkeypatch.delenv("TMUX", raising=False)
    ctx = Context(repo="repo", name="a", path=Path("/w"))

    assert not tmux.TmuxMultiplexer(Pane()).is_current(ctx)


def test_tmux_is_current_compares_the_attached_session(monkeypatch) -> None:
    monkeypatch.setenv("TMUX", "/tmp/tmux-1/default,1,0")
    monkeypatch.setattr(tmux, "_tmux", lambda *args: "repo--a")
    mux = tmux.TmuxMultiplexer(Pane())

    assert mux.is_current(Context(repo="repo", name="a", path=Path("/w")))
    assert not mux.is_current(Context(repo="repo", name="b", path=Path("/w")))


def test_zellij_layout_puts_pane_in_cwd() -> None:
    out = zellij._render_layout(Pane(), Path("/w"))

    assert 'pane cwd="/w"' in out


def test_zellij_layout_splits_command_into_args() -> None:
    out = zellij._render_layout(Pane("nvim -R file.txt"), Path("/w"))

    assert 'pane command="nvim" cwd="/w"' in out
    assert 'args "-R" "file.txt"' in out


def test_zellij_layout_escapes_kdl_strings() -> None:
    out = zellij._render_layout(Pane("claude 'say \"hi\"'"), Path("/w"))

    assert 'args "say \\"hi\\""' in out


def test_zellij_layout_marks_focus() -> None:
    out = zellij._render_layout(Pane("nvim", focus=True), Path("/w"))

    assert "focus=true" in out


def test_zellij_layout_maps_row_to_vertical_split() -> None:
    out = zellij._render_layout(Split(SplitDirection.ROW, (Pane(), Pane())), Path("/w"))

    assert 'split_direction="vertical"' in out


def test_zellij_layout_maps_column_to_horizontal_split() -> None:
    out = zellij._render_layout(Split(SplitDirection.COLUMN, (Pane(), Pane())), Path("/w"))

    assert 'split_direction="horizontal"' in out
