"""Pane builtins: named panes whose commands ctx composes itself.

A builtin stands in for a pane's command string, letting ctx adapt the
invocation to the occasion. Each implementation's specifics live here,
behind the builtin's name.
"""

PANE_BUILTINS = ("claude",)
