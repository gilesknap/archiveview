//! A trace's data: the archived history last fetched, the live samples
//! since it was added, and the merge of the two that is drawn.
//!
//! `Samples` and the listener pattern come from bobview's `trend.rs`; live
//! samples are stamped with the IOC's timestamp (not the arrival time) so
//! they line up with the archive's.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use archiver::{Meta, Operator, Sample, ValueKind};
use parking_lot::Mutex;
use pv::{Listener, Pv, PvState};
use web_time::{SystemTime, UNIX_EPOCH};

/// Live samples kept per trace: at most this many, none older than a day.
const MAX_SAMPLES: usize = 20_000;
const MAX_AGE: f64 = 86_400.0;

/// IOC timestamps before this (1990) are not real: arrival time is used.
const SANE_TIME: f64 = 631_152_000.0;

/// The time now, in seconds since 1970.
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// One PV's live samples, in time order: (seconds since 1970, value), a
/// NaN value marking where it disconnected.
#[derive(Default)]
pub struct Samples {
    points: VecDeque<(f64, f64)>,
}

impl Samples {
    pub fn push(&mut self, t: f64, v: f64) {
        // One gap is enough, and the live data starts with a value (as in
        // Phoebus, the first disconnected state is dropped).
        if v.is_nan() && self.points.back().is_none_or(|p| p.1.is_nan()) {
            return;
        }
        // Timestamps may step back; the samples stay in order.
        let t = self.points.back().map_or(t, |p| t.max(p.0));
        self.points.push_back((t, v));
        while self.points.len() > MAX_SAMPLES
            || self.points.front().is_some_and(|p| p.0 < t - MAX_AGE)
        {
            self.points.pop_front();
        }
    }

    /// The samples from `from` on, and the one before it, which holds at
    /// `from`.
    pub fn since(&self, from: f64) -> impl Iterator<Item = (f64, f64)> + '_ {
        let i = self.points.partition_point(|p| p.0 < from);
        self.points.range(i.saturating_sub(1)..).copied()
    }
}

/// Whether `v` is an array. The `pv` crate gives an array type exactly
/// when the channel has more than one element (NELM > 1 over CA, a
/// scalar array over PVA), however many it holds now, so this is a
/// property of the channel, not of one update.
pub fn is_array(v: &pv::Value) -> bool {
    matches!(
        v,
        pv::Value::DoubleArray(_) | pv::Value::LongArray(_) | pv::Value::StrArray(_)
    )
}

/// What a PV update adds to the live samples, as [`sample`], noting in
/// `array` (for good) when the channel turns out to be an array: an
/// array's live samples are never kept.
fn observe(array: &AtomicBool, s: &PvState, arrived: f64) -> Option<(f64, f64)> {
    if is_array(&s.value) {
        array.store(true, Ordering::Relaxed);
    }
    if array.load(Ordering::Relaxed) {
        return None;
    }
    sample(s, arrived)
}

/// What a PV update adds to the live samples: its value at its IOC time,
/// a gap (at arrival time) if it disconnected, or nothing if it has no
/// number.
fn sample(s: &PvState, arrived: f64) -> Option<(f64, f64)> {
    if !s.connected {
        return Some((arrived, f64::NAN));
    }
    let v = s.value.as_f64()?;
    let t = s
        .timestamp
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .filter(|t| *t > SANE_TIME)
        .unwrap_or(arrived);
    Some((t, v))
}

/// Archived data as last fetched for a trace.
#[derive(Clone, Debug, Default)]
pub struct History {
    /// The end of the window asked for.
    pub to: f64,
    pub binned: bool,
    pub kind: ValueKind,
    pub meta: Meta,
    pub samples: Vec<Sample>,
}

impl History {
    /// Take in a refreshed tail: `samples` replace the history from where
    /// they start, the history before `keep_from` is dropped (all but the
    /// sample holding there), and the window now ends at `to`.
    pub fn append(&mut self, samples: Vec<Sample>, keep_from: f64, to: f64) {
        if let Some(first) = samples.first() {
            let cut = self.samples.partition_point(|s| s.t < first.t);
            self.samples.truncate(cut);
            self.samples.extend(samples);
        }
        let start = self
            .samples
            .partition_point(|s| s.t < keep_from)
            .saturating_sub(1);
        self.samples.drain(..start);
        self.to = self.to.max(to);
    }
}

