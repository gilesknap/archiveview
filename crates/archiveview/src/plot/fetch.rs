//! Background archive requests. Each runs as a blocking task on the pv
//! crate's tokio runtime (the caller's `PvManager`), so a plot starts no
//! threads of its own; results come back through a shared inbox, and the
//! plot's context is asked to repaint.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use archiver::{ArchiverClient, Operator, PvInfo, Query, Search, TimeSeries};
use parking_lot::Mutex;
use pv::PvManager;

/// The padding fetched either side of the visible window, as a fraction of
/// its span, so a small pan or zoom has data at once.
pub const PAD: f64 = 0.25;

/// A data request for one trace.
pub struct Job {
    pub trace: u64,
    pub generation: u64,
    pub pv: String,
    pub from: f64,
    pub to: f64,
    pub op: Operator,
    /// Where to start instead if the metadata says to fetch raw: no more
    /// than [`RAW_MAX_SPAN`] (padded) of a wide window.
    pub raw_from: f64,
    /// Ask `getMetadata` first, to learn whether to fetch raw.
    pub probe: bool,
    /// Only the tail of the history (a live refresh): its samples are
    /// added to the history, not put in its place.
    pub append: bool,
}

/// A data job as sent: (trace, from, to, append).
#[cfg(test)]
pub type Sent = (u64, f64, f64, bool);

/// A finished data request.
pub struct Done {
    pub trace: u64,
    pub generation: u64,
    /// The window fetched (or asked for, on an error).
    pub from: f64,
    pub to: f64,
    pub op: Operator,
    /// As [`Job::append`].
    pub append: bool,
    pub info: Option<PvInfo>,
    pub result: Result<Option<TimeSeries>, archiver::Error>,
}

/// A finished search: its number and the names (or an error).
pub type Found = (u64, Result<Vec<String>, archiver::Error>);

/// A finished archiver probe: the archiver's epoch, and its version
/// (`None` when it answers, but not to `getVersion`) or the error.
pub type Probed = (u64, Result<Option<String>, archiver::Error>);

/// Where to ask for a repaint when a request finishes: the context and the
/// viewport the plot was last drawn in. A host such as bobview may show
/// the plot in its own viewport, and a plain `request_repaint` from a
/// worker thread repaints only the root one.
#[derive(Clone)]
pub struct Repaint {
    pub ctx: egui::Context,
    pub viewport: egui::ViewportId,
}

impl Repaint {
    fn now(&self) {
        self.ctx.request_repaint_of(self.viewport);
    }
}

/// The plot's requests in flight and the results not yet taken in.
#[derive(Clone, Default)]
pub struct Fetcher {
    inbox: Arc<Mutex<Vec<Done>>>,
    found: Arc<Mutex<Option<Found>>>,
    probed: Arc<Mutex<Option<Probed>>>,
    busy: Arc<AtomicUsize>,
    /// Every data job sent, for tests.
    #[cfg(test)]
    pub sent: Arc<Mutex<Vec<Sent>>>,
}

