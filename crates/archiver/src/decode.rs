//! Decoding response bodies. Tolerant by design: servers differ in
//! version and in how they serialise, so unknown keys are ignored, numbers
//! may arrive as strings, and odd samples are skipped rather than failing
//! the whole response.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

use serde::de::value::{MapAccessDeserializer, SeqAccessDeserializer};
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
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
///
/// Decoded into typed samples rather than a `serde_json::Value` tree: a
/// waveform's `val` keeps only its first five elements (all a bin or the
/// plot needs) and skips the rest unparsed, so memory follows the sample
/// count, not the element count. A day of a 1000-element waveform at 1 Hz
/// is most of a gigabyte of JSON, and as a `Value` tree several times that.
pub(crate) fn data(body: &str, bins: bool) -> Result<Vec<TimeSeries>> {
    let body = sanitize_json(body);
    let parse_err = |e: serde_json::Error| {
        let head: String = body.trim_start().chars().take(80).collect();
        Error::Parse(format!("{e} (body starts {head:?})"))
    };
    let items: Vec<Lenient<RawSeries>> = match body.trim_start().as_bytes().first() {
        None => return Ok(Vec::new()),
        Some(b'[') => serde_json::from_str(&body).map_err(parse_err)?,
        // A single object rather than a one-element array.
        Some(b'{') => vec![serde_json::from_str(&body).map_err(parse_err)?],
        Some(b'n') if body.trim() == "null" => return Ok(Vec::new()),
        Some(_) => {
            // Not JSON at all (an HTML error page), or not an array.
            serde_json::from_str::<IgnoredAny>(&body).map_err(parse_err)?;
            return Err(Error::Parse("expected a JSON array".into()));
        }
    };
    Ok(items
        .into_iter()
        .filter_map(|s| s.0)
        .map(|s| series(s, bins))
        .collect())
}

/// A value of type `T` where one is found (an object or an array), and
/// `None` for anything else (a number, a string, null), so one odd item
/// does not fail the whole response.
struct Lenient<T>(Option<T>);

impl<T> Default for Lenient<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Lenient<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Lenient<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                T::deserialize(MapAccessDeserializer::new(a)).map(|t| Lenient(Some(t)))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                T::deserialize(SeqAccessDeserializer::new(a)).map(|t| Lenient(Some(t)))
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                _: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(Lenient(None))
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> std::result::Result<Self::Value, E> {
                Ok(Lenient(None))
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> std::result::Result<Self::Value, E> {
                Ok(Lenient(None))
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<Self::Value, E> {
                Ok(Lenient(None))
            }
            fn visit_str<E: serde::de::Error>(
                self,
                _: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(Lenient(None))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(Lenient(None))
            }
        }
        d.deserialize_any(V(PhantomData))
    }
}

/// One PV's part of a `getData.json` body.
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawSeries {
    meta: Option<Value>,
    data: Lenient<Vec<Lenient<RawSample>>>,
}

/// One sample as sent. Unknown keys are skipped.
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawSample {
    secs: Num,
    nanos: Num,
    severity: Num,
    status: Num,
    val: Val,
    fields: Option<Value>,
}

/// A number, or a string holding one; `None` for anything else.
#[derive(Default, Clone, Copy)]
struct Num(Option<f64>);

/// A sample's `val`.
#[derive(Default)]
enum Val {
    /// Absent or null.
    #[default]
    Null,
    Num(f64),
    Bool(bool),
    Str,
    /// The first (up to) five elements, and how many there were.
    Array(Vec<Elem>, usize),
    Object,
}

/// An element of an array `val`.
#[derive(Clone, Copy)]
enum Elem {
    Null,
    Bool(bool),
    Num(f64),
    /// A string, and the number it holds, if any.
    Str(Option<f64>),
    Other,
}

/// Visit a scalar JSON value as `T`, skipping arrays and objects as
/// `other` without building them.
macro_rules! scalar_visitor {
    ($name:ident, $ty:ty, $expect:literal, other = $other:expr,
     num = |$n:ident| $num:expr, bool = |$b:ident| $bool:expr,
     str = |$s:ident| $str:expr, null = $null:expr) => {
        struct $name;
        impl<'de> Visitor<'de> for $name {
            type Value = $ty;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str($expect)
            }
            fn visit_bool<E: serde::de::Error>(self, $b: bool) -> std::result::Result<$ty, E> {
                Ok($bool)
            }
            fn visit_i64<E: serde::de::Error>(self, n: i64) -> std::result::Result<$ty, E> {
                self.visit_f64(n as f64)
            }
            fn visit_u64<E: serde::de::Error>(self, n: u64) -> std::result::Result<$ty, E> {
                self.visit_f64(n as f64)
            }
            fn visit_f64<E: serde::de::Error>(self, $n: f64) -> std::result::Result<$ty, E> {
                Ok($num)
            }
            fn visit_str<E: serde::de::Error>(self, $s: &str) -> std::result::Result<$ty, E> {
                Ok($str)
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<$ty, E> {
                Ok($null)
            }
            fn visit_none<E: serde::de::Error>(self) -> std::result::Result<$ty, E> {
                Ok($null)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<$ty, A::Error> {
                while a.next_element::<IgnoredAny>()?.is_some() {}
                Ok($other)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> std::result::Result<$ty, A::Error> {
                while a.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok($other)
            }
        }
    };
}

fn parse_num(s: &str) -> Option<f64> {
    s.trim().parse().ok()
}

scalar_visitor!(
    NumVisitor,
    Num,
    "a number",
    other = Num(None),
    num = |n| Num(Some(n)),
    bool = |_b| Num(None),
    str = |s| Num(parse_num(s)),
    null = Num(None)
);

scalar_visitor!(
    ElemVisitor,
    Elem,
    "an array element",
    other = Elem::Other,
    num = |n| Elem::Num(n),
    bool = |b| Elem::Bool(b),
    str = |s| Elem::Str(parse_num(s)),
    null = Elem::Null
);

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        d.deserialize_any(NumVisitor)
    }
}

