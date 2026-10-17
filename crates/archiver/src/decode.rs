//! Decoding response bodies. Tolerant by design: servers differ in
//! version and in how they serialise, so unknown keys are ignored, numbers
//! may arrive as strings, and odd samples are skipped rather than failing
//! the whole response.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::{Error, Meta, PvInfo, Result, Sample, Severity, TimeSeries, ValueKind, sanitize_json};

/// Parse JSON after replacing bare `NaN`/`Infinity` tokens.
fn json(body: &str) -> Result<Value> {
    serde_json::from_str(&sanitize_json(body)).map_err(|e| {
        let head: String = body.trim_start().chars().take(80).collect();
        Error::Parse(format!("{e} (body starts {head:?})"))
    })
}

/// A value as text: strings as they are, numbers and booleans printed.
fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A value as a number: numbers, or strings that parse as one.
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// A count or precision: a non-negative whole number.
fn whole(v: &Value) -> Option<u32> {
    number(v)
        .filter(|n| n.is_finite() && *n >= 0.0 && n.fract() == 0.0 && *n <= u32::MAX as f64)
        .map(|n| n as u32)
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

/// `getData.json`. `bins` allows 5-element array values to be read as
/// `optimized` bins.
pub(crate) fn data(body: &str, bins: bool) -> Result<Vec<TimeSeries>> {
    if body.trim().is_empty() {
        return Ok(Vec::new());
    }
    let items = match json(body)? {
        Value::Array(items) => items,
        // A single object rather than a one-element array.
        v @ Value::Object(_) => vec![v],
        Value::Null => Vec::new(),
        _ => return Err(Error::Parse("expected a JSON array".into())),
    };
    Ok(items
        .iter()
        .filter_map(Value::as_object)
        .map(|obj| series(obj, bins))
        .collect())
}

fn meta(obj: Option<&Map<String, Value>>) -> Meta {
    let mut m = Meta::default();
    for (k, v) in obj.into_iter().flatten() {
        match k.as_str() {
            "name" => m.name = text(v).unwrap_or_default(),
            "EGU" => m.egu = non_empty(text(v)),
            "PREC" => m.prec = whole(v),
            "LOPR" => m.lopr = number(v),
            "HOPR" => m.hopr = number(v),
            "DESC" => m.desc = non_empty(text(v)),
            _ => {
                if let Some(t) = text(v) {
                    m.extra.insert(k.clone(), t);
                }
            }
        }
    }
    m
}

/// What the `val`s say the series is, and whether the arrays are bins.
fn classify(data: &[Value], meta: &Meta, bins: bool) -> (ValueKind, bool) {
    let vals = || {
        data.iter()
            .filter_map(|s| s.get("val"))
            .filter(|v| !v.is_null())
    };
    let kind = match vals().next() {
        None | Some(Value::Number(_) | Value::Bool(_)) => ValueKind::Scalar,
        Some(Value::String(_)) => ValueKind::Text,
        Some(Value::Array(_)) => ValueKind::Waveform,
        Some(_) => ValueKind::Other,
    };
    let waveform_hint = meta.extra.get("waveform").is_some_and(|w| w == "true");
    let is_bin = |v: &Value| {
        v.as_array().is_some_and(|a| {
            a.len() == 5
                && a[..4].iter().all(|x| x.is_null() || x.is_number())
                && whole(&a[4]).is_some()
        })
    };
    let binned = bins && kind == ValueKind::Waveform && !waveform_hint && vals().all(is_bin);
    (if binned { ValueKind::Scalar } else { kind }, binned)
}

/// A JSON number (or numeric string) as f64; null and others are NaN.
fn float(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(v) => number(v).unwrap_or(f64::NAN),
        None => f64::NAN,
    }
}

