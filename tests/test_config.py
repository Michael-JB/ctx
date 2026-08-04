from pathlib import Path

import pytest

from ctx.config import Config, ConfigError, load_config
from ctx.layout import Pane
from ctx.multiplexer import MultiplexerKind


def write_config(tmp_path: Path, text: str) -> Path:
    path = tmp_path / "config.toml"
    path.write_text(text)
    return path


def test_missing_file_gives_defaults(tmp_path: Path) -> None:
    cfg = load_config(tmp_path / "missing.toml")

    assert cfg == Config()


def test_contexts_dir_override(tmp_path: Path) -> None:
    path = write_config(tmp_path, 'contexts_dir = "/data/contexts"')

    cfg = load_config(path)

    assert cfg.contexts_dir == Path("/data/contexts")


def test_repos_dir_override_expands_user(tmp_path: Path) -> None:
    path = write_config(tmp_path, 'repos_dir = "~/repos"')

    cfg = load_config(path)

    assert cfg.repos_dir == Path.home() / "repos"


def test_branch_prefix_override(tmp_path: Path) -> None:
    path = write_config(tmp_path, 'branch_prefix = "mb/"')

    cfg = load_config(path)

    assert cfg.branch_prefix == "mb/"


def test_multiplexer_override(tmp_path: Path) -> None:
    path = write_config(tmp_path, 'multiplexer = "zellij"')

    cfg = load_config(path)

    assert cfg.multiplexer is MultiplexerKind.ZELLIJ


def test_layout_override(tmp_path: Path) -> None:
    path = write_config(tmp_path, 'layout = { command = "nvim" }')

    cfg = load_config(path)

    assert cfg.layout == Pane("nvim")


def test_unknown_multiplexer_rejected(tmp_path: Path) -> None:
    path = write_config(tmp_path, 'multiplexer = "screen"')

    with pytest.raises(ConfigError, match="unknown multiplexer"):
        load_config(path)
