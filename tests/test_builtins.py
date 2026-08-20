import json
from pathlib import Path

import pytest

from ctx.builtins import prepare_checkout


def _trust_file(config_dir: Path) -> Path:
    return config_dir / ".claude.json"


def test_prepare_claude_trusts_the_checkout(isolated_claude_config: Path, tmp_path: Path) -> None:
    checkout = tmp_path / "checkout"

    prepare_checkout("claude", checkout)

    config = json.loads(_trust_file(isolated_claude_config).read_text())
    assert config["projects"][str(checkout)]["hasTrustDialogAccepted"] is True


def test_prepare_claude_keeps_the_rest_of_the_config(
    isolated_claude_config: Path, tmp_path: Path
) -> None:
    file = _trust_file(isolated_claude_config)
    file.write_text(
        json.dumps({"theme": "dark", "projects": {"/other": {"hasTrustDialogAccepted": True}}})
    )

    prepare_checkout("claude", tmp_path / "checkout")

    config = json.loads(file.read_text())
    assert config["theme"] == "dark"
    assert config["projects"]["/other"] == {"hasTrustDialogAccepted": True}
    assert config["projects"][str(tmp_path / "checkout")]["hasTrustDialogAccepted"] is True


def test_prepare_claude_respects_a_recorded_answer(
    isolated_claude_config: Path, tmp_path: Path
) -> None:
    checkout = tmp_path / "checkout"
    file = _trust_file(isolated_claude_config)
    file.write_text(json.dumps({"projects": {str(checkout): {"hasTrustDialogAccepted": False}}}))

    prepare_checkout("claude", checkout)

    config = json.loads(file.read_text())
    assert config["projects"][str(checkout)]["hasTrustDialogAccepted"] is False


def test_prepare_claude_leaves_a_malformed_config_alone(
    isolated_claude_config: Path, tmp_path: Path
) -> None:
    file = _trust_file(isolated_claude_config)
    file.write_text("not json")

    prepare_checkout("claude", tmp_path / "checkout")

    assert file.read_text() == "not json"


def test_prepare_claude_keeps_the_config_file_permissions(
    isolated_claude_config: Path, tmp_path: Path
) -> None:
    file = _trust_file(isolated_claude_config)
    file.write_text("{}")
    file.chmod(0o600)

    prepare_checkout("claude", tmp_path / "checkout")

    assert file.stat().st_mode & 0o777 == 0o600


def test_prepare_claude_defaults_to_the_home_config(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.delenv("CLAUDE_CONFIG_DIR")
    monkeypatch.setenv("HOME", str(home))

    prepare_checkout("claude", tmp_path / "checkout")

    assert (home / ".claude.json").exists()


def test_prepare_checkout_without_a_preparer_is_a_noop(
    isolated_claude_config: Path, tmp_path: Path
) -> None:
    prepare_checkout("unknown", tmp_path / "checkout")

    assert not _trust_file(isolated_claude_config).exists()