impl Fetcher {
    /// Requests in flight (data and search).
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed) > 0
    }

    /// How many requests are in flight.
    #[cfg(test)]
    pub fn in_flight(&self) -> usize {
        self.busy.load(Ordering::Relaxed)
    }

    /// Requests in flight, or results not yet taken in.
    pub fn pending(&self) -> bool {
        self.busy()
            || !self.inbox.lock().is_empty()
            || self.found.lock().is_some()
            || self.probed.lock().is_some()
    }

    pub fn take(&self) -> Vec<Done> {
        std::mem::take(&mut *self.inbox.lock())
    }

    pub fn take_found(&self) -> Option<Found> {
        self.found.lock().take()
    }

    pub fn take_probed(&self) -> Option<Probed> {
        self.probed.lock().take()
    }

    /// Run `job` against `client` in the background.
    pub fn fetch(
        &self,
        pvs: &PvManager,
        client: &ArchiverClient,
        job: Job,
        repaint: Option<Repaint>,
    ) {
        #[cfg(test)]
        self.sent
            .lock()
            .push((job.trace, job.from, job.to, job.append));
        let client = client.clone();
        let inbox = self.inbox.clone();
        let (trace, generation, from, to, op, append) = (
            job.trace,
            job.generation,
            job.from,
            job.to,
            job.op,
            job.append,
        );
        let panicked = {
            let inbox = inbox.clone();
            move || {
                inbox.lock().push(Done {
                    trace,
                    generation,
                    from,
                    to,
                    op,
                    append,
                    info: None,
                    result: Err(archiver::Error::Transport(
                        "the request failed unexpectedly".into(),
                    )),
                });
            }
        };
        self.run(pvs, repaint, panicked, move || {
            let info = if job.probe {
                match client.metadata(&job.pv) {
                    Ok(info) => Some(info),
                    // Not every server answers getMetadata, and a 404 from
                    // it doesn't prove the PV isn't archived (a proxy may
                    // not pass it on, or a cluster member may not know the
                    // PV): the data request decides.
                    Err(e) => {
                        log::debug!("{}: no metadata: {e}", job.pv);
                        None
                    }
                }
            } else {
                None
            };
            let (op, from) = match &info {
                Some(i) if !i.binnable() => (Operator::Raw, job.raw_from.max(job.from)),
                _ => (job.op, job.from),
            };
            // An array is not plotted: the metadata says it is archived,
            // and that is all the plot needs. A day of a busy waveform
            // could be gigabytes.
            let result = if let Some(series) = info.as_ref().and_then(|i| array_series(&job.pv, i))
            {
                Ok(Some(series))
            } else {
                client.get_data(&Query::new(&job.pv, from, job.to, op))
            };
            let result = result.map_err(|e| {
                log::debug!("{}: {e}", job.pv);
                if e.is_not_archived() {
                    check_404(&client, &job.pv, e)
                } else {
                    e
                }
            });
            inbox.lock().push(Done {
                trace: job.trace,
                generation: job.generation,
                from,
                to: job.to,
                op,
                append: job.append,
                info,
                result,
            });
        });
    }

    /// Search `client` for `search`, as request number `n`.
    pub fn search(
        &self,
        pvs: &PvManager,
        client: &ArchiverClient,
        search: Search,
        n: u64,
        repaint: Option<Repaint>,
    ) {
        let client = client.clone();
        let found = self.found.clone();
        let store = move |names: Result<Vec<String>, archiver::Error>| {
            // Searches can finish out of order: keep only the newest answer
            // not yet taken in, so an older one never hides a newer one.
            let mut slot = found.lock();
            if slot.as_ref().is_none_or(|(m, _)| n > *m) {
                *slot = Some((n, names));
            }
        };
        let panicked = {
            let store = store.clone();
            move || {
                store(Err(archiver::Error::Transport(
                    "the search failed unexpectedly".into(),
                )))
            }
        };
        self.run(pvs, repaint, panicked, move || {
            store(client.search(&search));
        });
    }

    /// Ask `client` for its version, to learn whether it can be reached:
    /// the archiver's `epoch` comes back with the answer. An archiver
    /// that answers `getVersion` with an HTTP error (a proxy passing on
    /// only data and search) is asked for a search instead.
    pub fn probe(
        &self,
        pvs: &PvManager,
        client: &ArchiverClient,
        epoch: u64,
        repaint: Option<Repaint>,
    ) {
        let client = client
            .clone()
            .with_timeout_secs(super::status::PROBE_TIMEOUT);
        let probed = self.probed.clone();
        let panicked = {
            let probed = probed.clone();
            move || {
                store_probe(
                    &probed,
                    (
                        epoch,
                        Err(archiver::Error::Transport(
                            "the check failed unexpectedly".into(),
                        )),
                    ),
                );
            }
        };
        self.run(pvs, repaint, panicked, move || {
            let result = probe(&client);
            if let Err(e) = &result {
                log::info!("archiver {}: {e}", client.base());
            }
            store_probe(&probed, (epoch, result));
        });
    }

    /// Run `work` as a blocking task, `panicked` instead reporting its
    /// failure if it panics. The busy count goes back down and the plot
    /// is repainted however `work` ends.
    fn run(
        &self,
        pvs: &PvManager,
        repaint: Option<Repaint>,
        panicked: impl FnOnce() + Send + 'static,
        work: impl FnOnce() + Send + 'static,
    ) {
        /// Undoes the busy count (and repaints) on drop, panic or not.
        struct Busy(Arc<AtomicUsize>, Option<Repaint>);
        impl Drop for Busy {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::Relaxed);
                if let Some(r) = &self.1 {
                    r.now();
                }
            }
        }
        self.busy.fetch_add(1, Ordering::Relaxed);
        let busy = Busy(self.busy.clone(), repaint);
        let task = move || {
            let _busy = busy;
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
                log::error!("an archiver request panicked");
                panicked();
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        drop(pvs.runtime().spawn_blocking(task));
        #[cfg(target_arch = "wasm32")]
        {
            // No blocking HTTP in a browser (yet): live only.
            let _ = (pvs, panicked);
            drop(task);
        }
    }
}

/// Keep a probe's answer for the plot, unless one about a newer archiver
/// is waiting: an answer about an earlier archiver arriving late must not
/// hide the current one's (the plot may not have been drawn in between).
fn store_probe(slot: &Mutex<Option<Probed>>, probed: Probed) {
    let mut slot = slot.lock();
    if slot.as_ref().is_none_or(|(epoch, _)| probed.0 >= *epoch) {
        *slot = Some(probed);
    }
}

