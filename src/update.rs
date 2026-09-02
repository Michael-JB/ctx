//! Update check: the newest release of this crate on crates.io.

use crate::git::new_command;

pub const CRATE: &str = env!("CARGO_PKG_NAME");
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

type Version = (u64, u64, u64);

/// The crate's file in the crates.io sparse index.
fn index_url() -> String {
    format!(
        "https://index.crates.io/{}/{}/{CRATE}",
        &CRATE[..2],
        &CRATE[2..4]
    )
}

/// A release as a numeric triple; pre-releases and odd forms don't count.
fn parse(version: &str) -> Option<Version> {
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    let parsed = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(parsed)
}

/// The newest unyanked release in the index file (one JSON entry per line).
fn newest(index: &str) -> Option<String> {
    index
        .lines()
        .filter_map(|line| {
            let entry: serde_json::Value = serde_json::from_str(line).ok()?;
            if entry["yanked"].as_bool() == Some(true) {
                return None;
            }
            let version = entry["vers"].as_str()?;
            Some((parse(version)?, version.to_string()))
        })
        .max()
        .map(|(_, version)| version)
}

fn fetch_index() -> Option<String> {
    let output = new_command("curl")
        .args(["-fsSL", "--max-time", "5", &index_url()])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The newest published version, if it is newer than this build.
pub fn available() -> Option<String> {
    let latest = newest(&fetch_index()?)?;
    (parse(&latest)? > parse(CURRENT)?).then_some(latest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::test_env;

    fn entry(version: &str, yanked: bool) -> String {
        format!("{{\"name\":\"{CRATE}\",\"vers\":\"{version}\",\"yanked\":{yanked}}}")
    }

    #[test]
    fn parses_release_triples_only() {
        assert_eq!(parse("2.0.3"), Some((2, 0, 3)));
        assert_eq!(parse("10.2.0"), Some((10, 2, 0)));
        assert_eq!(parse("2.0.3-rc.1"), None);
        assert_eq!(parse("2.0"), None);
        assert_eq!(parse("2.0.3.4"), None);
    }

    #[test]
    fn newest_skips_yanked_and_garbage_lines() {
        let index = [
            entry("1.9.0", false),
            entry("2.0.0", false),
            entry("3.0.0", true),
            entry("2.1.0-beta", false),
            "not json".to_string(),
        ]
        .join("\n");

        assert_eq!(newest(&index).as_deref(), Some("2.0.0"));
        assert_eq!(newest(""), None);
    }

    #[test]
    fn index_url_is_the_sparse_index_path() {
        assert_eq!(index_url(), "https://index.crates.io/ct/x-/ctx-tui");
    }

    #[test]
    fn available_reports_a_newer_release() {
        let env = test_env();
        let _curl = env.fake_cli("curl", &format!("echo '{}'", entry("999.0.0", false)));

        assert_eq!(available().as_deref(), Some("999.0.0"));
    }

    #[test]
    fn available_is_quiet_when_current_or_offline() {
        let env = test_env();
        {
            let _curl = env.fake_cli("curl", &format!("echo '{}'", entry(CURRENT, false)));
            assert_eq!(available(), None);
        }
        let _curl = env.fake_cli("curl", "exit 22");
        assert_eq!(available(), None);
    }
}
