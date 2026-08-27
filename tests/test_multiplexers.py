import re
import shlex
import subprocess
from pathlib import Path

import pytest

from ctx.contexts import Context
from ctx.layout import Pane, Split, SplitDirection
from ctx.multiplexer import MultiplexerError
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


def test_tmux_open_resolves_builtin_panes_on_session_creation(monkeypatch) -> None:
    calls: list[tuple[str, ...]] = []

    def fake_tmux(*args: str) -> str:
        calls.append(args)
        return "%0"

    monkeypatch.setattr(tmux, "_tmux", fake_tmux)
    monkeypatch.setattr(tmux.TmuxMultiplexer, "exists", lambda self, ctx: False)
    monkeypatch.setenv("TMUX", "/tmp/tmux-1/default,1,0")
    mux = tmux.TmuxMultiplexer(Pane(builtin="claude"))

    mux.open(Context(repo="repo", name="a", path=Path("/w")), {"prompt": "hi"})

    (new_session,) = [call for call in calls if call[0] == "new-session"]
    launcher = shlex.split(new_session[-1])
    assert launcher[0] == "sh"
    script = Path(launcher[1]).read_text()
    assert "exec sh -c 'ctx builtin claude trust; exec claude hi' <&9 9<&-" in script


def test_tmux_split_panes_start_their_own_commands(monkeypatch) -> None:
    calls: list[tuple[str, ...]] = []

    def fake_tmux(*args: str) -> str:
        calls.append(args)
        return f"%{len(calls)}"

    monkeypatch.setattr(tmux, "_tmux", fake_tmux)
    layout = Split(SplitDirection.ROW, (Pane("nvim"), Pane(), Pane("htop")))

    tmux._create_session("s", Path("/w"), layout)

    (new_session,) = [call for call in calls if call[0] == "new-session"]
    assert new_session[-1] == "nvim"
    shell_split, htop_split = [call for call in calls if call[0] == "split-window"]
    assert shell_split[-1] == "#{pane_id}"
    assert htop_split[-1] == "htop"


def test_tmux_reports_an_over_long_pane_command(monkeypatch) -> None:
    def fake_tmux(*args: str) -> str:
        raise subprocess.CalledProcessError(1, ["tmux", *args], stderr="command too long")

    killed: list[list[str]] = []
    monkeypatch.setattr(tmux, "_tmux", fake_tmux)
    monkeypatch.setattr(tmux.subprocess, "run", lambda command, **kwargs: killed.append(command))

    with pytest.raises(MultiplexerError, match="16KB"):
        tmux._create_session("s", Path("/w"), Pane("claude " + "x" * 20_000))

    assert killed == [["tmux", "kill-session", "-t", "=s"]]


def test_tmux_create_does_not_attach(monkeypatch) -> None:
    calls: list[tuple[str, ...]] = []

    def fake_tmux(*args: str) -> str:
        calls.append(args)
        return "%0"

    monkeypatch.setattr(tmux, "_tmux", fake_tmux)
    monkeypatch.setattr(tmux.TmuxMultiplexer, "exists", lambda self, ctx: False)
    monkeypatch.setenv("TMUX", "/tmp/tmux-1/default,1,0")
    mux = tmux.TmuxMultiplexer(Pane())

    mux.create(Context(repo="repo", name="a", path=Path("/w")), {})

    assert any(call[0] == "new-session" for call in calls)
    assert not any(call[0] in ("switch-client", "attach-session") for call in calls)


def test_zellij_create_makes_a_background_session(monkeypatch) -> None:
    commands: list[list[str]] = []
    envs: list[dict[str, str] | None] = []

    def fake_run(command, **kwargs):
        commands.append(command)
        envs.append(kwargs.get("env"))

        class Result:
            returncode = 0
            stdout = ""
            stderr = ""

        return Result()

    monkeypatch.setattr(zellij.subprocess, "run", fake_run)
    monkeypatch.setattr(zellij.ZellijMultiplexer, "exists", lambda self, ctx: False)
    monkeypatch.setenv("ZELLIJ", "0")
    monkeypatch.setenv("ZELLIJ_SESSION_NAME", "elsewhere")
    mux = zellij.ZellijMultiplexer(Pane())

    mux.create(Context(repo="repo", name="a", path=Path("/w")), {})

    (command,) = commands
    assert command[0] == "zellij"
    assert command[1] == "--layout"
    assert command[3:] == ["attach", "--create-background", "repo--a"]
    # With the session env visible, zellij would open the layout as new
    # tabs of the current session instead of creating one.
    (env,) = envs
    assert env is not None
    assert not any(key.startswith("ZELLIJ") for key in env)


def test_zellij_layout_file_resolves_builtin_panes() -> None:
    mux = zellij.ZellijMultiplexer(Pane(builtin="claude"))
    ctx = Context(repo="repo", name="a", path=Path("/w"))

    layout_file = mux._write_layout_file(ctx, {"prompt": "explore x"})

    content = Path(layout_file).read_text()
    assert 'command="sh"' in content
    match = re.search(r'args "([^"]+)"', content)
    assert match is not None
    script = Path(match.group(1)).read_text()
    assert "ctx builtin claude trust; exec claude '\"'\"'explore x'\"'\"'" in script


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
