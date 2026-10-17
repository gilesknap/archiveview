//! A client for the EPICS Archiver Appliance retrieval API.
//!
//! The pieces are split so that a browser build could reuse everything but
//! the HTTP: [`ArchiverClient`] builds the request URLs (`*_url`), the
//! `parse_*` functions decode the bodies, and on native targets the
//! blocking `get_data`/`search`/`metadata` methods do both over `ureq`.
//!
//! Times are UTC seconds since the Unix epoch as `f64` (sub-microsecond
//! resolution for current dates), the same as bobview's trend and
//! `archiveview`'s plot.
//!
//! See `DESIGN.md` at the repository root for the wire format and its
//! surprises (bare `NaN` tokens, string `PREC`, `[ ]` for no data, binned
//! `[mean, std, min, max, count]` values from `optimized_N`).

use std::collections::BTreeMap;
use std::fmt;

mod decode;
#[cfg(not(target_arch = "wasm32"))]
mod http;
mod json;
mod time;

pub use json::sanitize_json;
pub use time::{format_time, parse_time};

/// The default number of PV names a search asks for.
pub const DEFAULT_SEARCH_LIMIT: i32 = 500;

/// The most bins an `optimized_N` request asks for, whatever the plot width.
pub const MAX_BINS: u32 = 4000;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a request failed.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// The archiver URL given could not be used.
    BadUrl(String),
    /// The server answered with a non-2xx status. `message` is the first
    /// non-empty line of the body, with HTML tags stripped.
    Http { status: u16, message: String },
    /// No answer: connection refused, DNS, timeout, TLS, ...
    Transport(String),
    /// A 2xx answer that is not the expected JSON.
    Parse(String),
}

impl Error {
    /// True for the archiver's "PV is not archived" answer (HTTP 404).
    pub fn is_not_archived(&self) -> bool {
        matches!(self, Error::Http { status: 404, .. })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadUrl(m) => write!(f, "bad archiver URL: {m}"),
            Error::Http { status, message } if message.is_empty() => write!(f, "HTTP {status}"),
            Error::Http { status, message } => write!(f, "HTTP {status}: {message}"),
            Error::Transport(m) => write!(f, "{m}"),
            Error::Parse(m) => write!(f, "bad response: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// An archived alarm severity (the `severity` field: 0..=3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    #[default]
    NoAlarm,
    Minor,
    Major,
    Invalid,
}

impl Severity {
    /// From the archiver's number; anything above 3 counts as `Invalid`.
    pub fn from_code(code: i64) -> Self {
        match code {
            ..=0 => Severity::NoAlarm,
            1 => Severity::Minor,
            2 => Severity::Major,
            _ => Severity::Invalid,
        }
    }
}

/// One plotted point of archived data.
///
/// A raw sample has `min == max == val`. A bin from `optimized_N` has `val`
/// = the bin mean, `min`/`max` its extremes and `count` its sample count;
/// `t` is then the bin centre. `val` is NaN at a gap (a disconnect, a
/// severity-INVALID connection-lost event, or an archived NaN), where the
/// plot breaks the line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// UTC seconds since the epoch (`secs + nanos * 1e-9`).
    pub t: f64,
    pub val: f64,
    pub min: f64,
    pub max: f64,
    /// Samples in the bin; 1 for a raw sample.
    pub count: u32,
    pub severity: Severity,
    pub status: u16,
}

impl Sample {
    /// A raw sample.
    pub fn raw(t: f64, val: f64) -> Self {
        Self {
            t,
            val,
            min: val,
            max: val,
            count: 1,
            severity: Severity::NoAlarm,
            status: 0,
        }
    }

    /// A line break at `t`.
    pub fn gap(t: f64) -> Self {
        Self {
            severity: Severity::Invalid,
            ..Self::raw(t, f64::NAN)
        }
    }

    pub fn is_gap(&self) -> bool {
        self.val.is_nan()
    }
}

/// What the archived values were, as seen in the response.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ValueKind {
    /// Scalar numbers (doubles, integers or enum indices): plottable.
    #[default]
    Scalar,
    /// Arrays: element 0 is plotted. Always fetch these raw.
    Waveform,
    /// Strings: not plotted in v1 (samples carry NaN).
    Text,
    /// PVA structures and anything else: not plotted.
    Other,
}

/// The `meta` object of a response, with the common fields decoded. Values
/// may arrive as strings or numbers; both are accepted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub name: String,
    /// `EGU`, absent when the PV has no units.
    pub egu: Option<String>,
    /// `PREC`.
    pub prec: Option<u32>,
    /// `LOPR`/`HOPR` (only with `fetchLatestMetadata=true`).
    pub lopr: Option<f64>,
    pub hopr: Option<f64>,
    /// `DESC` (only with `fetchLatestMetadata=true`).
    pub desc: Option<String>,
    /// Every other key, as text.
    pub extra: BTreeMap<String, String>,
}

