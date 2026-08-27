import shlex
from pathlib import Path

from ctx.layout import Pane, Split, SplitDirection
from ctx.shellrun import via_shell


def _script(pane: Pane) -> str:
    assert pane.command is not None
    argv = shlex.split(pane.command)
    assert argv[0] == "sh"
    return Path(argv[1]).read_text()


def test_via_shell_leaves_plain_shell_panes_alone() -> None:
    assert via_shell(Pane(focus=True)) == Pane(focus=True)


def test_via_shell_defers_the_command_to_a_launcher_script() -> None:
    wrapped = via_shell(Pane("nvim -R file.txt", focus=True))

    assert isinstance(wrapped, Pane)
    assert wrapped.focus
    script = _script(wrapped)
    assert "exec nvim -R file.txt </dev/tty" in script
    assert '"$shell" "$@"' in script


def test_via_shell_starts_the_shell_interactively() -> None:
    script = _script(via_shell(Pane("htop")))

    assert "set -- -i" in script


def test_via_shell_recurses_into_splits() -> None:
    wrapped = via_shell(Split(SplitDirection.ROW, (Pane("nvim"), Pane())))

    assert isinstance(wrapped, Split)
    first, second = wrapped.panes
    assert isinstance(first, Pane)
    assert first.command is not None
    assert first.command.startswith("sh ")
    assert second == Pane()


def test_via_shell_writes_a_fresh_script_per_pane() -> None:
    first = via_shell(Pane("nvim"))
    second = via_shell(Pane("nvim"))

    assert isinstance(first, Pane) and isinstance(second, Pane)
    assert first.command != second.command
