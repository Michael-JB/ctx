"""Pane builtins: named panes whose commands ctx composes itself.

A builtin stands in for a pane's command string, letting ctx adapt the
invocation to the occasion. Each implementation's specifics live here,
behind the builtin's name.
"""

import shlex
from collections.abc import Mapping


def _claude(args: str | None, values: Mapping[str, str]) -> str:
    command = "claude"
    if args:
        command += f" {args}"
    if "prompt" in values:
        command += f" {shlex.quote(values['prompt'])}"
    return command


_RESOLVERS = {"claude": _claude}

PANE_BUILTINS = tuple(_RESOLVERS)

# The values each builtin consumes at context creation.
BUILTIN_KEYS = {"claude": frozenset({"prompt"})}


def builtin_command(name: str, args: str | None, values: Mapping[str, str]) -> str:
    """The command a builtin pane runs."""
    return _RESOLVERS[name](args, values)