/// Where a trace's archive fetching stands.
#[derive(Clone, Debug, Default)]
pub struct FetchState {
    /// Bumped for each request; results of older ones are dropped.
    pub generation: u64,
    /// The window and operator of the latest request (done or in flight),
    /// and when (seconds since 1970) it was made.
    pub requested: Option<Requested>,
    pub in_flight: bool,
    /// Not archived (a 404): live only, and not asked again.
    pub not_archived: bool,
    /// The last request's error.
    pub error: Option<String>,
    /// Requests failed in a row, for backing off.
    pub failures: u32,
    /// Fetch raw, never `optimized` (waveforms, strings): `None` until
    /// known from the archiver's metadata or a first response.
    pub raw_only: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Requested {
    pub from: f64,
    pub to: f64,
    pub op: Operator,
    pub at: f64,
}

/// A PV being plotted.
pub struct Trace {
    /// Tells it from the others, for fetch results.
    pub id: u64,
    /// As added (`X`, `pva://X`, `sim://sine`).
    pub name: String,
    pub pv: Pv,
    /// Locked by the listener while the PV's state is locked, so never
    /// read `pv.state()` while holding this lock.
    live: Arc<Mutex<Samples>>,
    /// The channel is an array (seen live): not plotted.
    array: Arc<AtomicBool>,
    _listener: Listener,
    /// A colour the view asked for, as `#rrggbb`.
    pub color: Option<String>,
    /// Its palette colour, when it has no `color`.
    pub slot: usize,
    pub visible: bool,
    pub history: Option<History>,
    pub fetch: FetchState,
}

impl Trace {
    pub fn new(id: u64, name: &str, pv: Pv) -> Self {
        let live = Arc::new(Mutex::new(Samples::default()));
        let into = live.clone();
        let array = Arc::new(AtomicBool::new(false));
        let is_array = array.clone();
        let listener: Listener = Arc::new(move |s: &PvState| {
            if let Some((t, v)) = observe(&is_array, s, now()) {
                into.lock().push(t, v);
            }
        });
        pv.listen(&listener);
        Self {
            id,
            name: name.to_string(),
            pv,
            live,
            array,
            _listener: listener,
            color: None,
            slot: 0,
            visible: true,
            history: None,
            fetch: FetchState::default(),
        }
    }

    /// The name the archiver knows the PV by: without `ca://` or
    /// `pva://`; `None` for PVs no archiver has (`sim://`, `loc://`,
    /// formulas).
    pub fn archive_name(&self) -> Option<String> {
        let name = self.pv.name();
        matches!(name.scheme, pv::Scheme::Ca | pv::Scheme::Pva).then(|| name.pv.clone())
    }

    /// An array PV, known from its live channel or from the archive (a
    /// waveform there, even when it held one element): not plotted.
    pub fn is_array(&self) -> bool {
        self.array.load(Ordering::Relaxed)
            || self
                .history
                .as_ref()
                .is_some_and(|h| h.kind == ValueKind::Waveform)
    }

    /// The name to show: no `ca://` prefix, no `<type>` suffix.
    pub fn label(&self) -> String {
        plain_name(self.pv.name())
    }

    /// The points to draw from `from` to `to` at time `now`: history up to
    /// the first live sample, then the live samples, the last value held
    /// to now while connected.
    pub fn points(&self, from: f64, to: f64, now: f64) -> Vec<Pt> {
        // Copied out before the PV's state is read: an update holds that
        // lock while it adds a sample.
        if self.is_array() {
            return Vec::new();
        }
        let live: Vec<(f64, f64)> = self.live.lock().since(from).collect();
        let connected = self.pv.connected();
        let (hist, binned, hist_to): (&[Sample], bool, f64) = match &self.history {
            Some(h) => (&h.samples, h.binned, h.to),
            None => (&[], false, f64::NEG_INFINITY),
        };
        let hold = if connected && !live.is_empty() {
            now
        } else if live.is_empty() {
            hist_to.min(now)
        } else {
            // Disconnected: the gap ends the line.
            f64::NEG_INFINITY
        };
        merge(hist, binned, &live, from, to, hold)
    }
}

/// A PV name as shown: `ca://` and any `<type>` suffix dropped (bobview's
/// `plain_name`).
pub fn plain_name(name: &pv::PvName) -> String {
    match name.scheme {
        pv::Scheme::Ca => name.base().to_string(),
        pv::Scheme::Formula | pv::Scheme::Unsupported => name.to_string(),
        _ => {
            let full = name.to_string();
            let prefix = &full[..full.len() - name.pv.len()];
            format!("{prefix}{}", name.base())
        }
    }
}

/// A point to draw: the value `v` at `t`, with the `lo..hi` envelope of a
/// bin (equal to `v` for a raw sample). `step`: the value holds flat until
/// the next point; else the line slopes to it (bin means). A NaN `v` is a
/// gap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub t: f64,
    pub v: f64,
    pub lo: f64,
    pub hi: f64,
    pub step: bool,
}

