//! Small, pure text formatters shared by views.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// "just now", "5 minutes ago", "yesterday", "3 weeks ago", … for a Unix timestamp.
pub fn relative_time(now_secs: i64, then_secs: i64) -> String {
    let delta = now_secs - then_secs;
    if delta < 0 {
        return "in the future".to_owned();
    }
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;

    let plural = |count: i64, unit: &str| {
        if count == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{count} {unit}s ago")
        }
    };
    match delta {
        d if d < MINUTE => "just now".to_owned(),
        d if d < HOUR => plural(d / MINUTE, "minute"),
        d if d < DAY => plural(d / HOUR, "hour"),
        d if d < 2 * DAY => "yesterday".to_owned(),
        d if d < WEEK => plural(d / DAY, "day"),
        d if d < MONTH => plural(d / WEEK, "week"),
        d if d < YEAR => plural(d / MONTH, "month"),
        d => plural(d / YEAR, "year"),
    }
}

/// Seconds since the Unix epoch for `time` (0 if before it).
pub fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Wall-clock `HH:MM:SS` for `time`, shifted by the local UTC offset.
pub fn clock_time(time: SystemTime, utc_offset_secs: i64) -> String {
    let local = (unix_seconds(time) + utc_offset_secs).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        local / 3600,
        local / 60 % 60,
        local % 60
    )
}

/// Compact duration: `12 ms`, `1.24 s`, `2 min 05 s`.
pub fn duration_label(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis < 1000 {
        format!("{millis} ms")
    } else if millis < 60_000 {
        format!("{:.2} s", duration.as_secs_f64())
    } else {
        let secs = duration.as_secs();
        format!("{} min {:02} s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        let now = 1_700_000_000;
        let cases = [
            (0, "just now"),
            (59, "just now"),
            (60, "1 minute ago"),
            (5 * 60, "5 minutes ago"),
            (3600, "1 hour ago"),
            (23 * 3600, "23 hours ago"),
            (24 * 3600, "yesterday"),
            (3 * 86_400, "3 days ago"),
            (7 * 86_400, "1 week ago"),
            (21 * 86_400, "3 weeks ago"),
            (45 * 86_400, "1 month ago"),
            (200 * 86_400, "6 months ago"),
            (365 * 86_400, "1 year ago"),
            (3 * 365 * 86_400, "3 years ago"),
        ];
        for (ago, expected) in cases {
            assert_eq!(relative_time(now, now - ago), expected, "{ago}s ago");
        }
        assert_eq!(relative_time(now, now + 10), "in the future");
    }

    #[test]
    fn clock_times_respect_the_offset() {
        let midnight_utc = UNIX_EPOCH + Duration::from_secs(1_700_006_400); // 2023-11-15 00:00:00Z
        assert_eq!(clock_time(midnight_utc, 0), "00:00:00");
        assert_eq!(clock_time(midnight_utc, 3600), "01:00:00");
        assert_eq!(clock_time(midnight_utc, -5 * 3600), "19:00:00");
        assert_eq!(
            clock_time(midnight_utc + Duration::from_secs(3723), 0),
            "01:02:03"
        );
    }

    #[test]
    fn duration_labels() {
        assert_eq!(duration_label(Duration::from_millis(0)), "0 ms");
        assert_eq!(duration_label(Duration::from_millis(12)), "12 ms");
        assert_eq!(duration_label(Duration::from_millis(1240)), "1.24 s");
        assert_eq!(duration_label(Duration::from_secs(125)), "2 min 05 s");
    }
}
