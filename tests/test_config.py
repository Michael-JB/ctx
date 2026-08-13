from pathlib import Path

import pytest

from ctx.config import Config, ConfigError, StatusColumn, Theme, load_config
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


def test_archive_dir_override_expands_user(tmp_path: Path) -> None:
    path = write_config(tmp_path, 'archive_dir = "~/archive"')

    cfg = load_config(path)

    assert cfg.archive_dir == Path.home() / "archive"


def test_branch_prefix_override(tmp_path: Path) -> None:
    path = write_config(tmp_path, 'branch_prefix = "mb/"')

    cfg = load_config(path)

    assert cfg.branch_prefix == "mb/"


def test_status_columns_override(tmp_path: Path) -> None:
    path = write_config(
        tmp_path,
        """
        [[status]]
        name = "claude"
        builtin = "agent"

        [[status]]
        name = "ci"
        command = "my-ci-status"
        """,
    )

    cfg = load_config(path)

    assert cfg.status == (
        StatusColumn("claude", builtin="agent"),
        StatusColumn("ci", command="my-ci-status"),
    )


def test_no_status_columns_by_default(tmp_path: Path) -> None:
    cfg = load_config(tmp_path / "missing.toml")

    assert cfg.status == ()


def test_status_icons_override(tmp_path: Path) -> None:
    path = write_config(
        tmp_path,
        """
        [[status]]
        name = "pr"
        builtin = "github"
        [status.icons]
        merged = "M"
        """,
    )

    cfg = load_config(path)

    assert cfg.status[0].icons == {"merged": "M"}


def test_status_rejects_non_string_icons(tmp_path: Path) -> None:
    path = write_config(
        tmp_path,
        """
        [[status]]
        name = "pr"
        builtin = "github"
        [status.icons]
        merged = 3
        """,
    )

    with pytest.raises(ConfigError, match="icons"):
        load_config(path)


def test_status_requires_a_name(tmp_path: Path) -> None:
    path = write_config(tmp_path, '[[status]]\ncommand = "true"')

    with pytest.raises(ConfigError, match="needs a name"):
        load_config(path)


def test_status_requires_a_command_or_a_builtin(tmp_path: Path) -> None:
    path = write_config(tmp_path, '[[status]]\nname = "ci"')

    with pytest.raises(ConfigError, match="either a command or a builtin"):
        load_config(path)


def test_status_rejects_a_command_combined_with_a_builtin(tmp_path: Path) -> None:
    path = write_config(tmp_path, '[[status]]\nname = "ci"\ncommand = "true"\nbuiltin = "agent"')

    with pytest.raises(ConfigError, match="either a command or a builtin"):
        load_config(path)


def test_status_interval_override(tmp_path: Path) -> None:
    path = write_config(tmp_path, '[[status]]\nname = "ci"\nbuiltin = "github"\ninterval = 60')

    cfg = load_config(path)

    assert cfg.status == (StatusColumn("ci", builtin="github", interval=60.0),)


def test_status_rejects_negative_intervals(tmp_path: Path) -> None:
    path = write_config(tmp_path, '[[status]]\nname = "ci"\ncommand = "true"\ninterval = -1')

    with pytest.raises(ConfigError, match="non-negative number"):
        load_config(path)


def test_status_rejects_non_numeric_intervals(tmp_path: Path) -> None:
    path = write_config(tmp_path, '[[status]]\nname = "ci"\ncommand = "true"\ninterval = "60"')

    with pytest.raises(ConfigError, match="non-negative number"):
        load_config(path)


def test_status_rejects_unknown_builtins(tmp_path: Path) -> None:
    path = write_config(tmp_path, '[[status]]\nname = "ci"\nbuiltin = "gitlab"')

    with pytest.raises(ConfigError, match="unknown status builtin 'gitlab'"):
        load_config(path)


def test_status_rejects_duplicate_names(tmp_path: Path) -> None:
    path = write_config(
        tmp_path,
        """
        [[status]]
        name = "ci"
        command = "true"

        [[status]]
        name = "ci"
        builtin = "github"
        """,
    )

    with pytest.raises(ConfigError, match="unique"):
        load_config(path)


def test_theme_defaults_to_the_ansi_palette(tmp_path: Path) -> None:
    cfg = load_config(tmp_path / "missing.toml")

    assert cfg.theme == Theme()
    assert cfg.theme.selection == "ansi_blue"


def test_theme_override(tmp_path: Path) -> None:
    path = write_config(
        tmp_path,
        """
        [theme]
        selection = "#2d3f76"
        border_active = "#ff966c"
        """,
    )

    cfg = load_config(path)

    assert cfg.theme.selection == "#2d3f76"
    assert cfg.theme.border_active == "#ff966c"
    assert cfg.theme.foreground == "ansi_default"


def test_theme_rejects_unknown_keys(tmp_path: Path) -> None:
    path = write_config(tmp_path, '[theme]\nselektion = "#2d3f76"')

    with pytest.raises(ConfigError, match="unknown theme key"):
        load_config(path)


def test_theme_rejects_malformed_colours(tmp_path: Path) -> None:
    path = write_config(tmp_path, '[theme]\nselection = "#12"')

    with pytest.raises(ConfigError, match="colour"):
        load_config(path)


def test_theme_rejects_colour_names(tmp_path: Path) -> None:
    """Toolkit colour names are an implementation detail, not config surface."""
    path = write_config(tmp_path, '[theme]\nselection = "ansi_blue"')

    with pytest.raises(ConfigError, match="hex colour"):
        load_config(path)


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
