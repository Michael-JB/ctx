//! Human-readable "how long ago" wording.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The local calendar date (year, month, day) that `when` falls on.
fn local_date(when: SystemTime) -> Option<(i64, i64, i64)> {
    let secs = when.duration_since(UNIX_EPOCH).ok()?.as_secs() as libc::time_t;
    // SAFETY: `tm` is a plain C struct for which all-zero is a valid value,
    // and localtime_r only writes into the buffer we pass it.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return None;
        }
        tm
    };
    Some((
        i64::from(tm.tm_year) + 1900,
        i64::from(tm.tm_mon) + 1,
        i64::from(tm.tm_mday),
    ))
}

fn ago(count: u64, unit: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

/// How long before `now` `when` was: "Just now", then minutes, hours,
/// days, weeks (up to 30 days) and months (30-day, up to a year) ago, then
/// the local ISO date. Empty when that date cannot be determined.
pub fn relative_time(when: SystemTime, now: SystemTime) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;
    let elapsed = now.duration_since(when).unwrap_or(Duration::ZERO).as_secs();
    match elapsed {
        0..MINUTE => "Just now".to_string(),
        MINUTE..HOUR => ago(elapsed / MINUTE, "minute"),
        HOUR..DAY => ago(elapsed / HOUR, "hour"),
        DAY..WEEK => ago(elapsed / DAY, "day"),
        WEEK..MONTH => ago(elapsed / WEEK, "week"),
        MONTH..YEAR => ago(elapsed / MONTH, "month"),
        _ => local_date(when)
            .map(|(year, month, day)| format!("{year:04}-{month:02}-{day:02}"))
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);
    const HOUR: Duration = Duration::from_secs(60 * 60);
    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    #[test]
    fn elapsed_time_is_worded_by_its_largest_unit() {
        let now = SystemTime::now();
        for (back, shown) in [
            (Duration::ZERO, "Just now"),
            (59 * Duration::from_secs(1), "Just now"),
            (MINUTE, "1 minute ago"),
            (59 * MINUTE, "59 minutes ago"),
            (HOUR, "1 hour ago"),
            (23 * HOUR, "23 hours ago"),
            (DAY, "1 day ago"),
            (6 * DAY, "6 days ago"),
            (7 * DAY, "1 week ago"),
            (29 * DAY, "4 weeks ago"),
            (30 * DAY, "1 month ago"),
            (364 * DAY, "12 months ago"),
        ] {
            assert_eq!(relative_time(now - back, now), shown);
        }
    }

    #[test]
    fn a_year_or_more_back_is_an_iso_date() {
        let now = SystemTime::now();
        let then = now - 365 * DAY;
        let (year, month, day) = local_date(then).unwrap();
        assert_eq!(
            relative_time(then, now),
            format!("{year:04}-{month:02}-{day:02}")
        );
    }

    #[test]
    fn the_future_reads_as_just_now() {
        let now = SystemTime::now();
        assert_eq!(relative_time(now + HOUR, now), "Just now");
    }
}
