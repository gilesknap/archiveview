//! `getData.json`: raw retrieval and the post-processing operators, written
//! out the way the appliance's `JSONResponse` does (by hand, with bare
//! `NaN` tokens and Java number formatting).

use crate::signals::{self, Event, Signal, Value};
use crate::{Clock, Params, Reply};

/// The appliance's default summarising interval (seconds).
const DEFAULT_INTERVAL: u32 = 900;
/// `optimized`'s default number of points.
const DEFAULT_POINTS: u32 = 1000;
/// Refuse requests that would build more bins than this.
const MAX_BINS: i64 = 5_000_000;

/// A per-bin statistic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stat {
    Mean,
    Min,
    Max,
    Std,
    Variance,
    Median,
    Count,
    First,
    Last,
}

/// A parsed post-processor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Raw,
    /// `optimized_N` / `optimizedLastSample_N` (`fill` false for the
    /// `...Sample` no-fill variants).
    Optimized {
        points: u32,
        fill: bool,
        last: bool,
    },
    Stat {
        stat: Stat,
        secs: u32,
        fill: bool,
    },
    Nth(u32),
    NCount,
}

/// What a post-processor name means, in the appliance's registration order
/// (it matches names with `startsWith`, so order matters).
#[derive(Clone, Copy)]
enum Base {
    Stat(Stat),
    Optimized { last: bool },
    Nth,
    NCount,
}

const REGISTRY: &[(&str, Base, bool)] = &[
    // (identity, what, has a no-fill "<identity>Sample" variant)
    ("firstSample", Base::Stat(Stat::First), true),
    ("mean", Base::Stat(Stat::Mean), true),
    ("std", Base::Stat(Stat::Std), true),
    ("median", Base::Stat(Stat::Median), true),
    ("variance", Base::Stat(Stat::Variance), true),
    ("firstFill", Base::Stat(Stat::First), true),
    ("lastFill", Base::Stat(Stat::Last), true),
    ("lastSample", Base::Stat(Stat::Last), true),
    ("max", Base::Stat(Stat::Max), true),
    ("min", Base::Stat(Stat::Min), true),
    ("count", Base::Stat(Stat::Count), true),
    ("ncount", Base::NCount, false),
    ("nth", Base::Nth, false),
    ("optimizedLastSample", Base::Optimized { last: true }, true),
    ("optimized", Base::Optimized { last: false }, true),
];

/// Parse a post-processor argument such as `optimized_1200` or
/// `meanSample_600`. Unknown names mean raw data, as on the appliance;
/// a malformed number is an error.
pub(crate) fn parse_op(arg: &str) -> Result<Op, String> {
    let arg = arg.trim();
    if arg.is_empty() {
        return Ok(Op::Raw);
    }
    let found = REGISTRY.iter().find_map(|&(id, base, no_fill)| {
        if no_fill && arg.starts_with(&format!("{id}Sample")) {
            Some((base, false))
        } else if arg.starts_with(id) {
            Some((base, true))
        } else {
            None
        }
    });
    let Some((base, fill)) = found else {
        return Ok(Op::Raw);
    };
    let number = |default: u32| -> Result<u32, String> {
        match arg.split('_').nth(1) {
            None => Ok(default),
            Some(n) => n
                .parse::<u32>()
                .ok()
                .filter(|&n| n > 0)
                .ok_or_else(|| format!("bad post-processor argument in {arg}")),
        }
    };
    Ok(match base {
        Base::Stat(stat) => Op::Stat {
            stat,
            secs: number(DEFAULT_INTERVAL)?,
            fill,
        },
        Base::Optimized { last } => Op::Optimized {
            points: number(DEFAULT_POINTS)?,
            fill,
            last,
        },
        Base::Nth => Op::Nth(number(1)?),
        Base::NCount => Op::NCount,
    })
}