impl Pt {
    pub fn raw(t: f64, v: f64) -> Self {
        Self {
            t,
            v,
            lo: v,
            hi: v,
            step: true,
        }
    }

    pub fn is_bin(&self) -> bool {
        self.lo < self.hi
    }
}

/// Merge archived `history` and `live` samples for `from..to`.
///
/// - History is cut at the first live sample (live wins at the right edge,
///   as in Phoebus' `PlotSampleMerger`); its last point then holds flat to
///   the first live one.
/// - Each part starts with the point that holds at `from` and ends with
///   the first after `to`.
/// - The last value is held to `hold` (now while live, the end of the
///   fetched window for history alone) when that is later.
pub fn merge(
    history: &[Sample],
    binned: bool,
    live: &[(f64, f64)],
    from: f64,
    to: f64,
    hold: f64,
) -> Vec<Pt> {
    let border = live.first().map_or(f64::INFINITY, |p| p.0);
    let hist = &history[..history.partition_point(|s| s.t < border)];
    let mut out: Vec<Pt> = Vec::new();
    for s in visible(hist, |s| s.t, from, to) {
        out.push(if binned && !s.is_gap() {
            Pt {
                t: s.t,
                v: s.val,
                lo: s.min.min(s.val),
                hi: s.max.max(s.val),
                step: false,
            }
        } else {
            Pt::raw(s.t, s.val)
        });
    }
    let live = visible(live, |p| p.0, from, to);
    if !live.is_empty()
        && let Some(last) = out.last_mut()
    {
        last.step = true;
    }
    out.extend(live.iter().map(|&(t, v)| Pt::raw(t, v)));
    if let Some(&last) = out.last()
        && last.v.is_finite()
        && hold > last.t
    {
        out.push(Pt {
            t: hold,
            ..Pt::raw(hold, last.v)
        });
    }
    out
}

/// The part of `items` (in time order) that shows in `from..to`: from the
/// one that holds at `from` to the first after `to`.
fn visible<T>(items: &[T], t: impl Fn(&T) -> f64, from: f64, to: f64) -> &[T] {
    let start = items.partition_point(|x| t(x) < from).saturating_sub(1);
    let end = (items.partition_point(|x| t(x) <= to) + 1).min(items.len());
    if start >= end {
        return &[];
    }
    &items[start..end]
}

/// The point whose value the line shows at `t`: for raw samples (drawn as
/// steps) the last one at or before `t`, which may be a gap; for bin means
/// (sloped lines) the nearest bin with a value. `None` before the data.
pub fn at_time(points: &[Pt], t: f64) -> Option<usize> {
    let i = points.partition_point(|p| p.t <= t);
    match i.checked_sub(1).map(|j| (j, points[j])) {
        Some((j, p)) if p.step => Some(j),
        Some(_) => nearest(points, t),
        None => points.first().filter(|p| !p.step).and(nearest(points, t)),
    }
}

