//! Whether a newer ctx than this build has been released. Nothing here
//! installs anything; the TUI only shows the newer version.

use crate::git::git_quiet;

type Version = (u64, u64, u64);

/// "X.Y.Z" as a comparable triple; anything else (pre-releases included)
/// is None.
fn parse_version(text: &str) -> Option<Version> {
    let mut parts = text.split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(version)
}

/// The highest release tag (vX.Y.Z) at `remote`, as "X.Y.Z".
fn latest_release(remote: &str) -> Option<String> {
    let output = git_quiet(&["ls-remote", "--tags", "--refs", remote], None).ok()?;
    output
        .lines()
        .filter_map(|line| line.split_once("refs/tags/v"))
        .filter_map(|(_, tag)| parse_version(tag).map(|version| (version, tag.to_string())))
        .max()
        .map(|(_, tag)| tag)
}

/// The version of a release newer than this build, if one is published.
pub fn available_update() -> Option<String> {
    let latest = latest_release(env!("CARGO_PKG_REPOSITORY"))?;
    (parse_version(&latest)? > parse_version(env!("CARGO_PKG_VERSION"))?).then_some(latest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{git, test_env};

    #[test]
    fn the_highest_release_tag_wins() {
        let env = test_env();
        let repo = env.make_origin("upstream", false);
        for tag in ["v1.2.3", "v1.10.0", "v2.0.0-rc1", "unrelated"] {
            git(&["tag", tag], &repo);
        }

        let latest = latest_release(&repo.to_string_lossy());

        assert_eq!(latest, Some("1.10.0".to_string()));
    }
}