/// Split `op_N(PV)` / `op(PV,N)` into the PV name and the post-processor
/// argument, the way `DataRetrievalServlet` does. `None` for unbalanced
/// parentheses.
pub(crate) fn split_pv(pv: &str) -> Option<(String, Option<String>)> {
    let open = pv.matches('(').count();
    let close = pv.matches(')').count();
    if open == 0 && close == 0 {
        return Some((pv.to_string(), None));
    }
    if open != 1 || close != 1 || !pv.ends_with(')') {
        return None;
    }
    let (op, rest) = pv.split_once('(')?;
    let inner = rest.strip_suffix(')')?;
    let mut parts = inner.split(',');
    let name = parts.next()?.trim().to_string();
    let op = match parts.next() {
        Some(n) => format!("{}_{}", op.trim(), n.trim()),
        None => op.trim().to_string(),
    };
    Some((name, Some(op)))
}

/// Parse an appliance timestamp to epoch nanoseconds.
pub(crate) fn parse_ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .ok()?
        .timestamp_nanos_opt()
}

/// `GET /retrieval/data/getData.json`.
pub(crate) fn get_data(params: &Params, clock: &Clock) -> Reply {
    let Some(pv) = params.get("pv") else {
        return Reply::error(400, "Bad Request", "No PV specified");
    };
    let Some((name, op_arg)) = split_pv(pv) else {
        return Reply::error(400, "Bad Request", "Unbalanced parentheses in pv");
    };
    let op_arg = op_arg.or_else(|| params.get("pp").map(str::to_string));
    let op = match parse_op(op_arg.as_deref().unwrap_or("")) {
        Ok(op) => op,
        Err(e) => return Reply::error(400, "Bad Request", &e),
    };
    let to = match params.get("to") {
        None => clock.now_ns() + 3_600_000_000_000,
        Some(s) => match parse_ts(s) {
            Some(t) => t,
            None => return Reply::error(400, "Bad Request", &format!("Cannot parse to {s}")),
        },
    };
    let from = match params.get("from") {
        None => to - 86_400_000_000_000,
        Some(s) => match parse_ts(s) {
            Some(t) => t,
            None => {
                return Reply::error(400, "Bad Request", &format!("Cannot parse from {s}"));
            }
        },
    };
    if to < from {
        return Reply::error(400, "Bad Request", "End time is before start time");
    }
    let Some(sig) = signals::find(&name) else {
        return Reply::error(404, "Not Found", &format!("Cannot find PV {name}"));
    };
    let latest = params.get("fetchLatestMetadata") == Some("true");
    let window = Window::new(sig, clock, from, to);
    let numeric = sig.kind.is_scalar_numeric();
    let lines: Box<dyn Iterator<Item = String> + Send> = match op {
        Op::Raw => Box::new(window.raw().map(|e| event_json(&e))),
        Op::Nth(n) => Box::new(window.raw().step_by(n as usize).map(|e| event_json(&e))),
        Op::NCount => {
            let n = window.in_range().count();
            let (secs, nanos) = split_ns(from);
            Box::new(std::iter::once(format!(
                "{{ \"secs\": {secs}, \"val\": {n}, \"nanos\": {nanos}, \"severity\":0, \"status\":0 }}"
            )))
        }
        Op::Optimized { points, fill, last } if numeric => {
            if window.raw().take(points as usize + 1).count() <= points as usize {
                Box::new(window.raw().map(|e| event_json(&e)))
            } else {
                let secs = ((to.div_euclid(NS) - from.div_euclid(NS)) / points as i64).max(1);
                match window.bins(secs, Collect::Vector, fill) {
                    Ok(mut bins) => {
                        if last && let Some(e) = window.in_range().last() {
                            bins.push(event_json(&e));
                        }
                        Box::new(bins.into_iter())
                    }
                    Err(e) => return Reply::error(400, "Bad Request", &e),
                }
            }
        }
        Op::Stat { stat, secs, fill } if numeric => {
            match window.bins(secs as i64, Collect::Scalar(stat), fill) {
                Ok(bins) => Box::new(bins.into_iter()),
                Err(e) => return Reply::error(400, "Bad Request", &e),
            }
        }
        // Summaries of strings and waveforms: the mock returns raw data.
        _ => Box::new(window.raw().map(|e| event_json(&e))),
    };
    Reply::json_stream(response(sig, latest, lines))
}

