//! Timestamps as the archiver wants them.

use chrono::{DateTime, Utc};

/// `t` (UTC epoch seconds) as `YYYY-MM-DDTHH:MM:SS.mmmZ`, rounded to the
/// millisecond. Non-finite or out-of-range times are clamped to the epoch
/// and to year 9999.
pub fn format_time(t: f64) -> String {
    const MAX_MS: f64 = 253_402_300_799_999.0; // 9999-12-31T23:59:59.999Z
    let ms = if t.is_finite() {
        (t * 1000.0).round().clamp(0.0, MAX_MS)
    } else if t > 0.0 {
        MAX_MS
    } else {
        0.0
    };
    DateTime::<Utc>::from_timestamp_millis(ms as i64)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// An archiver timestamp (`Instant.parse` / ISO offset forms, e.g.
/// `2024-01-02T03:04:05.123Z` or `2024-01-02T03:04:05-07:00`) as epoch seconds.
pub fn parse_time(s: &str) -> Option<f64> {
    let t = DateTime::parse_from_rfc3339(s.trim()).ok()?;
    Some(t.timestamp() as f64 + f64::from(t.timestamp_subsec_nanos()) * 1e-9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_millis() {
        assert_eq!(format_time(0.0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_time(1_704_067_200.0), "2024-01-01T00:00:00.000Z");
        assert_eq!(format_time(1_704_067_200.123), "2024-01-01T00:00:00.123Z");
        assert_eq!(format_time(1_704_067_200.999_6), "2024-01-01T00:00:01.000Z");
        assert_eq!(format_time(f64::NAN), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_time(f64::INFINITY), "9999-12-31T23:59:59.999Z");
        assert_eq!(format_time(-5.0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn parses_instant_and_offset_forms() {
        assert_eq!(parse_time("2024-01-01T00:00:00Z"), Some(1_704_067_200.0));
        let t = parse_time("2024-01-01T00:00:00.123456789Z").unwrap();
        assert!((t - 1_704_067_200.123_457).abs() < 1e-6);
        assert_eq!(
            parse_time("2023-12-31T17:00:00-07:00"),
            Some(1_704_067_200.0)
        );
        assert_eq!(parse_time("yesterday"), None);
        assert_eq!(parse_time("2024-01-01T00:00:00"), None);
    }

    #[test]
    fn round_trips() {
        for t in [1_700_000_000.5, 1_234_567_890.001, 86_400.25] {
            assert!((parse_time(&format_time(t)).unwrap() - t).abs() < 1e-3);
        }
    }
}
