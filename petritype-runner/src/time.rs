//! ISO 8601 timestamps in UTC with millisecond precision, written by hand.
//!
//! The protocol's times are strings such as `"2026-10-03T09:35:02.413Z"`
//! (PROTOCOL.md §2). One function is all the runner needs, so it is written
//! here rather than pulled in with a date crate.

use std::time::{SystemTime, UNIX_EPOCH};

/// `time` as `YYYY-MM-DDTHH:MM:SS.mmmZ`, in UTC.
///
/// A time before the Unix epoch is printed as the epoch: no clock the runner
/// is handed should be there, and a wire string must still parse.
pub(crate) fn iso8601(time: SystemTime) -> String {
    let millis = time
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0);
    let millis = u64::try_from(millis).unwrap_or(u64::MAX);
    let (days, rest) = (millis / 86_400_000, millis % 86_400_000);
    let (year, month, day) = civil_from_days(days);
    let (hour, rest) = (rest / 3_600_000, rest % 3_600_000);
    let (minute, rest) = (rest / 60_000, rest % 60_000);
    let (second, milli) = (rest / 1000, rest % 1000);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milli:03}Z")
}

/// The proleptic Gregorian date `days` after 1970-01-01.
///
/// Howard Hinnant's `civil_from_days`, restricted to days on or after the
/// epoch: the calendar is counted in 400-year eras of 146 097 days, each
/// starting on 1 March so that the leap day is the last day of its year.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    // Shift the origin from 1970-01-01 to 0000-03-01.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097; // day of era, 0..=146_096
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // 0..=399
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of the March year
    let mp = (5 * doy + 2) / 153; // month counted from March, 0..=11
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::iso8601;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(millis: u64) -> String {
        iso8601(UNIX_EPOCH + Duration::from_millis(millis))
    }

    #[test]
    fn the_epoch() {
        assert_eq!(at(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn known_instants() {
        // Each checked against `date -u` and Python's `datetime`.
        assert_eq!(at(1_791_020_102_413), "2026-10-03T09:35:02.413Z");
        assert_eq!(at(1_759_486_502_413), "2025-10-03T10:15:02.413Z");
        assert_eq!(at(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(at(1_709_251_199_999), "2024-02-29T23:59:59.999Z");
        assert_eq!(at(1_709_251_200_000), "2024-03-01T00:00:00.000Z");
        assert_eq!(at(4_107_542_400_000), "2100-03-01T00:00:00.000Z");
        assert_eq!(at(1_735_689_599_001), "2024-12-31T23:59:59.001Z");
    }

    #[test]
    fn before_the_epoch_is_the_epoch() {
        assert_eq!(
            iso8601(UNIX_EPOCH - Duration::from_secs(5)),
            "1970-01-01T00:00:00.000Z"
        );
    }
}