const NS: i64 = 1_000_000_000;

fn split_ns(t: i64) -> (i64, i64) {
    (t.div_euclid(NS), t.rem_euclid(NS))
}

/// The response: `[ ]` when there are no samples, else one PV's
/// `{meta, data}` written as the appliance's `JSONResponse` does.
fn response(
    sig: &'static Signal,
    latest: bool,
    lines: Box<dyn Iterator<Item = String> + Send>,
) -> Box<dyn Iterator<Item = String> + Send> {
    let mut lines = lines.peekable();
    if lines.peek().is_none() {
        return Box::new(std::iter::once("[ ]".to_string()));
    }
    let mut meta = format!("{{ \"name\": {} ", json_str(sig.name));
    if !sig.egu.is_empty() {
        meta.push_str(&format!(", \"EGU\": {} ", json_str(sig.egu)));
    }
    meta.push_str(&format!(", \"PREC\": \"{}\" ", sig.prec));
    if latest {
        meta.push_str(&format!(", \"DESC\": {} ", json_str(sig.desc)));
        meta.push_str(&format!(", \"LOPR\": \"{}\" ", java_double(sig.lopr)));
        meta.push_str(&format!(", \"HOPR\": \"{}\" ", java_double(sig.hopr)));
    }
    meta.push('}');
    let head = format!("[ \n{{ \"meta\": {meta},\n\"data\": [ \n");
    let mut first = true;
    let body = lines.map(move |l| {
        if std::mem::take(&mut first) {
            l
        } else {
            format!(",\n{l}")
        }
    });
    Box::new(
        std::iter::once(head)
            .chain(body)
            .chain(std::iter::once("\n] }\n ]\n".to_string())),
    )
}

/// One sample as the appliance writes it.
pub(crate) fn event_json(e: &Event) -> String {
    let (secs, nanos) = split_ns(e.t_ns);
    let val = match &e.value {
        Value::Double(v) => java_double(*v),
        Value::Int(i) => i.to_string(),
        Value::Text(s) => json_str(s),
        Value::Array(a) => {
            let items: Vec<String> = a.iter().map(|v| java_double(*v)).collect();
            format!("[{}]", items.join(","))
        }
    };
    let mut s = format!(
        "{{ \"secs\": {secs}, \"val\": {val}, \"nanos\": {nanos}, \"severity\":{}, \"status\":{}",
        e.severity, e.status
    );
    push_fields(&mut s, e.fields.iter().map(|(k, v)| (*k, v.as_str())));
    s.push_str(" }");
    s
}

fn push_fields<'a>(s: &mut String, fields: impl Iterator<Item = (&'a str, &'a str)>) {
    let items: Vec<String> = fields
        .map(|(k, v)| format!("{}: {}", json_str(k), json_str(v)))
        .collect();
    if !items.is_empty() {
        s.push_str(&format!(", \"fields\": {{ {}}}", items.join(", ")));
    }
}

/// Java's `Double.toString`: `300.0`, `1.0E-5`, `3.0E10`, `NaN`,
/// `Infinity`.
pub(crate) fn java_double(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let a = v.abs();
    if a == 0.0 || (1e-3..1e7).contains(&a) {
        let s = format!("{v}");
        if s.contains('.') { s } else { format!("{s}.0") }
    } else {
        let s = format!("{v:E}");
        match s.split_once('E') {
            Some((m, e)) if !m.contains('.') => format!("{m}.0E{e}"),
            _ => s,
        }
    }
}