/// One PV's archived data for a request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimeSeries {
    pub meta: Meta,
    pub kind: ValueKind,
    /// True when the samples are `optimized_N` bins (`min`/`max` envelope
    /// meaningful), false for raw samples.
    pub binned: bool,
    /// In time order. The first may be before the requested `from`: it is
    /// the value that held at `from`.
    pub samples: Vec<Sample>,
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// Server-side processing for a data request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Operator {
    /// Every archived sample.
    #[default]
    Raw,
    /// `optimized_N`: about N points; raw when there are at most N samples,
    /// else `[mean, std, min, max, count]` bins.
    Optimized(u32),
    /// `mean_N`: the mean of each N-second bin.
    Mean(u32),
    /// `nth_N`: every Nth raw sample.
    Nth(u32),
}

impl Operator {
    /// The `pv` parameter for `pv`: `optimized_1200(SR-DI-DCCT-01:SIGNAL)`.
    pub fn wrap(&self, pv: &str) -> String {
        match self {
            Operator::Raw => pv.to_string(),
            Operator::Optimized(n) => format!("optimized_{n}({pv})"),
            Operator::Mean(n) => format!("mean_{n}({pv})"),
            Operator::Nth(n) => format!("nth_{n}({pv})"),
        }
    }

    /// The operator to plot `width_px` device pixels: `optimized` with two
    /// bins per pixel, at most [`MAX_BINS`].
    pub fn for_width(width_px: f32) -> Self {
        let n = (2.0 * width_px.max(1.0)).round() as u32;
        Operator::Optimized(n.clamp(1, MAX_BINS))
    }
}

/// A `getData.json` request.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    /// The PV name, without any `ca://`/`pva://` prefix.
    pub pv: String,
    pub from: f64,
    pub to: f64,
    pub op: Operator,
    /// Ask for `DESC`, `LOPR`, `HOPR`, ... in `meta`.
    pub fetch_latest_metadata: bool,
}

impl Query {
    pub fn new(pv: impl Into<String>, from: f64, to: f64, op: Operator) -> Self {
        Self {
            pv: pv.into(),
            from,
            to,
            op,
            fetch_latest_metadata: false,
        }
    }
}

/// How a search pattern is matched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchMode {
    /// `*` and `?` wildcards, sent as `pv=`.
    #[default]
    Glob,
    /// A Java regex matching the whole name, sent as `regex=`.
    Regex,
}

/// A `getMatchingPVs` request.
#[derive(Clone, Debug, PartialEq)]
pub struct Search {
    pub pattern: String,
    pub mode: SearchMode,
    /// Names wanted at most; -1 for all.
    pub limit: i32,
}

impl Search {
    /// What the search box sends for `text`: a glob, wrapped as `*text*`
    /// for a substring match when it has no `*` or `?`. Surrounding
    /// whitespace is trimmed.
    pub fn from_user(text: &str, limit: i32) -> Self {
        let text = text.trim();
        let pattern = if text.contains(['*', '?']) {
            text.to_string()
        } else {
            format!("*{text}*")
        };
        Self {
            pattern,
            mode: SearchMode::Glob,
            limit,
        }
    }

    pub fn regex(re: impl Into<String>, limit: i32) -> Self {
        Self {
            pattern: re.into(),
            mode: SearchMode::Regex,
            limit,
        }
    }
}

/// What `getMetadata` says about a PV (all optional: servers differ).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PvInfo {
    /// e.g. `DBR_SCALAR_DOUBLE`, `DBR_WAVEFORM_DOUBLE`, `DBR_SCALAR_ENUM`.
    pub dbr_type: Option<String>,
    pub element_count: Option<u32>,
    pub units: Option<String>,
    pub precision: Option<u32>,
    pub lower_display_limit: Option<f64>,
    pub upper_display_limit: Option<f64>,
    /// Every key as returned.
    pub raw: BTreeMap<String, String>,
}

impl PvInfo {
    /// True when `optimized_N` makes sense: a scalar number (or enum).
    /// Unknown types count as numeric.
    pub fn is_scalar_numeric(&self) -> bool {
        let scalar = self.element_count.is_none_or(|n| n <= 1);
        let numeric = self
            .dbr_type
            .as_deref()
            .is_none_or(|t| !t.contains("STRING") && !t.contains("V4"));
        scalar && numeric
    }

