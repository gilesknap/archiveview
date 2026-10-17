//! A view: what a plot shows. Saved as JSON in
//! `~/.config/archiveview/views/<name>.json` and returned by
//! [`crate::Plot::view`] so an embedding app (bobview) can store it in its
//! own layouts.

use serde::{Deserialize, Serialize};

/// The view file format's version. Files with a newer one are refused.
/// Additions an older program can ignore (value axes after the first, a
/// trace's axis, axis names) keep the version.
pub const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub version: u32,
    /// The traces, in legend order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pvs: Vec<TraceView>,
    #[serde(default)]
    pub range: Range,
    /// The first value axis (the only one, in files from before there
    /// were several).
    #[serde(default)]
    pub y: YAxis,
    /// The value axes after `y`: a trace's `axis` counts `y` as 0, so
    /// these are 1, 2, ...
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub axes: Vec<YAxis>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            version: VERSION,
            pvs: Vec::new(),
            range: Range::default(),
            y: YAxis::default(),
            axes: Vec::new(),
        }
    }
}

impl View {
    /// The value axes in the order a trace's `axis` counts them: `y`,
    /// then `axes`.
    pub fn value_axes(&self) -> impl Iterator<Item = &YAxis> {
        std::iter::once(&self.y).chain(&self.axes)
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
    /// Its value axis: 0 for `y`, 1 for the first of `axes`, and so on.
    /// Absent (files from before there were several axes, or a trace
    /// whose data had not come yet), it joins an axis by itself when its
    /// data arrives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<usize>,
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

/// A value axis.
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
    /// The name the user gave it. Without one it names itself from its
    /// traces' units and names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Default for YAxis {
    fn default() -> Self {
        Self {
            autoscale: true,
            min: None,
            max: None,
            log: false,
            name: None,
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
                    axis: None,
                },
                TraceView {
                    name: "bl01t:SUM".into(),
                    color: Some("#d62728".into()),
                    visible: false,
                    axis: None,
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
    fn views_keep_their_value_axes() {
        let v = View {
            pvs: vec![
                TraceView {
                    name: "SR-DI-DCCT-01:SIGNAL".into(),
                    color: None,
                    visible: true,
                    axis: Some(0),
                },
                TraceView {
                    name: "SR-DI-DCCT-01:LIFETIME".into(),
                    color: None,
                    visible: true,
                    axis: Some(1),
                },
            ],
            axes: vec![YAxis {
                autoscale: false,
                min: Some(1.0),
                max: Some(100.0),
                log: true,
                name: Some("Lifetime".into()),
            }],
            ..View::default()
        };
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "version": 1,
                "pvs": [
                    { "name": "SR-DI-DCCT-01:SIGNAL", "axis": 0 },
                    { "name": "SR-DI-DCCT-01:LIFETIME", "axis": 1 }
                ],
                "range": { "kind": "relative", "secs": 3600.0 },
                "y": { "autoscale": true },
                "axes": [
                    { "autoscale": false, "min": 1.0, "max": 100.0, "log": true, "name": "Lifetime" }
                ]
            })
        );
        assert_eq!(serde_json::from_value::<View>(json).unwrap(), v);
        assert_eq!(v.value_axes().count(), 2);
    }

    #[test]
    fn files_from_before_several_axes_load() {
        // As the first release wrote them: one `y`, no trace axes.
        let old = r##"{
          "version": 1,
          "pvs": [ { "name": "SR-DI-DCCT-01:SIGNAL" }, { "name": "bl01t:SUM", "color": "#d62728", "visible": false } ],
          "range": { "kind": "relative", "secs": 3600 },
          "y": { "autoscale": false, "min": 0, "max": 300, "log": true }
        }"##;
        let v: View = serde_json::from_str(old).unwrap();
        assert!(v.pvs.iter().all(|t| t.axis.is_none()));
        assert!(v.axes.is_empty());
        assert_eq!((v.y.min, v.y.max, v.y.log), (Some(0.0), Some(300.0), true));
        assert_eq!(v.y.name, None);
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