fn series(obj: &Map<String, Value>, bins: bool) -> TimeSeries {
    let meta = meta(obj.get("meta").and_then(Value::as_object));
    let data: &[Value] = obj
        .get("data")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let (kind, binned) = classify(data, &meta, bins);
    let mut samples: Vec<Sample> = Vec::with_capacity(data.len());
    for s in data.iter().filter_map(Value::as_object) {
        let Some(secs) = s.get("secs").and_then(number) else {
            continue;
        };
        let nanos = s.get("nanos").and_then(number).unwrap_or(0.0);
        let t = secs + nanos * 1e-9;
        let severity = Severity::from_code(s.get("severity").and_then(Value::as_i64).unwrap_or(0));
        let status = s
            .get("status")
            .and_then(Value::as_u64)
            .map_or(0, |n| n.min(u16::MAX as u64) as u16);
        let fields = s.get("fields").and_then(Value::as_object);
        let field = |k: &str| fields.and_then(|f| f.get(k)).and_then(number);
        let lost = field("cnxlostepsecs");

        let val = s.get("val");
        let mut sample = match (val, binned) {
            (Some(Value::Array(a)), true) => {
                let mean = float(a.first());
                let (mut min, mut max) = (float(a.get(2)), float(a.get(3)));
                if min.is_nan() || max.is_nan() {
                    (min, max) = (mean, mean);
                }
                Sample {
                    t,
                    val: mean,
                    min,
                    max,
                    count: a.get(4).and_then(whole).unwrap_or(0),
                    severity,
                    status,
                }
            }
            (Some(Value::Array(a)), false) => Sample::raw(t, float(a.first())),
            (Some(Value::String(_) | Value::Object(_)), _) => Sample::raw(t, f64::NAN),
            (v, _) => Sample::raw(t, float(v)),
        };
        sample.severity = severity;
        sample.status = status;

        if severity == Severity::Invalid && lost.is_some() {
            // The connection-lost event: the line breaks here.
            sample = Sample {
                status,
                ..Sample::gap(t)
            };
        } else if let Some(lost) = lost {
            // A first sample after reconnecting: break the line at the
            // time the connection was lost, if nothing has yet.
            let prev = samples.last();
            if lost < t && prev.is_none_or(|p| p.t < lost && !p.is_gap()) {
                samples.push(Sample::gap(lost));
            }
        }
        samples.push(sample);
    }
    if !samples.is_sorted_by(|a, b| a.t <= b.t) {
        samples.sort_by(|a, b| a.t.total_cmp(&b.t));
    }
    TimeSeries {
        meta,
        kind,
        binned,
        samples,
    }
}

/// A JSON array of PV names.
pub(crate) fn names(body: &str) -> Result<Vec<String>> {
    match json(body)? {
        Value::Array(items) => Ok(items.iter().filter_map(text).collect()),
        Value::Null => Ok(Vec::new()),
        _ => Err(Error::Parse("expected a JSON array of names".into())),
    }
}

/// `getMetadata`: a flat object, values usually strings.
pub(crate) fn metadata(body: &str) -> Result<PvInfo> {
    let Value::Object(obj) = json(body)? else {
        return Err(Error::Parse("expected a JSON object".into()));
    };
    let raw: BTreeMap<String, String> = obj
        .iter()
        .filter_map(|(k, v)| Some((k.clone(), text(v)?)))
        .collect();
    let get = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| raw.get(*k).filter(|v| !v.trim().is_empty()))
            .cloned()
    };
    let num = |keys: &[&str]| get(keys).and_then(|v| v.trim().parse::<f64>().ok());
    let whole = |keys: &[&str]| {
        num(keys)
            .filter(|n| *n >= 0.0 && n.fract() == 0.0 && *n <= u32::MAX as f64)
            .map(|n| n as u32)
    };
    Ok(PvInfo {
        dbr_type: get(&["DBRType"]),
        element_count: whole(&["elementCount"]),
        units: get(&["units", "EGU"]),
        precision: whole(&["precision", "PREC"]),
        lower_display_limit: num(&["lowerDisplayLimit", "LOPR"]),
        upper_display_limit: num(&["upperDisplayLimit", "HOPR"]),
        raw,
    })
}