impl<'de> Deserialize<'de> for Elem {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        d.deserialize_any(ElemVisitor)
    }
}

impl<'de> Deserialize<'de> for Val {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Val;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a sample value")
            }
            fn visit_bool<E: serde::de::Error>(self, b: bool) -> std::result::Result<Val, E> {
                Ok(Val::Bool(b))
            }
            fn visit_i64<E: serde::de::Error>(self, n: i64) -> std::result::Result<Val, E> {
                Ok(Val::Num(n as f64))
            }
            fn visit_u64<E: serde::de::Error>(self, n: u64) -> std::result::Result<Val, E> {
                Ok(Val::Num(n as f64))
            }
            fn visit_f64<E: serde::de::Error>(self, n: f64) -> std::result::Result<Val, E> {
                Ok(Val::Num(n))
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> std::result::Result<Val, E> {
                Ok(Val::Str)
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Val, E> {
                Ok(Val::Null)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<Val, A::Error> {
                let mut head = Vec::new();
                while head.len() < 5 {
                    match a.next_element::<Elem>()? {
                        Some(e) => head.push(e),
                        None => {
                            let n = head.len();
                            return Ok(Val::Array(head, n));
                        }
                    }
                }
                // The rest of a waveform: skipped, not stored.
                let mut n = head.len();
                while a.next_element::<IgnoredAny>()?.is_some() {
                    n += 1;
                }
                Ok(Val::Array(head, n))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> std::result::Result<Val, A::Error> {
                while a.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Val::Object)
            }
        }
        d.deserialize_any(V)
    }
}

impl Elem {
    /// As a plotted value: numbers, booleans and numeric strings; NaN for
    /// the rest.
    fn float(self) -> f64 {
        match self {
            Elem::Num(n) => n,
            Elem::Bool(b) => f64::from(u8::from(b)),
            Elem::Str(n) => n.unwrap_or(f64::NAN),
            Elem::Null | Elem::Other => f64::NAN,
        }
    }

    /// As a bin's count: a non-negative whole number.
    fn whole(self) -> Option<u32> {
        let n = match self {
            Elem::Num(n) | Elem::Str(Some(n)) => n,
            _ => return None,
        };
        (n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n <= u32::MAX as f64).then_some(n as u32)
    }
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
fn classify(data: &[RawSample], meta: &Meta, bins: bool) -> (ValueKind, bool) {
    let vals = || {
        data.iter()
            .map(|s| &s.val)
            .filter(|v| !matches!(v, Val::Null))
    };
    let kind = match vals().next() {
        None | Some(Val::Num(_) | Val::Bool(_)) => ValueKind::Scalar,
        Some(Val::Str) => ValueKind::Text,
        Some(Val::Array(..)) => ValueKind::Waveform,
        Some(_) => ValueKind::Other,
    };
    let waveform_hint = meta.extra.get("waveform").is_some_and(|w| w == "true");
    let is_bin = |v: &Val| match v {
        Val::Array(a, 5) => {
            a[..4]
                .iter()
                .all(|x| matches!(x, Elem::Null | Elem::Num(_)))
                && a[4].whole().is_some()
        }
        _ => false,
    };
    let binned = bins && kind == ValueKind::Waveform && !waveform_hint && vals().all(is_bin);
    (if binned { ValueKind::Scalar } else { kind }, binned)
}

fn series(raw: RawSeries, bins: bool) -> TimeSeries {
    let meta = meta(raw.meta.as_ref().and_then(Value::as_object));
    let data: Vec<RawSample> = raw
        .data
        .0
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| s.0)
        .collect();
    let (kind, binned) = classify(&data, &meta, bins);
    let mut samples: Vec<Sample> = Vec::with_capacity(data.len());
    for s in &data {
        let Some(secs) = s.secs.0 else {
            continue;
        };
        let nanos = s.nanos.0.unwrap_or(0.0);
        let t = secs + nanos * 1e-9;
        let severity = Severity::from_code(
            s.severity
                .0
                .filter(|n| n.fract() == 0.0)
                .map_or(0, |n| n as i64),
        );
        let status = s
            .status
            .0
            .filter(|n| *n >= 0.0 && n.fract() == 0.0)
            .map_or(0, |n| n.min(u16::MAX as f64) as u16);
        let fields = s.fields.as_ref().and_then(Value::as_object);
        let field = |k: &str| fields.and_then(|f| f.get(k)).and_then(number);
        let lost = field("cnxlostepsecs");

        let mut sample = match (&s.val, binned) {
            (Val::Array(a, _), true) => {
                let get = |i: usize| a.get(i).map_or(f64::NAN, |e| e.float());
                let mean = get(0);
                let (mut min, mut max) = (get(2), get(3));
                if min.is_nan() || max.is_nan() {
                    (min, max) = (mean, mean);
                }
                Sample {
                    t,
                    val: mean,
                    min,
                    max,
                    count: a.get(4).and_then(|e| e.whole()).unwrap_or(0),
                    severity,
                    status,
                }
            }
            (Val::Array(a, _), false) => Sample::raw(t, a.first().map_or(f64::NAN, |e| e.float())),
            (Val::Num(n), _) => Sample::raw(t, *n),
            (Val::Bool(b), _) => Sample::raw(t, f64::from(u8::from(*b))),
            (Val::Str | Val::Object | Val::Null, _) => Sample::raw(t, f64::NAN),
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
