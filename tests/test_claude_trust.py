import json
from pathlib import Path

import pytest

from ctx.claude_trust import trust


@pytest.fixture
def config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    config_dir = tmp_path / "claude-config"
    config_dir.mkdir()
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(config_dir))
    return config_dir / ".claude.json"


def test_trust_adds_the_project_entry(config: Path) -> None:
    config.write_text(json.dumps({"numStartups": 3, "projects": {}}, indent=2))

    trust(Path("/w/repo"))

    data = json.loads(config.read_text())
    assert data["projects"]["/w/repo"] == {"hasTrustDialogAccepted": True}
    assert data["numStartups"] == 3


def test_trust_keeps_other_entry_fields(config: Path) -> None:
    config.write_text(json.dumps({"projects": {"/w/repo": {"allowedTools": ["Bash"]}}}))

    trust(Path("/w/repo"))

    entry = json.loads(config.read_text())["projects"]["/w/repo"]
    assert entry == {"allowedTools": ["Bash"], "hasTrustDialogAccepted": True}


def test_trust_creates_a_missing_config(config: Path) -> None:
    trust(Path("/w/repo"))

    data = json.loads(config.read_text())
    assert data == {"projects": {"/w/repo": {"hasTrustDialogAccepted": True}}}


def test_trust_defaults_to_the_home_config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    monkeypatch.setenv("HOME", str(home))

    trust(Path("/w/repo"))

    assert (home / ".claude.json").exists()


def test_trust_respects_a_recorded_answer(config: Path) -> None:
    original = json.dumps({"projects": {"/w/repo": {"hasTrustDialogAccepted": False}}})
    config.write_text(original)

    trust(Path("/w/repo"))

    assert config.read_text() == original


def test_trust_keeps_the_config_file_permissions(config: Path) -> None:
    config.write_text("{}")
    config.chmod(0o600)

    trust(Path("/w/repo"))

    assert config.stat().st_mode & 0o777 == 0o600


def test_trust_leaves_an_unparseable_config_alone(config: Path) -> None:
    config.write_text("{not json")

    trust(Path("/w/repo"))

    assert config.read_text() == "{not json"
