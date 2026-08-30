//! Forge integrations: the PR-hosting service behind a checkout's remote.

// Opens the current branch's PR in the browser via the forge's CLI, run in
// the checkout; the CLI resolves the PR (or MR) and its URL itself.
const GITHUB_PR_VIEW: &[&str] = &["gh", "pr", "view", "--web"];
const GITLAB_PR_VIEW: &[&str] = &["glab", "mr", "view", "--web"];

/// The PR-opening invocation for a checkout, derived from its remote URL.
///
/// The forge is per repo, not per installation, so it is read off the
/// remote's host: GitLab hosts get glab, anything else defaults to gh.
pub fn pr_view_command(remote_url: &str) -> Vec<String> {
    let command = if host(remote_url).contains("gitlab") {
        GITLAB_PR_VIEW
    } else {
        GITHUB_PR_VIEW
    };
    command.iter().map(|s| s.to_string()).collect()
}

/// The host of a remote URL, tolerating ssh/https/scp and local forms.
fn host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split_once('@').map_or(rest, |(_, r)| r);
    rest.split(['/', ':'])
        .next()
        .unwrap_or_default()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_gitlab_remotes_default_to_gh() {
        for url in [
            "git@github.com:jane/tool.git",
            "https://github.com/jane/tool.git",
            "ssh://git@github.com/jane/tool.git",
            "/local/mirrors/tool.git",
        ] {
            assert_eq!(pr_view_command(url), ["gh", "pr", "view", "--web"]);
        }
    }

    #[test]
    fn gitlab_remotes_use_glab() {
        for url in [
            "git@gitlab.com:jane/tool.git",
            "https://gitlab.example.com/jane/tool.git",
            "ssh://git@gitlab.com/jane/tool.git",
        ] {
            assert_eq!(pr_view_command(url), ["glab", "mr", "view", "--web"]);
        }
    }
}