/// A JSON string literal.
pub(crate) fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The events of one PV around a request's range.
struct Window {
    sig: &'static Signal,
    /// First and last grid index with `from <= t <= min(to, now)` that are
    /// in history. `lo > hi` when there are none.
    lo: i64,
    hi: i64,
    /// The last event before `from`, unless there is one exactly at `from`.
    before: Option<Event>,
    from_ns: i64,
    to_ns: i64,
}

impl Window {
    fn new(sig: &'static Signal, clock: &Clock, from: i64, to: i64) -> Self {
        let p = sig.period_ns();
        let ceil = |t: i64| t.div_euclid(p) + i64::from(t.rem_euclid(p) != 0);
        let first = ceil(clock.origin_ns);
        let newest = clock.now_ns().div_euclid(p);
        let lo = ceil(from).max(first);
        let hi = newest.min(to.div_euclid(p));
        let at_from = from.rem_euclid(p) == 0
            && (first..=newest).contains(&(from / p))
            && sig.event(from / p).is_some();
        let before = if at_from {
            None
        } else {
            Self::back(sig, (ceil(from) - 1).min(newest), first)
        };
        Self {
            sig,
            lo,
            hi,
            before,
            from_ns: from,
            to_ns: to,
        }
    }

    /// The newest event at or before index `k`, not before `first`.
    fn back(sig: &Signal, k: i64, first: i64) -> Option<Event> {
        (first..=k).rev().take(10_000).find_map(|k| sig.event(k))
    }

    /// Events with `from <= t <= to`.
    fn in_range(&self) -> impl Iterator<Item = Event> + Send + 'static {
        let sig = self.sig;
        (self.lo..=self.hi).filter_map(move |k| sig.event(k))
    }

    /// The raw stream: the event before `from`, then those in range.
    fn raw(&self) -> impl Iterator<Item = Event> + Send + 'static {
        self.before.clone().into_iter().chain(self.in_range())
    }

    /// Summarise into `secs`-wide bins aligned to the epoch, as
    /// `SummaryStatsPostProcessor` and `SummaryStatsCollectorEventStream`
    /// do: the event before `from` goes alone into the bin before the
    /// first; bins run to the one holding `to` (even past now); an empty
    /// bin repeats the previous one when `fill` and is left out otherwise;
    /// each is stamped at its centre.
    fn bins(&self, secs: i64, collect: Collect, fill: bool) -> Result<Vec<String>, String> {
        let first_bin = self.from_ns.div_euclid(NS).div_euclid(secs);
        let last_bin = self.to_ns.div_euclid(NS).div_euclid(secs);
        if last_bin - first_bin > MAX_BINS {
            return Err(format!("too many bins ({})", last_bin - first_bin));
        }
        let mut bins: Vec<(i64, Acc)> = Vec::new();
        if let Some(e) = &self.before {
            let mut acc = Acc::new(collect);
            acc.add(e);
            bins.push((first_bin - 1, acc));
        }
        for e in self.in_range() {
            let b = e.t_ns.div_euclid(NS).div_euclid(secs);
            match bins.last_mut() {
                Some((bin, acc)) if *bin == b => acc.add(&e),
                _ => {
                    let mut acc = Acc::new(collect);
                    acc.add(&e);
                    bins.push((b, acc));
                }
            }
        }
        let Some(start) = bins.first().map(|(b, _)| *b) else {
            return Ok(Vec::new());
        };
        let nanos = if secs % 2 == 0 { 0 } else { 500_000_000 };
        let mut out = Vec::new();
        let mut it = bins.into_iter().peekable();
        let mut previous: Option<Bin> = None;
        for b in start..=last_bin {
            let bin = match it.next_if(|(bin, _)| *bin == b) {
                Some((_, acc)) => acc.finish(),
                None if fill => match &previous {
                    Some(v) => v.clone(),
                    None => continue,
                },
                None => continue,
            };
            let t = b * secs + secs / 2;
            let mut line = format!(
                "{{ \"secs\": {t}, \"val\": {}, \"nanos\": {nanos}, \"severity\":{}, \"status\":0",
                bin.val, bin.severity
            );
            if bin.cnx {
                push_fields(&mut line, std::iter::once(("connectionChange", "true")));
            }
            line.push_str(" }");
            out.push(line);
            previous = Some(bin);
        }
        Ok(out)
    }
}

