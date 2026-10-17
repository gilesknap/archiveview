//! The time axis: ticks on whole steps of local time, adaptive labels, and
//! the span text the range controls read and show. `time_ticks` (made
//! daylight-saving aware) and `parse_span` come from bobview's `trend.rs`.

use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike};

/// Steps for the time axis, in seconds: whole seconds, minutes, hours and
/// days.
const TIME_STEPS: &[f64] = &[
    1.0,
    2.0,
    5.0,
    10.0,
    15.0,
    30.0,
    60.0,
    120.0,
    300.0,
    600.0,
    900.0,
    1800.0,
    3600.0,
    7200.0,
    10_800.0,
    21_600.0,
    43_200.0,
    86_400.0,
    2.0 * 86_400.0,
    7.0 * 86_400.0,
    14.0 * 86_400.0,
    30.0 * 86_400.0,
];

/// Ticks from `from` to `to` (seconds since 1970) at the smallest step
/// giving at most `max_ticks`, on whole steps of wall-clock time in `tz`
/// (each tick's own offset, so day lines stay at midnight across a
/// daylight-saving change). Returns the ticks and the step.
pub fn time_ticks<Tz: TimeZone>(from: f64, to: f64, max_ticks: usize, tz: &Tz) -> (Vec<f64>, f64) {
    let span = to - from;
    if span <= 0.0 || !span.is_finite() {
        return (Vec::new(), 0.0);
    }
    let step = TIME_STEPS
        .iter()
        .copied()
        .find(|s| span / s <= max_ticks.max(1) as f64)
        .unwrap_or_else(|| 86_400.0 * (span / 86_400.0 / max_ticks.max(1) as f64).ceil());
    let Some(start) = DateTime::from_timestamp(from.floor() as i64, 0) else {
        return (Vec::new(), step);
    };
    let local = start.with_timezone(tz).naive_local();
    // The wall-clock time `n` and its instant (the earlier of a repeated
    // hour; none in the hour skipped in spring).
    let at = |n: NaiveDateTime| {
        tz.from_local_datetime(&n)
            .earliest()
            .map(|d| d.timestamp() as f64)
    };
    let mut ticks: Vec<f64> = Vec::new();
    let push = |ticks: &mut Vec<f64>, t: f64| {
        if t >= from && t <= to && ticks.last().is_none_or(|l| t > *l) {
            ticks.push(t);
        }
    };
    if step >= 86_400.0 {
        // Local midnights, every `days` days counted from 1970-01-01.
        let days = (step / 86_400.0).round() as i64;
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default();
        let mut date = local.date();
        for _ in 0..4000 {
            let Some(t) = at(date.and_time(NaiveTime::MIN)) else {
                date = date.succ_opt().unwrap_or(date);
                continue;
            };
            if t > to || ticks.len() >= 1000 {
                break;
            }
            if (date - epoch).num_days().rem_euclid(days) == 0 {
                push(&mut ticks, t);
            }
            match date.succ_opt() {
                Some(d) => date = d,
                None => break,
            }
        }
    } else {
        // Whole steps of the local day (every step divides a day).
        let step_s = step as i64;
        let into_day = local.time().num_seconds_from_midnight() as i64;
        let mut n = local.date().and_time(NaiveTime::MIN)
            + chrono::Duration::seconds(into_day / step_s * step_s);
        for _ in 0..4000 {
            if let Some(t) = at(n) {
                if t > to || ticks.len() >= 1000 {
                    break;
                }
                push(&mut ticks, t);
            }
            n += chrono::Duration::seconds(step_s);
        }
    }
    (ticks, step)
}

/// A time axis label in local time, as precise as the step: "14:30:15",
/// "14:30", "Tue 17" at midnight or for day steps, "Oct" at the start of a
/// month. The `first` tick carries its context ("Tue 17 14:00").
pub fn time_label(t: f64, step: f64, first: bool) -> String {
    let Some(d) = Local.timestamp_opt(t.round() as i64, 0).single() else {
        return String::new();
    };
    let midnight = d.hour() == 0 && d.minute() == 0 && d.second() == 0;
    let format = if step < 60.0 {
        if first { "%a %-d %H:%M:%S" } else { "%H:%M:%S" }
    } else if step < 86_400.0 {
        if first {
            "%a %-d %H:%M"
        } else if midnight {
            "%a %-d"
        } else {
            "%H:%M"
        }
    } else if first {
        "%-d %b"
    } else if d.day() == 1 {
        "%b"
    } else {
        "%a %-d"
    };
    d.format(format).to_string()
}

