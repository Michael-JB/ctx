import pytest

from ctx.layout import (
    LayoutError,
    Pane,
    Split,
    SplitDirection,
    accepted_keys,
    parse_layout,
    resolve_layout,
)


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


def test_pane_takes_a_builtin_with_a_wrap() -> None:
    node = parse_layout({"builtin": "claude", "wrap": "direnv exec ."})

    assert node == Pane(builtin="claude", wrap="direnv exec .")


def test_pane_rejects_command_and_builtin_together() -> None:
    with pytest.raises(LayoutError, match="either a command or a builtin"):
        parse_layout({"command": "claude", "builtin": "claude"})


def test_pane_rejects_an_unknown_builtin() -> None:
    with pytest.raises(LayoutError, match="unknown pane builtin 'clod'"):
        parse_layout({"builtin": "clod"})


def test_pane_rejects_args_without_a_builtin() -> None:
    with pytest.raises(LayoutError, match="args require a builtin"):
        parse_layout({"command": "nvim", "args": "-R"})


def test_pane_rejects_wrap_without_a_builtin() -> None:
    with pytest.raises(LayoutError, match="wrap requires a builtin"):
        parse_layout({"command": "nvim", "wrap": "direnv exec ."})


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


def test_accepted_keys_collects_builtin_keys_across_the_tree() -> None:
    node = Split(SplitDirection.ROW, (Pane(builtin="claude"), Pane("nvim"), Pane()))

    assert accepted_keys(node) == {"prompt"}


def test_accepted_keys_is_empty_without_builtins() -> None:
    assert accepted_keys(Pane("claude")) == frozenset()


_TRUST = "ctx builtin claude trust"


def test_resolve_claude_passes_the_prompt_as_one_word() -> None:
    node = resolve_layout(Pane(builtin="claude", focus=True), {"prompt": "explore the bug"})

    quoted_prompt = "'\"'\"'explore the bug'\"'\"'"
    assert node == Pane(f"sh -c '{_TRUST}; exec claude {quoted_prompt}'", focus=True)


def test_resolve_claude_without_a_prompt() -> None:
    assert resolve_layout(Pane(builtin="claude"), {}) == Pane(f"sh -c '{_TRUST}; exec claude'")


def test_resolve_claude_keeps_extra_args() -> None:
    node = resolve_layout(Pane(builtin="claude", args="--model opus"), {})

    assert node == Pane(f"sh -c '{_TRUST}; exec claude --model opus'")


def test_resolve_claude_runs_through_the_wrap() -> None:
    node = resolve_layout(Pane(builtin="claude", args="--model opus", wrap="direnv exec ."), {})

    assert node == Pane(f"sh -c '{_TRUST}; exec direnv exec . claude --model opus'")


def test_resolve_claude_on_a_recreated_session_resumes() -> None:
    assert resolve_layout(Pane(builtin="claude"), None) == Pane(
        f"sh -c '{_TRUST}; exec claude --continue'"
    )


def test_resolve_claude_on_a_recreated_session_keeps_extra_args() -> None:
    node = resolve_layout(Pane(builtin="claude", args="--model opus"), None)

    assert node == Pane(f"sh -c '{_TRUST}; exec claude --model opus --continue'")


def test_resolve_leaves_command_panes_alone() -> None:
    node = Split(SplitDirection.COLUMN, (Pane("nvim"), Pane(builtin="claude")))

    resolved = resolve_layout(node, {"prompt": "x"})

    assert resolved == Split(
        SplitDirection.COLUMN,
        (Pane("nvim"), Pane(f"sh -c '{_TRUST}; exec claude x'")),
    )