    /// True when the plot should ask for `optimized_N` bins: a scalar
    /// number that is not an enum. Bin means of enum state indices (1.63)
    /// name no state, so enums are fetched raw, as are waveforms and text.
    pub fn binnable(&self) -> bool {
        self.is_scalar_numeric() && self.dbr_type.as_deref().is_none_or(|t| !t.contains("ENUM"))
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An Archiver Appliance retrieval service. Cheap to clone; holds no
/// connection state, so it can be shared freely between threads and
/// between several plots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiverClient {
    /// The normalised base, ending in `/retrieval` with no trailing slash.
    base: String,
    /// Per-request timeout in seconds (native only).
    timeout_secs: u64,
}

impl ArchiverClient {
    /// From the URL the user gave, e.g. `http://host:17668` or
    /// `http://host:17668/retrieval/`: strips trailing slashes and appends
    /// `/retrieval` unless it already ends so. Only `http` and `https` are
    /// accepted; a bare `host:port` gets `http://`.
    pub fn new(url: &str) -> Result<Self> {
        let mut url = url.trim().trim_end_matches('/').to_string();
        if url.is_empty() {
            return Err(Error::BadUrl("empty".into()));
        }
        if !url.contains("://") {
            url = format!("http://{url}");
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(Error::BadUrl(format!(
                "{url}: only http and https are supported"
            )));
        }
        if !url.ends_with("/retrieval") {
            url.push_str("/retrieval");
        }
        Ok(Self {
            base: url,
            timeout_secs: 30,
        })
    }

    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// The normalised retrieval base, e.g. `http://host:17668/retrieval`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// `getData.json` with `from`/`to` as `YYYY-MM-DDTHH:MM:SS.mmmZ` and
    /// every parameter percent-encoded.
    pub fn data_url(&self, q: &Query) -> String {
        let mut url = format!(
            "{}/data/getData.json?pv={}&from={}&to={}",
            self.base,
            encode(&q.op.wrap(&q.pv)),
            encode(&format_time(q.from)),
            encode(&format_time(q.to)),
        );
        if q.fetch_latest_metadata {
            url.push_str("&fetchLatestMetadata=true");
        }
        url
    }

    /// `bpl/getMatchingPVs?pv=...&limit=N` or `?regex=...&limit=N`.
    pub fn search_url(&self, s: &Search) -> String {
        let key = match s.mode {
            SearchMode::Glob => "pv",
            SearchMode::Regex => "regex",
        };
        format!(
            "{}/bpl/getMatchingPVs?{key}={}&limit={}",
            self.base,
            encode(&s.pattern),
            s.limit
        )
    }

    /// `bpl/getMetadata?pv=...`.
    pub fn metadata_url(&self, pv: &str) -> String {
        format!("{}/bpl/getMetadata?pv={}", self.base, encode(pv))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ArchiverClient {
    /// Fetch and decode one PV's data. `Ok(None)` when the archiver has no
    /// data in the range (a `[ ]` response); `Err(Http{404})` when the PV is
    /// not archived. Blocking: call from a worker thread.
    pub fn get_data(&self, q: &Query) -> Result<Option<TimeSeries>> {
        let body = self.get(&self.data_url(q))?;
        let bins = matches!(q.op, Operator::Optimized(_));
        Ok(decode::data(&body, bins)?.into_iter().next())
    }

    /// The archived PV names matching `s`, sorted. Blocking.
    pub fn search(&self, s: &Search) -> Result<Vec<String>> {
        let mut names = parse_names(&self.get(&self.search_url(s))?)?;
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// The PV's archive type info. Blocking.
    pub fn metadata(&self, pv: &str) -> Result<PvInfo> {
        parse_metadata(&self.get(&self.metadata_url(pv))?)
    }
}

// ---------------------------------------------------------------------------
// Decoding (usable without HTTP, e.g. by a browser build)
// ---------------------------------------------------------------------------

/// Decode a `getData.json` body: one [`TimeSeries`] per PV, empty for
/// `[ ]`. Tolerates bare `NaN`/`Infinity`/`-Infinity`, string or numeric
/// `PREC`/`LOPR`/`HOPR`, unknown keys, and `fields` objects. A severity-3
/// sample whose `fields` hold `cnxlostepsecs` becomes a [`Sample::gap`].
/// A 5-element array `val` on a scalar PV is an `optimized` bin
/// (`binned = true`); other arrays are waveforms (element 0 plotted).
///
/// Without knowing the request, a series whose every array value has five
/// elements with a whole, non-negative count is taken as bins;
/// [`ArchiverClient::get_data`] only does that for `optimized` requests.
pub fn parse_data(body: &str) -> Result<Vec<TimeSeries>> {
    decode::data(body, true)
}

/// Decode a JSON array of PV names (`getMatchingPVs`, `getAllPVs`).
pub fn parse_names(body: &str) -> Result<Vec<String>> {
    decode::names(body)
}

/// Decode a `getMetadata` body.
pub fn parse_metadata(body: &str) -> Result<PvInfo> {
    decode::metadata(body)
}

/// Percent-encode a query parameter value (everything but RFC 3986
/// unreserved characters).
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3 / 2);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests;