/// A time in full for the crosshair readout: "Tue 17 Oct 14:32:05.250"
/// (milliseconds only when the span shown is under a minute).
pub fn readout_time(t: f64, span: f64) -> String {
    let ms = (t * 1000.0).round() as i64;
    let Some(d) = Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    let format = if span < 60.0 {
        "%a %-d %b %H:%M:%S%.3f"
    } else {
        "%a %-d %b %H:%M:%S"
    };
    d.format(format).to_string()
}

/// A time for the crosshair's tag on the time axis: "14:32:05", with the
/// day ("Tue 17 14:32") over spans of a day or more, and milliseconds
/// over spans under a minute.
pub fn tag_time(t: f64, span: f64) -> String {
    let ms = (t * 1000.0).round() as i64;
    let Some(d) = Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    let format = if span < 60.0 {
        "%H:%M:%S%.3f"
    } else if span < 86_400.0 {
        "%H:%M:%S"
    } else {
        "%a %-d %H:%M"
    };
    d.format(format).to_string()
}

/// A span typed into the custom range field, such as `90m`, `1 hour 30 min`
/// or `2 days`, in seconds (bobview's strip chart `parse_span`).
pub fn parse_span(text: &str) -> Option<f64> {
    let mut total = 0.0;
    let mut words = text.split_whitespace().peekable();
    let mut any = false;
    while let Some(word) = words.next() {
        // A number and its unit, apart (`10 min`) or together (`10min`).
        let split = word
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(word.len());
        let (num, unit) = word.split_at(split);
        let n: f64 = num.parse().ok()?;
        let unit = if unit.is_empty() {
            words.next().unwrap_or("s")
        } else {
            unit
        };
        let secs = match unit.to_ascii_lowercase().trim_end_matches('s') {
            "" | "sec" | "second" => 1.0,
            "m" | "min" | "minute" => 60.0,
            "h" | "hr" | "hour" => 3600.0,
            "d" | "day" => 86_400.0,
            "w" | "wk" | "week" => 7.0 * 86_400.0,
            _ => return None,
        };
        total += n * secs;
        any = true;
    }
    (any && total > 0.0).then_some(total)
}

/// A span as short text: "45s", "3h 20m", "1d", "7d 12h".
pub fn format_span(secs: f64) -> String {
    let s = secs.round().max(1.0) as u64;
    let (d, h, m, sec) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60, s % 60);
    let parts: Vec<String> = [(d, "d"), (h, "h"), (m, "m"), (sec, "s")]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, u)| format!("{n}{u}"))
        .collect();
    // The two largest units are plenty.
    parts.into_iter().take(2).collect::<Vec<_>>().join(" ")
}

/// A local date and time as typed: `2026-10-17 14:30`, with optional
/// seconds, or a `T` between date and time; or just a date (midnight).
pub fn parse_local(text: &str) -> Option<f64> {
    let text = text.trim().replace('T', " ");
    let naive = ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(&text, f).ok())
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(&text, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
        })?;
    let local = Local.from_local_datetime(&naive).earliest()?;
    Some(local.timestamp() as f64)
}

