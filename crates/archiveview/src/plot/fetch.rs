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
}

/// A finished data request.
pub struct Done {
    pub trace: u64,
    pub generation: u64,
    /// The window fetched (or asked for, on an error).
    pub from: f64,
    pub to: f64,
    pub op: Operator,
    pub info: Option<PvInfo>,
    pub result: Result<Option<TimeSeries>, archiver::Error>,
}

/// A finished search: its number and the names (or an error).
pub type Found = (u64, Result<Vec<String>, String>);

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
    busy: Arc<AtomicUsize>,
}

impl Fetcher {
    /// Requests in flight (data and search).
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed) > 0
    }

    /// Requests in flight, or results not yet taken in.
    pub fn pending(&self) -> bool {
        self.busy() || !self.inbox.lock().is_empty() || self.found.lock().is_some()
    }

    pub fn take(&self) -> Vec<Done> {
        std::mem::take(&mut *self.inbox.lock())
    }

    pub fn take_found(&self) -> Option<Found> {
        self.found.lock().take()
    }

    /// Run `job` against `client` in the background.
    pub fn fetch(
        &self,
        pvs: &PvManager,
        client: &ArchiverClient,
        job: Job,
        repaint: Option<Repaint>,
    ) {
        let client = client.clone();
        let inbox = self.inbox.clone();
        let (trace, generation, from, to, op) =
            (job.trace, job.generation, job.from, job.to, job.op);
        let panicked = {
            let inbox = inbox.clone();
            move || {
                inbox.lock().push(Done {
                    trace,
                    generation,
                    from,
                    to,
                    op,
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
                    Err(e) if e.is_not_archived() => {
                        inbox.lock().push(Done {
                            trace: job.trace,
                            generation: job.generation,
                            from: job.from,
                            to: job.to,
                            op: job.op,
                            info: None,
                            result: Err(check_404(&client, &job.pv, e)),
                        });
                        return;
                    }
                    // Not every server has getMetadata: the data says.
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
            let result = client
                .get_data(&Query::new(&job.pv, from, job.to, op))
                .map_err(|e| {
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
        let store = move |names: Result<Vec<String>, String>| {
            // Searches can finish out of order: keep only the newest answer
            // not yet taken in, so an older one never hides a newer one.
            let mut slot = found.lock();
            if slot.as_ref().is_none_or(|(m, _)| n > *m) {
                *slot = Some((n, names));
            }
        };
        let panicked = {
            let store = store.clone();
            move || store(Err("the search failed unexpectedly".into()))
        };
        self.run(pvs, repaint, panicked, move || {
            store(client.search(&search).map_err(|e| e.to_string()));
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

/// A 404 means "not archived" only when the archiver itself said so. With
/// a wrong `--archiver` base (port or context path) Tomcat answers 404 to
/// every URL, so check with a search, which any archiver answers: if that
/// fails too, the URL is wrong, and that is reported as an error.
fn check_404(client: &ArchiverClient, pv: &str, e: archiver::Error) -> archiver::Error {
    let probe = Search {
        pattern: pv.to_string(),
        mode: archiver::SearchMode::Glob,
        limit: 1,
    };
    match client.search(&probe) {
        Ok(_) => e,
        Err(check) => {
            log::debug!("{pv}: {e}; the PV search: {check}");
            archiver::Error::BadUrl(format!(
                "{}: not an archiver (HTTP 404 for data and search)",
                client.base()
            ))
        }
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
        let n = (width_px.max(1.0) * (1.0 + 2.0 * PAD as f32)).round() as u32;
        Operator::Optimized(n.clamp(1, archiver::MAX_BINS))
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
}
