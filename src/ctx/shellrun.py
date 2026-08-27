"""Run pane commands through the user's interactive shell.

Multiplexers exec a pane's command directly, bypassing the shell, so
prompt-hook environment loaders (direnv, mise, ...) never run for the
pane. via_shell defers each command to a launcher script instead: the
user's shell starts interactively with the script as stdin, sources its
rc files and fires its prompt hooks, and only then reads the script's
single line, which execs the command with stdin re-pointed at the
pane's tty.
"""

import os
import shlex
import tempfile
from pathlib import Path

from ctx.layout import Node, Pane, Split

# XXX Assumes $SHELL is a POSIX-family shell that runs its prompt hooks
# before the first read from a non-tty stdin (holds for zsh and bash;
# fish untested, non-POSIX shells would break).
_LAUNCHER_HEAD = """\
#!/bin/sh
shell="${SHELL:-/bin/sh}"
case "${shell##*/}" in
    zsh)
        # zsh's line editor reads the tty directly, ignoring a non-tty
        # stdin; +o zle makes zsh read the heredoc.
        set -- -i +o zle
        ;;
    *)
        set -- -i
        ;;
esac
exec "$shell" "$@" <<'CTX_PANE_COMMAND'
"""


def _launcher(command: str) -> str:
    fd, path = tempfile.mkstemp(prefix="ctx-pane-", suffix=".sh")
    os.close(fd)
    Path(path).write_text(_LAUNCHER_HEAD + f"exec {command} </dev/tty\nCTX_PANE_COMMAND\n")
    return f"sh {shlex.quote(path)}"


def via_shell(node: Node) -> Node:
    """Defer each command pane to a launcher run by the user's shell."""
    if isinstance(node, Pane):
        if node.command is None:
            return node
        return Pane(_launcher(node.command), focus=node.focus)
    return Split(node.direction, tuple(via_shell(pane) for pane in node.panes))
