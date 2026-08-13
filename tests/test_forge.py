import pytest

from ctx import forge


@pytest.mark.parametrize(
    "url",
    [
        "git@github.com:jane/tool.git",
        "https://github.com/jane/tool.git",
        "ssh://git@github.com/jane/tool.git",
        "/local/mirrors/tool.git",
    ],
)
def test_non_gitlab_remotes_default_to_gh(url: str) -> None:
    assert forge.pr_view_command(url) == ["gh", "pr", "view", "--web"]


@pytest.mark.parametrize(
    "url",
    [
        "git@gitlab.com:jane/tool.git",
        "https://gitlab.example.com/jane/tool.git",
        "ssh://git@gitlab.com/jane/tool.git",
    ],
)
def test_gitlab_remotes_use_glab(url: str) -> None:
    assert forge.pr_view_command(url) == ["glab", "mr", "view", "--web"]
