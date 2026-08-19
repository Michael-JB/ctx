from __future__ import annotations

from abc import ABC, abstractmethod
from collections.abc import Mapping
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
    def can_open_in_place(self) -> bool:
        """Whether open() returns control instead of taking over the terminal."""

    @abstractmethod
    def exists(self, ctx: Context) -> bool:
        """Whether a session for this context is running."""

    @abstractmethod
    def is_current(self, ctx: Context) -> bool:
        """Whether this process runs inside the context's session."""

    @abstractmethod
    def create(self, ctx: Context, values: Mapping[str, str] | None = None) -> None:
        """Create the context's session without attaching, if it doesn't exist.

        `values` has open()'s semantics.
        """

    @abstractmethod
    def open(self, ctx: Context, values: Mapping[str, str] | None = None) -> None:
        """Create the context's session if needed, then attach to it.

        `values` feed the layout's builtin panes and only take effect when
        this call creates the session. Pass a mapping (possibly empty) when
        opening a freshly created context; None marks a recreated session,
        where builtins resume instead of starting anew.
        """

    @abstractmethod
    def kill(self, ctx: Context) -> None:
        """Tear down the context's session."""


def get_multiplexer(kind: MultiplexerKind, layout: Node) -> Multiplexer:
    match kind:
        case MultiplexerKind.TMUX:
            from ctx.multiplexers.tmux import TmuxMultiplexer

            return TmuxMultiplexer(layout)
        case MultiplexerKind.ZELLIJ:
            from ctx.multiplexers.zellij import ZellijMultiplexer

            return ZellijMultiplexer(layout)