/// Probe `client`: its version, or `None` when it answers HTTP but not to
/// `getVersion` (a proxy passing on only data and search, or refusing it
/// with 401 or 403), as long as a search answers.
fn probe(client: &ArchiverClient) -> Result<Option<String>, archiver::Error> {
    let e = match client.version() {
        Ok(v) => return Ok(Some(v)),
        Err(e @ archiver::Error::Http { status, .. }) if !(502..=504).contains(&status) => e,
        Err(e) => return Err(e),
    };
    log::debug!("{}: no getVersion: {e}", client.base());
    match client.search(&Search::exact("archiveview", 1)) {
        Ok(_) => Ok(None),
        // 404 for both: nothing is served here.
        Err(check) if check.is_not_archived() && e.is_not_archived() => Err(
            archiver::Error::BadUrl(format!("{}: not an archiver (HTTP 404)", client.base())),
        ),
        Err(check) if super::status::archiver_fault(&check, false) => Err(check),
        // Something answers, if not to these.
        Err(_) => Ok(None),
    }
}

/// A 404 means "not archived" only when the archiver itself said so. With
/// a wrong `--archiver` base (port or context path) Tomcat answers 404 to
/// every URL, so check with a search for the exact name, which any
/// archiver answers: only if that is a 404 too is the URL wrong, and that
/// is reported as an error. Any other failure of the check (a timeout, a
/// server error) proves nothing, so the PV stays "not archived".
fn check_404(client: &ArchiverClient, pv: &str, e: archiver::Error) -> archiver::Error {
    match client.search(&Search::exact(pv, 1)) {
        Err(check) if check.is_not_archived() => {
            log::debug!("{pv}: {e}; the PV search: {check}");
            archiver::Error::BadUrl(format!(
                "{}: not an archiver (HTTP 404 for data and search)",
                client.base()
            ))
        }
        Err(check) => {
            log::debug!("{pv}: {e}; the PV search failed too: {check}");
            e
        }
        Ok(_) => e,
    }
}

/// The window to fetch for a visible `from..to`: padded by [`PAD`] of the
/// span either side.
pub fn padded(from: f64, to: f64) -> (f64, f64) {
    let pad = (to - from) * PAD;
    (from - pad, to + pad)
}

/// The operator for a plot `width_px` device pixels wide showing part of a
/// padded window: about one bin per device pixel of the padded window (at
/// most `MAX_BINS`), or raw. Finer bins only alias: the means zig-zag into
/// a solid block where the envelope already shows the extremes.
pub fn operator(width_px: f32, raw: bool) -> Operator {
    if raw {
        Operator::Raw
    } else {
        Operator::for_width(width_px.max(1.0) * (1.0 + 2.0 * PAD as f32))
    }
}

/// The longest span fetched raw, for traces that cannot be binned
/// (waveforms, text, enums): a week of a busy waveform could be gigabytes.
/// Wider windows show only the latest part of their history.
pub const RAW_MAX_SPAN: f64 = 86_400.0;

/// The start of the history a trace needs for a visible `from..to`: all of
/// it, or for a raw-only trace no more than [`RAW_MAX_SPAN`].
pub fn needed_from(from: f64, to: f64, raw: bool) -> f64 {
    if raw {
        from.max(to - RAW_MAX_SPAN)
    } else {
        from
    }
}

/// Refresh binned history no more often than every this many bin widths
/// while following now.
const BINS_PER_REFRESH: f64 = 3.0;

/// How long after fetching `got` with `op` to refresh it while following
/// now: `base` for raw history (only its tail is fetched, which is
/// cheap), but for bins no sooner than [`BINS_PER_REFRESH`] bin widths.
/// Refetching a week of bins every 30 s would load the appliance for no
/// visible change: a bin is minutes wide, and the right edge is drawn from
/// live data anyway.
pub fn refresh_after(base: f64, got: (f64, f64), op: Operator, binned: bool) -> f64 {
    match op {
        Operator::Optimized(n) if binned && n > 0 => {
            let bin = (got.1 - got.0) / f64::from(n);
            (BINS_PER_REFRESH * bin).max(base)
        }
        _ => base,
    }
}

/// Whether raw history fetched as `optimized_N` (because there were at
/// most N samples) has grown, through live tail refreshes, past N samples:
/// then it should be fetched whole again, to come back binned, rather than
/// grow without end.
pub fn outgrown(op: Option<Operator>, len: usize) -> bool {
    matches!(op, Some(Operator::Optimized(n)) if len > n as usize)
}

