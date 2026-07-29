from abc import ABC, abstractmethod

from ctx.config import Config
from ctx.contexts import Context


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


def get_backend(cfg: Config) -> Multiplexer:
    if cfg.multiplexer == "tmux":
        from ctx.backends.tmux import TmuxBackend

        return TmuxBackend(cfg.layout)
    if cfg.multiplexer == "zellij":
        from ctx.backends.zellij import ZellijBackend

        return ZellijBackend(cfg.layout)
    raise MultiplexerError(f"unknown multiplexer '{cfg.multiplexer}' (supported: tmux, zellij)")