/// The index of the point nearest `t` with a value.
pub fn nearest(points: &[Pt], t: f64) -> Option<usize> {
    let i = points.partition_point(|p| p.t < t);
    let mut best: Option<usize> = None;
    let mut look = |j: usize| {
        if points[j].v.is_finite()
            && best.is_none_or(|b| (points[j].t - t).abs() < (points[b].t - t).abs())
        {
            best = Some(j);
        }
    };
    // The nearest finite point either side (gaps skipped).
    if let Some(j) = (0..i.min(points.len()))
        .rev()
        .find(|&j| points[j].v.is_finite())
    {
        look(j);
    }
    if let Some(j) = (i..points.len()).find(|&j| points[j].v.is_finite()) {
        look(j);
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(t: f64, v: f64) -> Sample {
        Sample::raw(t, v)
    }

    #[test]
    fn samples_keep_gaps_once_and_stay_in_order() {
        let mut s = Samples::default();
        s.push(1.0, f64::NAN);
        assert!(s.points.is_empty(), "live data starts with a value");
        s.push(1.0, 5.0);
        s.push(2.0, f64::NAN);
        s.push(3.0, f64::NAN);
        s.push(1.5, 6.0);
        let pts: Vec<(f64, f64)> = s.since(0.0).collect();
        assert_eq!(pts.len(), 3);
        assert!(pts[1].1.is_nan());
        assert_eq!(pts[2], (2.0, 6.0), "a clock stepping back keeps the order");
    }

    #[test]
    fn samples_are_bounded() {
        let mut s = Samples::default();
        for i in 0..MAX_SAMPLES + 10 {
            s.push(i as f64 * 0.001, i as f64);
        }
        assert_eq!(s.points.len(), MAX_SAMPLES);
        s.push(MAX_AGE + 200.0, 1.0);
        assert_eq!(s.points.len(), 1);
    }

    #[test]
    fn live_samples_use_the_ioc_time() {
        let at = |secs: u64| Some(UNIX_EPOCH + std::time::Duration::from_secs(secs));
        let state = |timestamp| PvState {
            connected: true,
            value: pv::Value::Double(2.0),
            timestamp,
            ..Default::default()
        };
        assert_eq!(sample(&state(at(1_700_000_000)), 5e9), Some((1.7e9, 2.0)));
        // No time, or a bogus one: when it arrived.
        assert_eq!(sample(&state(None), 5e9), Some((5e9, 2.0)));
        assert_eq!(sample(&state(at(10)), 5e9), Some((5e9, 2.0)));
        let gap = sample(&PvState::default(), 5e9).unwrap();
        assert!(gap.0 == 5e9 && gap.1.is_nan());
        let text = PvState {
            connected: true,
            value: pv::Value::Str("idle".into()),
            ..Default::default()
        };
        assert_eq!(sample(&text, 5e9), None);
    }

    #[test]
    fn arrays_are_not_plotted() {
        let state = |value| PvState {
            connected: true,
            value,
            ..Default::default()
        };
        let array = AtomicBool::new(false);
        // A scalar channel: its values are kept.
        let scalar = state(pv::Value::Double(1.0));
        assert_eq!(observe(&array, &scalar, 5e9), Some((5e9, 1.0)));
        assert!(!array.load(Ordering::Relaxed));
        // A variable-length array, 1 element, then 3, then 1 again: an
        // array throughout, never a sample (not even a stale element 0).
        for n in [1, 3, 1] {
            let wave = state(pv::Value::DoubleArray(vec![2.0; n].into()));
            assert!(is_array(&wave.value));
            assert_eq!(observe(&array, &wave, 5e9), None, "{n} elements");
        }
        assert!(array.load(Ordering::Relaxed));
        // Once an array, always: a disconnect or a scalar adds nothing.
        assert_eq!(observe(&array, &PvState::default(), 5e9), None);
        assert_eq!(observe(&array, &scalar, 5e9), None);
    }

    #[test]
    fn history_is_cut_at_the_first_live_sample() {
        let hist: Vec<Sample> = (0..10).map(|i| raw(i as f64 * 10.0, i as f64)).collect();
        let live = [(55.0, 100.0), (70.0, 101.0)];
        let pts = merge(&hist, false, &live, 0.0, 100.0, 80.0);
        let ts: Vec<f64> = pts.iter().map(|p| p.t).collect();
        assert_eq!(ts, [0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 55.0, 70.0, 80.0]);
        // The last archived value holds to the first live one; the last
        // live value to `hold`.
        assert!(pts[5].step);
        assert_eq!(pts.last().unwrap().v, 101.0);
    }

    #[test]
    fn a_refreshed_tail_is_added_and_the_old_head_dropped() {
        let mut h = History {
            to: 100.0,
            samples: (0..10).map(|i| raw(i as f64 * 10.0, i as f64)).collect(),
            ..History::default()
        };
        // The tail starts with the last sample again, as the archiver
        // sends the one holding at `from`.
        h.append(
            vec![raw(90.0, 9.0), raw(95.0, 10.0), raw(120.0, 11.0)],
            35.0,
            150.0,
        );
        let ts: Vec<f64> = h.samples.iter().map(|s| s.t).collect();
        assert_eq!(ts, [30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 95.0, 120.0]);
        assert_eq!(h.to, 150.0);
        // Nothing new: the window still moves on.
        h.append(Vec::new(), 0.0, 180.0);
        assert_eq!((h.samples.len(), h.to), (9, 180.0));
    }

    #[test]
    fn merge_keeps_the_point_holding_at_the_start() {
        let hist: Vec<Sample> = (0..10).map(|i| raw(i as f64 * 10.0, i as f64)).collect();
        let pts = merge(&hist, false, &[], 35.0, 62.0, 0.0);
        let ts: Vec<f64> = pts.iter().map(|p| p.t).collect();
        // 30 holds at 35; 70 is the first after 62 (so the line reaches the edge).
        assert_eq!(ts, [30.0, 40.0, 50.0, 60.0, 70.0]);
        // History alone holds to the end of its window.
        let pts = merge(&hist, false, &[], 0.0, 200.0, 150.0);
        assert_eq!(pts.last().map(|p| (p.t, p.v)), Some((150.0, 9.0)));
        assert!(merge(&hist, false, &[], 500.0, 600.0, 0.0).len() <= 2);
        assert!(merge(&[], false, &[], 0.0, 1.0, 5.0).is_empty());
    }

    #[test]
    fn bins_carry_their_envelope() {
        let bin = Sample {
            min: 1.0,
            max: 9.0,
            count: 10,
            ..raw(5.0, 4.0)
        };
        let pts = merge(&[bin, Sample::gap(6.0)], true, &[], 0.0, 10.0, 0.0);
        assert!(pts[0].is_bin() && !pts[0].step);
        assert_eq!((pts[0].lo, pts[0].hi), (1.0, 9.0));
        assert!(pts[1].v.is_nan() && !pts[1].is_bin());
        assert_eq!(pts.len(), 2, "no hold after a gap");
    }

    #[test]
    fn nearest_skips_gaps() {
        let pts = [
            Pt::raw(0.0, 1.0),
            Pt::raw(10.0, f64::NAN),
            Pt::raw(20.0, 3.0),
        ];
        assert_eq!(nearest(&pts, 9.0), Some(0));
        assert_eq!(nearest(&pts, 11.0), Some(2));
        assert_eq!(nearest(&pts, 100.0), Some(2));
        assert_eq!(nearest(&pts, -5.0), Some(0));
        assert_eq!(nearest(&[], 1.0), None);
    }

    #[test]
    fn the_readout_follows_the_steps() {
        let pts = [
            Pt::raw(0.0, 1.0),
            Pt::raw(100.0, 5.0),
            Pt::raw(150.0, f64::NAN),
            Pt::raw(200.0, 7.0),
        ];
        // A raw value holds until the next sample, whichever is nearer.
        assert_eq!(at_time(&pts, 60.0), Some(0));
        assert_eq!(at_time(&pts, 100.0), Some(1));
        // In a gap: the gap ("—"), not the value across it.
        assert_eq!(at_time(&pts, 190.0), Some(2));
        assert_eq!(at_time(&pts, -1.0), None);
        // Bin means: the nearest.
        let bin = |t: f64, v: f64| Pt {
            lo: v - 1.0,
            hi: v + 1.0,
            step: false,
            ..Pt::raw(t, v)
        };
        let bins = [bin(0.0, 1.0), bin(10.0, 2.0)];
        assert_eq!(at_time(&bins, 7.0), Some(1));
        assert_eq!(at_time(&bins, -2.0), Some(0));
    }

    #[test]
    fn names_drop_ca_and_types() {
        let n = |s: &str| plain_name(&pv::PvName::parse(s));
        assert_eq!(n("ca://A:B"), "A:B");
        assert_eq!(n("A:B"), "A:B");
        assert_eq!(n("pva://A:B"), "pva://A:B");
        assert_eq!(n("loc://x<VDouble>(3)"), "loc://x");
    }
}