/// How long to wait before asking again after `failures` errors in a row:
/// `base`, doubling each time, at most five minutes.
pub fn backoff(base: f64, failures: u32) -> f64 {
    (base * 2f64.powi(failures.saturating_sub(1).min(10) as i32)).min(300.0)
}

/// Whether data fetched for `got` (and binned or not) still serves the
/// visible `from..to`: it covers it, and (for bins) it is not zoomed in so
/// far that the bins show as steps.
pub fn covers(got: (f64, f64), binned: bool, from: f64, to: f64) -> bool {
    let (a, b) = got;
    let inside = a <= from && b >= to;
    let fine_enough = !binned || (to - from) / (b - a) >= 0.35;
    inside && fine_enough
}

/// For an array PV (more than one element, by its metadata): an empty
/// waveform series standing for its history, which is not fetched.
fn array_series(pv: &str, info: &PvInfo) -> Option<TimeSeries> {
    info.element_count
        .is_some_and(|n| n > 1)
        .then(|| TimeSeries {
            meta: archiver::Meta {
                name: pv.to_string(),
                egu: info.units.clone(),
                prec: info.precision,
                ..Default::default()
            },
            kind: archiver::ValueKind::Waveform,
            binned: false,
            samples: Vec::new(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_are_padded_and_binned_to_the_width() {
        assert_eq!(padded(100.0, 200.0), (75.0, 225.0));
        // 1000 visible pixels in a window 1.5 times as wide: 1500 bins,
        // one per pixel of the padded window.
        assert_eq!(operator(1000.0, false), Operator::Optimized(1500));
        assert_eq!(operator(4000.0, false), Operator::Optimized(4000));
        assert_eq!(operator(1000.0, true), Operator::Raw);
    }

    #[test]
    fn refetch_when_out_of_cover_or_too_coarse() {
        let got = padded(100.0, 200.0);
        assert!(covers(got, true, 100.0, 200.0));
        // Panned within the padding.
        assert!(covers(got, true, 120.0, 220.0));
        assert!(!covers(got, true, 150.0, 250.0));
        // Zoomed in on bins: refetch for finer ones; raw data never needs it.
        assert!(!covers(got, true, 140.0, 160.0));
        assert!(covers(got, false, 140.0, 160.0));
    }

    #[test]
    fn raw_history_is_capped_and_errors_back_off() {
        let week = 7.0 * 86_400.0;
        assert_eq!(needed_from(0.0, week, false), 0.0);
        assert_eq!(needed_from(0.0, week, true), week - RAW_MAX_SPAN);
        assert_eq!(needed_from(100.0, 200.0, true), 100.0);
        assert_eq!(backoff(10.0, 1), 10.0);
        assert_eq!(backoff(10.0, 3), 40.0);
        assert_eq!(backoff(10.0, 40), 300.0);
    }

    #[test]
    fn binned_history_refreshes_at_the_bin_pace() {
        let week = padded(0.0, 7.0 * 86_400.0);
        // 1500 bins over 10.5 days: 605 s each, refreshed every 3 of them.
        let every = refresh_after(30.0, week, Operator::Optimized(1500), true);
        assert!((every - 1814.4).abs() < 1e-6, "{every}");
        // Fine bins, raw history, and raw answers to `optimized`: 30 s.
        let hour = padded(0.0, 3600.0);
        assert_eq!(
            refresh_after(30.0, hour, Operator::Optimized(1500), true),
            30.0
        );
        assert_eq!(refresh_after(30.0, week, Operator::Raw, false), 30.0);
        assert_eq!(
            refresh_after(30.0, week, Operator::Optimized(1500), false),
            30.0
        );
    }

    #[test]
    fn a_late_probe_answer_does_not_hide_a_newer_one() {
        let slot = Mutex::new(None);
        let down = || Err(archiver::Error::Transport("down".into()));
        // Archiver 2's answer waits; archiver 1's comes late.
        store_probe(&slot, (2, Ok(Some("v2".into()))));
        store_probe(&slot, (1, down()));
        assert_eq!(slot.lock().take(), Some((2, Ok(Some("v2".into())))));
        // In order, the newer replaces the older.
        store_probe(&slot, (1, down()));
        store_probe(&slot, (2, Ok(None)));
        assert_eq!(slot.lock().take(), Some((2, Ok(None))));
    }

    #[test]
    fn raw_history_past_n_samples_is_rebinned() {
        let op = Some(Operator::Optimized(100));
        assert!(!outgrown(op, 100));
        assert!(outgrown(op, 101));
        assert!(!outgrown(Some(Operator::Raw), 1_000_000));
        assert!(!outgrown(None, 1_000_000));
    }
}
