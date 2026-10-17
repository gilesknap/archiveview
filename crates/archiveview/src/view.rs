//! A view: what a plot shows. Saved as JSON in
//! `~/.config/archiveview/views/<name>.json` and returned by
//! [`crate::Plot::view`] so an embedding app (bobview) can store it in its
//! own layouts.

use serde::{Deserialize, Serialize};

/// The view file format's version. Files with a newer one are refused.
pub const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub version: u32,
    /// The traces, in legend order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pvs: Vec<TraceView>,
    #[serde(default)]
    pub range: Range,
    #[serde(default)]
    pub y: YAxis,
}

impl Default for View {
    fn default() -> Self {
        Self {
            version: VERSION,
            pvs: Vec::new(),
            range: Range::default(),
            y: YAxis::default(),
        }
    }
}

/// One trace of a view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceView {
    /// As typed: `X`, `ca://X`, `pva://X`, `sim://sine`, ...
    pub name: String,
    /// `#rrggbb`; the palette colour for its position when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
}

/// The time range shown.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Range {
    /// The last `secs` seconds, following now (live).
    Relative { secs: f64 },
    /// A fixed window, as RFC 3339 UTC timestamps
    /// (`2026-10-17T09:00:00Z`).
    Absolute { start: String, end: String },
}

impl Default for Range {
    fn default() -> Self {
        Range::Relative { secs: 3600.0 }
    }
}

/// The value axis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct YAxis {
    #[serde(default = "yes")]
    pub autoscale: bool,
    /// The fixed range when not autoscaling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub log: bool,
}

impl Default for YAxis {
    fn default() -> Self {
        Self {
            autoscale: true,
            min: None,
            max: None,
            log: false,
        }
    }
}

/// `t` (seconds since 1970) as RFC 3339 UTC, as views store absolute
/// ranges: `2026-10-17T09:00:00Z`, with milliseconds when it has them.
pub fn format_time(t: f64) -> String {
    let ms = (t * 1000.0).round() as i64;
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
        .unwrap_or_default()
}

/// An RFC 3339 time (any offset) as seconds since 1970.
pub fn parse_time(text: &str) -> Option<f64> {
    let d = chrono::DateTime::parse_from_rfc3339(text.trim()).ok()?;
    Some(d.timestamp() as f64 + d.timestamp_subsec_nanos() as f64 * 1e-9)
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_serialise_compactly() {
        let v = View {
            pvs: vec![
                TraceView {
                    name: "SR-DI-DCCT-01:SIGNAL".into(),
                    color: None,
                    visible: true,
                },
                TraceView {
                    name: "bl01t:SUM".into(),
                    color: Some("#d62728".into()),
                    visible: false,
                },
            ],
            ..View::default()
        };
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "version": 1,
                "pvs": [
                    { "name": "SR-DI-DCCT-01:SIGNAL" },
                    { "name": "bl01t:SUM", "color": "#d62728", "visible": false }
                ],
                "range": { "kind": "relative", "secs": 3600.0 },
                "y": { "autoscale": true }
            })
        );
        let back: View = serde_json::from_value(json).unwrap();
        assert_eq!(back, v);
        // Old or partial files load.
        let min: View = serde_json::from_str(r#"{"version":1}"#).unwrap();
        assert_eq!(min, View::default());
        let abs: View = serde_json::from_str(
            r#"{"version":1,"range":{"kind":"absolute","start":"2026-10-17T09:00:00Z","end":"2026-10-17T12:00:00Z"}}"#,
        )
        .unwrap();
        assert!(matches!(abs.range, Range::Absolute { .. }));
    }

    #[test]
    fn times_round_trip() {
        let t = parse_time("2026-10-17T09:00:00Z").unwrap();
        assert_eq!(format_time(t), "2026-10-17T09:00:00Z");
        assert_eq!(format_time(t + 0.25), "2026-10-17T09:00:00.250Z");
        assert_eq!(parse_time("2026-10-17T10:00:00+01:00"), Some(t));
        assert_eq!(parse_time("2026-10-17T09:00:00.250Z"), Some(t + 0.25));
        assert_eq!(parse_time("soon"), None);
    }
}