/// `t` as local `YYYY-MM-DD HH:MM:SS`, as [`parse_local`] reads it.
pub fn format_local(t: f64) -> String {
    Local
        .timestamp_opt(t.floor() as i64, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{FixedOffset, LocalResult, Offset};

    /// UTC, or an hour ahead of it, as `FixedOffset`s.
    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    /// A UK-like zone: GMT, then BST (an hour ahead) from `change`
    /// (seconds since 1970) on: clocks go 01:00 -> 02:00.
    #[derive(Clone, Copy)]
    struct Spring {
        change: i64,
    }

    impl TimeZone for Spring {
        type Offset = FixedOffset;
        fn from_offset(_: &FixedOffset) -> Self {
            unreachable!()
        }
        fn offset_from_local_date(&self, _: &NaiveDate) -> LocalResult<FixedOffset> {
            unreachable!()
        }
        fn offset_from_local_datetime(&self, l: &NaiveDateTime) -> LocalResult<FixedOffset> {
            let as_utc = l.and_utc().timestamp();
            let (gmt, bst) = (utc(), FixedOffset::east_opt(3600).unwrap());
            if as_utc < self.change {
                LocalResult::Single(gmt)
            } else if as_utc < self.change + 3600 {
                LocalResult::None
            } else {
                LocalResult::Single(bst)
            }
        }
        fn offset_from_utc_date(&self, _: &NaiveDate) -> FixedOffset {
            unreachable!()
        }
        fn offset_from_utc_datetime(&self, u: &NaiveDateTime) -> FixedOffset {
            let off = if u.and_utc().timestamp() < self.change {
                0
            } else {
                3600
            };
            FixedOffset::east_opt(off).unwrap().fix()
        }
    }

    #[test]
    fn time_ticks_fall_on_whole_local_steps() {
        let (t, step) = time_ticks(1000.0, 1600.0, 6, &utc());
        assert_eq!(step, 120.0);
        assert_eq!(t, [1080.0, 1200.0, 1320.0, 1440.0, 1560.0]);
        // Half an hour ahead of UTC, hours are whole in local time.
        let half = FixedOffset::east_opt(1800).unwrap();
        let (t, step) = time_ticks(0.0, 4.0 * 3600.0, 4, &half);
        assert_eq!(step, 3600.0);
        assert_eq!(t[0], 1800.0);
        // A week over 4 ticks: 2-day steps; 60 days over 3: 30-day steps.
        assert_eq!(time_ticks(0.0, 7.0 * 86_400.0, 4, &utc()).1, 2.0 * 86_400.0);
        assert_eq!(
            time_ticks(0.0, 60.0 * 86_400.0, 3, &utc()).1,
            30.0 * 86_400.0
        );
        // Longer still: whole days.
        let (_, step) = time_ticks(0.0, 400.0 * 86_400.0, 5, &utc());
        assert_eq!(step, 80.0 * 86_400.0);
        assert!(time_ticks(5.0, 5.0, 5, &utc()).0.is_empty());
    }

    #[test]
    fn day_ticks_stay_at_midnight_across_a_clock_change() {
        // Clocks go forward at 01:00 GMT on day 10.
        let change = 10 * 86_400 + 3600;
        let tz = Spring { change };
        let (t, step) = time_ticks(5.0 * 86_400.0, 15.0 * 86_400.0, 12, &tz);
        assert_eq!(step, 86_400.0);
        // Midnights before the change are at 00:00 UTC, after it at 23:00
        // UTC the day before (00:00 BST): never an hour off in local time.
        assert_eq!(t[0], 5.0 * 86_400.0);
        assert!(t.contains(&(10.0 * 86_400.0)));
        assert!(t.contains(&(11.0 * 86_400.0 - 3600.0)));
        assert!(!t.contains(&(11.0 * 86_400.0)));
        // Hour ticks skip the missing hour and stay whole.
        let (t, step) = time_ticks(change as f64 - 7200.0, change as f64 + 7200.0, 6, &tz);
        assert_eq!(step, 3600.0);
        assert!(t.windows(2).all(|w| w[1] - w[0] == 3600.0), "{t:?}");
    }

    #[test]
    fn labels_adapt_to_the_step() {
        let t = parse_local("2026-10-17 14:30").unwrap();
        assert_eq!(time_label(t, 1800.0, false), "14:30");
        assert_eq!(time_label(t + 15.0, 5.0, false), "14:30:15");
        assert_eq!(time_label(t, 1800.0, true), "Sat 17 14:30");
        let midnight = parse_local("2026-10-18").unwrap();
        assert_eq!(time_label(midnight, 3600.0, false), "Sun 18");
        assert_eq!(time_label(midnight, 86_400.0, false), "Sun 18");
        let month = parse_local("2026-11-01").unwrap();
        assert_eq!(time_label(month, 86_400.0, false), "Nov");
        assert_eq!(time_label(month, 86_400.0, true), "1 Nov");
    }

    #[test]
    fn spans_parse_and_print() {
        assert_eq!(parse_span("1 minute"), Some(60.0));
        assert_eq!(parse_span("90m"), Some(5400.0));
        assert_eq!(parse_span("1 hour 30 minutes"), Some(5400.0));
        assert_eq!(parse_span("2 days"), Some(172_800.0));
        assert_eq!(parse_span("1w"), Some(604_800.0));
        assert_eq!(parse_span("soon"), None);
        assert_eq!(parse_span(""), None);
        assert_eq!(format_span(3600.0), "1h");
        assert_eq!(format_span(12_000.0), "3h 20m");
        assert_eq!(format_span(7.0 * 86_400.0 + 43_200.0 + 61.0), "7d 12h");
        assert_eq!(format_span(45.0), "45s");
    }

    #[test]
    fn local_times_round_trip() {
        let t = parse_local("2026-10-17 09:05:30").unwrap();
        assert_eq!(format_local(t), "2026-10-17 09:05:30");
        assert_eq!(parse_local("2026-10-17T09:05:30"), Some(t));
        assert_eq!(parse_local("2026-10-17 09:05"), Some(t - 30.0));
        assert!(parse_local("yesterday").is_none());
    }
}
