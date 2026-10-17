//! The time axis: ticks on whole steps of local time, adaptive labels, and
//! the span text the range controls read and show. `time_ticks` (made
//! daylight-saving aware) and `parse_span` come from bobview's `trend.rs`.

use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike};

const DAY: f64 = 86_400.0;
const WEEK: f64 = 7.0 * DAY;
/// A month as a step: nominal, for choosing it; its ticks are on the 1st.
const MONTH: f64 = 30.0 * DAY;
/// A year as a step: its ticks are on 1 January.
const YEAR: f64 = 365.0 * DAY;

/// Steps for the time axis, in seconds: whole seconds, minutes, hours,
/// days, weeks (from Mondays), calendar months and years.
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
    DAY,
    2.0 * DAY,
    WEEK,
    2.0 * WEEK,
    MONTH,
    2.0 * MONTH,
    3.0 * MONTH,
    6.0 * MONTH,
    YEAR,
];

/// Ticks from `from` to `to` (seconds since 1970) at the smallest step
/// giving at most `max_ticks`, on whole steps of wall-clock time in `tz`
/// (each tick's own offset, so day lines stay at midnight across a
/// daylight-saving change). Week ticks are on Mondays, month ticks on the
/// 1st and year ticks on 1 January. Returns the ticks and the step.
pub fn time_ticks<Tz: TimeZone>(from: f64, to: f64, max_ticks: usize, tz: &Tz) -> (Vec<f64>, f64) {
    let span = to - from;
    if span <= 0.0 || !span.is_finite() {
        return (Vec::new(), 0.0);
    }
    let max_ticks = max_ticks.max(1) as f64;
    let step = TIME_STEPS
        .iter()
        .copied()
        .find(|s| span / s <= max_ticks)
        .unwrap_or_else(|| YEAR * (span / YEAR / max_ticks).ceil());
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
    if step >= MONTH {
        // The 1st of every `months`th month (counted from January 1970),
        // or 1 January of every `years`th year.
        let months = if step >= YEAR {
            12 * (step / YEAR).round() as i32
        } else {
            (step / MONTH).round() as i32
        };
        let (mut y, mut m) = (local.year(), local.month0() as i32);
        for _ in 0..4000 {
            if ticks.len() >= 1000 {
                break;
            }
            let index = (y - 1970) * 12 + m;
            if index.rem_euclid(months) == 0
                && let Some(d) = NaiveDate::from_ymd_opt(y, m as u32 + 1, 1)
                && let Some(t) = at(d.and_time(NaiveTime::MIN))
            {
                if t > to {
                    break;
                }
                push(&mut ticks, t);
            }
            m += 1;
            if m == 12 {
                (y, m) = (y + 1, 0);
            }
        }
    } else if step >= DAY {
        // Local midnights: every `days`th day counted from 1970-01-01, or
        // for weeks every `days`th day from Monday 1970-01-05.
        let days = (step / DAY).round() as i64;
        let epoch = if step >= WEEK {
            NaiveDate::from_ymd_opt(1970, 1, 5)
        } else {
            NaiveDate::from_ymd_opt(1970, 1, 1)
        }
        .unwrap_or_default();
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
/// month. Week ticks read "13 Oct" (with the year on the first of a
/// year), month ticks "Oct" ("Jan 2027" in January) and year ticks "2027".
/// The `first` tick carries its context ("Tue 17 14:00", "Oct 2026").
pub fn time_label(t: f64, step: f64, first: bool) -> String {
    let Some(d) = Local.timestamp_opt(t.round() as i64, 0).single() else {
        return String::new();
    };
    let midnight = d.hour() == 0 && d.minute() == 0 && d.second() == 0;
    let format = if step < 60.0 {
        if first { "%a %-d %H:%M:%S" } else { "%H:%M:%S" }
    } else if step < DAY {
        if first {
            "%a %-d %H:%M"
        } else if midnight {
            "%a %-d"
        } else {
            "%H:%M"
        }
    } else if step < WEEK {
        if first {
            "%-d %b"
        } else if d.day() == 1 {
            "%b"
        } else {
            "%a %-d"
        }
    } else if step < MONTH {
        // The first tick of a new year carries it.
        let new_year = d.month() == 1 && f64::from(d.day() - 1) * DAY < step;
        if new_year { "%-d %b %Y" } else { "%-d %b" }
    } else if step < YEAR {
        if first || d.month() == 1 {
            "%b %Y"
        } else {
            "%b"
        }
    } else {
        "%Y"
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

/// A span typed into the custom range field, such as `90m`, `1 hour 30 min`,
/// `2 days`, `3w` or `1y`, in seconds (bobview's strip chart `parse_span`).
/// A month is 30 days and a year 365; plain `m` is minutes.
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
            "mo" | "month" => 30.0 * 86_400.0,
            "y" | "yr" | "year" => 365.0 * 86_400.0,
            _ => return None,
        };
        total += n * secs;
        any = true;
    }
    (any && total > 0.0).then_some(total)
}

/// A span as short text: "45s", "3h 20m", "1d", "1w 12h", "1y 3w".
pub fn format_span(secs: f64) -> String {
    let mut s = secs.round().max(1.0) as u64;
    let mut parts = Vec::new();
    for (size, unit) in [
        (365 * 86_400, "y"),
        (7 * 86_400, "w"),
        (86_400, "d"),
        (3600, "h"),
        (60, "m"),
        (1, "s"),
    ] {
        if s >= size {
            parts.push(format!("{}{unit}", s / size));
            s %= size;
        }
    }
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
        // Longer still: calendar months, then years.
        let (_, step) = time_ticks(0.0, 400.0 * 86_400.0, 5, &utc());
        assert_eq!(step, 3.0 * MONTH);
        let (t, step) = time_ticks(0.0, 20.0 * YEAR, 5, &utc());
        assert_eq!(step, 4.0 * YEAR);
        let years: Vec<i32> = t
            .iter()
            .map(|t| DateTime::from_timestamp(*t as i64, 0).unwrap().year())
            .collect();
        assert_eq!(years, [1970, 1974, 1978, 1982, 1986]);
        assert!(time_ticks(5.0, 5.0, 5, &utc()).0.is_empty());
    }

    /// The UTC dates of `ticks`, as `YYYY-MM-DD`.
    fn dates(ticks: &[f64]) -> Vec<String> {
        ticks
            .iter()
            .map(|t| {
                DateTime::from_timestamp(*t as i64, 0)
                    .unwrap()
                    .format("%Y-%m-%d")
                    .to_string()
            })
            .collect()
    }

    fn utc_at(s: &str) -> f64 {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M")
            .unwrap()
            .and_utc()
            .timestamp() as f64
    }

    #[test]
    fn week_month_and_year_ticks_are_on_calendar_boundaries() {
        // 60 days from 20 Aug 2026 over 8 ticks: weekly, on Mondays.
        let from = utc_at("2026-08-20 10:00");
        let (t, step) = time_ticks(from, from + 60.0 * DAY, 9, &utc());
        assert_eq!(step, WEEK);
        assert_eq!(&dates(&t)[..3], ["2026-08-24", "2026-08-31", "2026-09-07"]);
        // ... over 4: months, on the 1st.
        let (t, step) = time_ticks(from, from + 60.0 * DAY, 4, &utc());
        assert_eq!(step, MONTH);
        assert_eq!(dates(&t), ["2026-09-01", "2026-10-01"]);
        // A year over 8 ticks: every two months, from January.
        let from = utc_at("2025-11-08 00:00");
        let (t, step) = time_ticks(from, from + 365.0 * DAY, 8, &utc());
        assert_eq!(step, 2.0 * MONTH);
        assert_eq!(
            dates(&t),
            [
                "2026-01-01",
                "2026-03-01",
                "2026-05-01",
                "2026-07-01",
                "2026-09-01",
                "2026-11-01"
            ]
        );
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
    fn long_spans_name_the_month() {
        // A 60-day span with weekly ticks: "24 Aug", "7 Sep", never "Thu 3".
        let monday = parse_local("2026-09-07").unwrap();
        assert_eq!(time_label(monday, WEEK, false), "7 Sep");
        assert_eq!(time_label(monday, 2.0 * WEEK, true), "7 Sep");
        // The first week tick of a year carries it.
        let jan = parse_local("2027-01-04").unwrap();
        assert_eq!(time_label(jan, WEEK, false), "4 Jan 2027");
        assert_eq!(time_label(jan + 7.0 * DAY, WEEK, false), "11 Jan");
        // A 365-day span with month ticks: "Mar", "Jan 2027", first "Nov 2026".
        let mar = parse_local("2026-03-01").unwrap();
        assert_eq!(time_label(mar, 2.0 * MONTH, false), "Mar");
        let jan = parse_local("2027-01-01").unwrap();
        assert_eq!(time_label(jan, MONTH, false), "Jan 2027");
        assert_eq!(
            time_label(parse_local("2026-11-01").unwrap(), MONTH, true),
            "Nov 2026"
        );
        assert_eq!(time_label(jan, 2.0 * YEAR, false), "2027");
    }

    #[test]
    fn spans_parse_and_print() {
        const D: f64 = 86_400.0;
        // Typed text, its seconds, and how that span prints.
        let table = [
            ("45s", 45.0, "45s"),
            ("1 minute", 60.0, "1m"),
            ("90m", 5400.0, "1h 30m"),
            ("1 hour 30 minutes", 5400.0, "1h 30m"),
            ("12000", 12_000.0, "3h 20m"),
            ("2 days", 2.0 * D, "2d"),
            ("7d", 7.0 * D, "1w"),
            ("1w", 7.0 * D, "1w"),
            ("2 weeks", 14.0 * D, "2w"),
            ("5w 2d", 37.0 * D, "5w 2d"),
            ("7d 12h 1m", 7.5 * D + 60.0, "1w 12h"),
            ("1mo", 30.0 * D, "4w 2d"),
            ("2 months", 60.0 * D, "8w 4d"),
            ("1y", 365.0 * D, "1y"),
            ("1 year 3 weeks", 386.0 * D, "1y 3w"),
            ("2yrs", 730.0 * D, "2y"),
        ];
        for (text, secs, shown) in table {
            assert_eq!(parse_span(text), Some(secs), "{text}");
            assert_eq!(format_span(secs), shown, "{text}");
            // What prints parses back, to within the units dropped.
            let back = parse_span(shown).unwrap();
            assert!(back <= secs && secs - back < D, "{text}: {shown}");
        }
        assert_eq!(parse_span("soon"), None);
        assert_eq!(parse_span(""), None);
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
