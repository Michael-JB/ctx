"""Forge integrations: the PR-hosting service behind a checkout's remote."""

import re

# Opens the current branch's PR in the browser via the forge's CLI, run in
# the checkout; the CLI resolves the PR (or MR) and its URL itself.
_GITHUB_PR_VIEW = ("gh", "pr", "view", "--web")
_GITLAB_PR_VIEW = ("glab", "mr", "view", "--web")


def pr_view_command(remote_url: str) -> list[str]:
    """The PR-opening invocation for a checkout, derived from its remote URL.

    The forge is per repo, not per installation, so it is read off the
    remote's host: GitLab hosts get glab, anything else defaults to gh.
    """
    return list(_GITLAB_PR_VIEW if "gitlab" in _host(remote_url) else _GITHUB_PR_VIEW)


def _host(url: str) -> str:
    """The host of a remote URL, tolerating ssh/https/scp and local forms."""
    rest = url.split("://", 1)[-1].split("@", 1)[-1]
    return re.split(r"[/:]", rest, maxsplit=1)[0].lower()
