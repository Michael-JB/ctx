import pytest

from ctx.layout import LayoutError, Pane, Split, SplitDirection, parse_layout


def test_empty_pane_gives_defaults() -> None:
    node = parse_layout({})

    assert node == Pane(command=None, focus=False)


def test_pane_takes_command_and_focus() -> None:
    node = parse_layout({"command": "nvim", "focus": True})

    assert node == Pane("nvim", focus=True)


def test_split_holds_panes() -> None:
    node = parse_layout({"split": "row", "panes": [{}, {"command": "htop"}]})

    assert node == Split(SplitDirection.ROW, (Pane(), Pane("htop")))


def test_splits_nest() -> None:
    node = parse_layout({"split": "column", "panes": [{"split": "row", "panes": [{}, {}]}, {}]})

    assert node == Split(
        SplitDirection.COLUMN, (Split(SplitDirection.ROW, (Pane(), Pane())), Pane())
    )


def test_pane_takes_a_builtin_with_args() -> None:
    node = parse_layout({"builtin": "claude", "args": "--model opus", "focus": True})

    assert node == Pane(builtin="claude", args="--model opus", focus=True)


def test_pane_rejects_command_and_builtin_together() -> None:
    with pytest.raises(LayoutError, match="either a command or a builtin"):
        parse_layout({"command": "claude", "builtin": "claude"})


def test_pane_rejects_an_unknown_builtin() -> None:
    with pytest.raises(LayoutError, match="unknown pane builtin 'clod'"):
        parse_layout({"builtin": "clod"})


def test_pane_rejects_args_without_a_builtin() -> None:
    with pytest.raises(LayoutError, match="args require a builtin"):
        parse_layout({"command": "nvim", "args": "-R"})


def test_unknown_pane_key_rejected() -> None:
    with pytest.raises(LayoutError, match="unknown pane key"):
        parse_layout({"comand": "nvim"})


def test_unknown_split_key_rejected() -> None:
    with pytest.raises(LayoutError, match="unknown split key"):
        parse_layout({"split": "row", "panes": [{}], "focus": True})


def test_unknown_direction_rejected() -> None:
    with pytest.raises(LayoutError, match="split must be one of"):
        parse_layout({"split": "diagonal", "panes": [{}]})


def test_empty_panes_rejected() -> None:
    with pytest.raises(LayoutError, match="non-empty 'panes'"):
        parse_layout({"split": "row", "panes": []})


def test_missing_panes_rejected() -> None:
    with pytest.raises(LayoutError, match="non-empty 'panes'"):
        parse_layout({"split": "row"})


def test_multiple_focus_rejected() -> None:
    with pytest.raises(LayoutError, match="at most one pane"):
        parse_layout({"split": "row", "panes": [{"focus": True}, {"focus": True}]})