/// One bin's output.
#[derive(Clone, Debug)]
struct Bin {
    val: String,
    severity: u8,
    cnx: bool,
}

/// What a bin collects.
#[derive(Clone, Copy, Debug)]
enum Collect {
    /// `[mean, std, min, max, count]` (`optimized`).
    Vector,
    Scalar(Stat),
}

/// Running statistics for one bin (Welford; NaN values are skipped, as
/// commons-math's `SummaryStatistics` is fed by the appliance).
struct Acc {
    collect: Collect,
    n: u64,
    mean: f64,
    m2: f64,
    min: f64,
    max: f64,
    first: f64,
    last: f64,
    values: Vec<f64>,
    severity: u8,
    cnx: bool,
    events: u64,
}

impl Acc {
    fn new(collect: Collect) -> Self {
        Self {
            collect,
            n: 0,
            mean: 0.0,
            m2: 0.0,
            min: f64::NAN,
            max: f64::NAN,
            first: f64::NAN,
            last: f64::NAN,
            values: Vec::new(),
            severity: 0,
            cnx: false,
            events: 0,
        }
    }

    fn add(&mut self, e: &Event) {
        self.severity = self.severity.max(e.severity);
        self.cnx |= e.fields.iter().any(|(k, _)| *k == "cnxregainedepsecs");
        let v = e.value.as_f64().unwrap_or(f64::NAN);
        if self.events == 0 {
            self.first = v;
        }
        self.events += 1;
        self.last = v;
        if v.is_nan() {
            return;
        }
        self.n += 1;
        let d = v - self.mean;
        self.mean += d / self.n as f64;
        self.m2 += d * (v - self.mean);
        self.min = if self.n == 1 { v } else { self.min.min(v) };
        self.max = if self.n == 1 { v } else { self.max.max(v) };
        if matches!(self.collect, Collect::Scalar(Stat::Median)) {
            self.values.push(v);
        }
    }

    /// The sample variance (n - 1), 0 for one value, NaN for none.
    fn variance(&self) -> f64 {
        match self.n {
            0 => f64::NAN,
            1 => 0.0,
            n => self.m2 / (n - 1) as f64,
        }
    }

    /// The bin's `val`, severity and connection flag.
    fn finish(mut self) -> Bin {
        let mean = if self.n == 0 { f64::NAN } else { self.mean };
        let val = match self.collect {
            Collect::Vector => format!(
                "[{},{},{},{},{}]",
                java_double(mean),
                java_double(self.variance().sqrt()),
                java_double(self.min),
                java_double(self.max),
                java_double(self.n as f64)
            ),
            Collect::Scalar(stat) => java_double(match stat {
                Stat::Mean => mean,
                Stat::Min => self.min,
                Stat::Max => self.max,
                Stat::Std => self.variance().sqrt(),
                Stat::Variance => self.variance(),
                Stat::Count => self.n as f64,
                Stat::First => self.first,
                Stat::Last => self.last,
                Stat::Median => {
                    self.values.sort_by(f64::total_cmp);
                    let n = self.values.len();
                    match n {
                        0 => f64::NAN,
                        _ if n % 2 == 1 => self.values[n / 2],
                        _ => (self.values[n / 2 - 1] + self.values[n / 2]) / 2.0,
                    }
                }
            }),
        };
        Bin {
            val,
            severity: self.severity,
            cnx: self.cnx,
        }
    }
}
