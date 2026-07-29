from __future__ import annotations

from abc import ABC, abstractmethod
from enum import StrEnum
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from ctx.contexts import Context
    from ctx.layout import Node


class MultiplexerKind(StrEnum):
    TMUX = "tmux"
    ZELLIJ = "zellij"


class MultiplexerError(Exception):
    pass


class Multiplexer(ABC):
    @abstractmethod
    def exists(self, ctx: Context) -> bool:
        """Whether a session for this context is running."""

    @abstractmethod
    def open(self, ctx: Context) -> None:
        """Create the context's session if needed, then attach to it."""

    @abstractmethod
    def kill(self, ctx: Context) -> None:
        """Tear down the context's session."""


def get_backend(kind: MultiplexerKind, layout: Node) -> Multiplexer:
    match kind:
        case MultiplexerKind.TMUX:
            from ctx.backends.tmux import TmuxBackend

            return TmuxBackend(layout)
        case MultiplexerKind.ZELLIJ:
            from ctx.backends.zellij import ZellijBackend

            return ZellijBackend(layout)
