//! [`Plot`]: the embeddable archived + live time-series widget.
//!
//! See DESIGN.md ("Embedding", "Data flow" and "UI") for the interaction
//! model, fetch policy and live merge. The pieces:
//!
//! - `series`: a trace's live samples, archived history, and their merge;
//! - `fetch`: background archive requests on the pv crate's runtime;
//! - `window`: time window and value range maths (pan, zoom, presets);
//! - `timeaxis`: ticks and adaptive labels in local time;
//! - `render`: painting the plot area;
//! - `bar`: the slim top bar (search, range pill, live pill, archiver
//!   pill, buttons);
//! - `legend`: the chips under the plot;
//! - `hover`: the crosshair readout, empty state and status line;
//! - `axes`: which value axis a trace joins, and the axes' names;
//! - `status`: whether the archiver can be reached;
//! - `paste`: adding PVs from the primary selection or the clipboard;
//! - `undo`: undo and redo;
//! - `pane`: the plots stacked on the one time axis, each with its own
//!   value axes.

mod axes;
mod axisui;
mod bar;
mod fetch;
mod hover;
mod legend;
mod pane;
mod paste;
mod render;
mod series;
mod status;
mod timeaxis;
mod undo;
mod window;

use std::sync::atomic::{AtomicU64, Ordering};

use archiver::{ArchiverClient, Operator, ValueKind};
use egui::{Color32, Pos2, Rect, Sense, Vec2};
use pv::PvManager;

use crate::axis;
use crate::history::SearchHistory;
use crate::view::{self, TraceView, View, YAxis};
use axes::{Members, PaneAxes, Place, Stats};
use fetch::Fetcher;
use pane::Pane;
pub use paste::{MAX_PASTE, MiddleClick, Pasted, pv_names};
pub use series::now;
use series::{History, Pt, Trace};
pub use status::{ArchiverState, ArchiverStatus};
use window::Window;

pub use timeaxis::parse_span;

/// Wait this long after the last pan or zoom before fetching (seconds).
const DEBOUNCE: f64 = 0.3;

/// While following now, refresh the archived data this often (seconds);
/// binned history less often, at its bin pace (`fetch::refresh_after`).
const LIVE_REFRESH: f64 = 30.0;

/// After an error, ask again no sooner than this (seconds), doubling with
/// each failure in a row.
const RETRY: f64 = 10.0;

/// The Live pill pulses this long after following now starts (seconds).
const PULSE_SECS: f64 = 3.0;

/// A trace with live data waits this long (seconds) for its archived
/// history before it settles on an axis by the live data alone.
const SETTLE_WAIT: f64 = 5.0;

/// A trace with live data only settles after this long (seconds), so its
/// first sample alone (often a 0) does not decide its axis.
const LIVE_WAIT: f64 = 1.0;

/// The most archivers the pill's switcher lists.
const MAX_RECENT: usize = 8;

/// The smallest plot (top bar and chips aside), so one in an auto-sized
/// window neither collapses nor grows without end.
const MIN_SIZE: Vec2 = Vec2::new(360.0, 200.0);

/// The gap between panes (points), which holds the divider.
const PANE_GAP: f32 = 8.0;

/// Dragging a divider leaves no pane shorter than this (points).
const MIN_PANE: f32 = 40.0;

/// No pane's weight (its share of the height) is less than this.
const MIN_WEIGHT: f32 = 0.05;

/// How long the overflow toast shows (seconds).
const TOAST_SECS: f64 = 8.0;

/// Tells plots apart, for their egui ids, when the host gives no salt
/// ([`Plot::with_id_salt`]).
static NEXT_PLOT: AtomicU64 = AtomicU64::new(0);

/// An archived + live plot of a set of PVs on one shared time axis.
///
/// Owns its traces, background fetches and live subscriptions; dropping it
/// unsubscribes and abandons in-flight fetches. Its only shared state is
/// the counter [`Plot::new`] salts ids with, so any number may exist at
/// once.
///
/// Array (waveform) PVs are not plotted: their chip shows the latest array
/// (`[936] 0, 0, 0, …`) and says "not plotted", and their history is not
/// fetched. (Earlier builds plotted element 0 over time; a host such as
/// bobview that wants a waveform's elements should show them itself.)
pub struct Plot {
    pvs: PvManager,
    archiver: Option<ArchiverClient>,
    /// Salt for every egui id this plot uses.
    id: egui::Id,
    traces: Vec<Trace>,
    next_trace: u64,
    window: Window,
    /// The span a reset (double-click, Home) returns to.
    home_span: f64,
    /// The range drawn, easing toward `window`'s.
    shown: Option<(f64, f64)>,
    /// The panes, top to bottom, each with its value axes: never empty.
    panes: Vec<Pane>,
    next_pane: u32,
    /// Value axis ids, unique across the panes.
    next_axis: u32,
    /// New axes start on a log scale: the view (from before there were
    /// several axes) said so for its one.
    default_log: bool,
    /// The axis whose name is being edited, and the text so far.
    renaming: Option<axisui::Renaming>,
    /// A legend chip being dragged (its trace), to drop on an axis.
    chip_drag: Option<u64>,
    /// Traces to move (a chip dropped, a menu's choice) or, with `None`,
    /// remove (its ✕) this frame: done at the end of the frame, so the
    /// panes stay as laid out until they are drawn.
    moves: Vec<(u64, Option<MoveTo>)>,
    /// The last trace that settled in a new pane for want of room, for
    /// the toast offering to keep it where it was.
    overflow: Option<Overflow>,
    /// Where the axes (strip, name) and chips (whole, dot) were last
    /// drawn, for tests to point at.
    #[cfg(test)]
    seen: Seen,
    fetcher: Fetcher,
    /// When (seconds since 1970) the user last panned or zoomed, for the
    /// fetch debounce.
    changed_at: f64,
    /// A fetch is waiting for the debounce.
    pending: bool,
    /// Where finished requests ask for a repaint: set every frame.
    repaint: Option<fetch::Repaint>,
    /// When (egui time) the plot last started following now, for the
    /// Live pill's pulse.
    live_since: Option<f64>,
    /// The Live pill's dot pulses this frame.
    pulse: bool,
    /// egui's time at the last frame, for animation steps.
    last_frame: Option<f64>,
    /// Draw the next frame without easing (direct manipulation).
    snap: bool,
    /// Kinetic panning after a drag, in seconds of time per second.
    glide: f64,
    drag: Option<Drag>,
    /// The trace whose chip is hovered: drawn bold, the others dimmed.
    highlight: Option<u64>,
    /// The chips as last laid out: (trace, rect, value shown).
    #[cfg(test)]
    chips_shown: Vec<(u64, Rect, String)>,
    /// Keyboard shortcuts apply: the plot was clicked last.
    active: bool,
    search: bar::SearchState,
    /// The search box's recent searches, and whether the user changed
    /// them since [`Plot::search_history_changed`] last said so.
    history: SearchHistory,
    history_changed: bool,
    custom: bar::CustomState,
    /// Whether the archiver can be reached, for the archiver pill.
    monitor: status::Monitor,
    /// Recently used archivers (retrieval bases), newest first, for the
    /// pill's switcher.
    recent: Vec<String>,
    /// The user picked another archiver in the pill since the host last
    /// asked ([`Plot::take_archiver_change`]).
    archiver_changed: bool,
    /// The pill can switch archivers, and a view's archiver is followed
    /// ([`Plot::set_archiver_switching`]).
    switch_archivers: bool,
    edit: bar::ArchiverEdit,
    /// Middle-clicks and pastes, for the host's selection.
    paste: paste::PasteState,
    /// The undo and redo lists.
    undo: undo::Undo,
    /// The host's name for the view shown ([`Plot::set_view_name`]).
    view_name: Option<String>,
}

/// Where the axes (strip, name), chips (whole, dot) and panes were last
/// drawn.
#[cfg(test)]
type Seen = (Vec<(Rect, Rect)>, Vec<(Rect, Rect)>, Vec<Rect>);

/// Where a trace is moved by hand.
#[derive(Clone, Copy, Debug, PartialEq)]
enum MoveTo {
    /// The value axis with this id (in whichever pane).
    Axis(u32),
    /// An axis of its own in the pane with this id.
    NewAxis(u32),
    /// The pane with this id: the axis there that suits it, else one of
    /// its own.
    Pane(u32),
    /// A pane of its own, below the others.
    NewPane,
}

/// A trace that settled in a new pane, the pane it would have gone in
/// being full.
#[derive(Clone, Debug)]
struct Overflow {
    trace: u64,
    /// The pane it would have gone in (its id), and its axes of values.
    from: u32,
    axes: usize,
    /// When (egui time) the toast goes, once shown.
    until: Option<f64>,
}

/// A value axis and the traces on it (those whose `axis` is its id).
#[derive(Clone, Debug)]
struct ValueAxis {
    id: u32,
    /// The name the user gave it; else it names itself.
    name: Option<String>,
    autoscale: bool,
    /// The fixed range when not autoscaling.
    lo: f64,
    hi: f64,
    log: bool,
    /// The range drawn, easing toward the target.
    shown: Option<(f64, f64)>,
    /// It has had a trace, so it goes when its last one leaves. An axis
    /// from a view that no trace is on yet waits for a trace to settle:
    /// the one in `reserved`, else any.
    used: bool,
    reserved: Option<u64>,
}

impl ValueAxis {
    fn new(id: u32) -> Self {
        Self::from_view(id, &YAxis::default())
    }

    fn from_view(id: u32, y: &YAxis) -> Self {
        Self {
            id,
            name: y.name.clone(),
            autoscale: y.autoscale || y.min.is_none() || y.max.is_none(),
            lo: y.min.unwrap_or(0.0),
            hi: y.max.unwrap_or(1.0),
            log: y.log,
            shown: None,
            used: false,
            reserved: None,
        }
    }

    fn view(&self) -> YAxis {
        YAxis {
            autoscale: self.autoscale,
            min: (!self.autoscale).then_some(self.lo),
            max: (!self.autoscale).then_some(self.hi),
            log: self.log,
            name: self.name.clone(),
        }
    }

    /// On a log scale a fixed range must be above zero: back to
    /// autoscaling if not.
    fn fit_log_range(&mut self) {
        if self.log && !self.autoscale && self.lo <= 0.0 {
            self.autoscale = true;
            self.shown = None;
        }
    }

    /// Zoom by `factor` about `at` (the middle when `None`), from the
    /// range `shown` (else its own), leaving autoscale.
    fn zoom(&mut self, factor: f64, at: Option<f64>, shown: (f64, f64)) {
        let (lo, hi) = self.shown.unwrap_or(shown);
        let mid = if self.log && lo > 0.0 {
            (lo * hi).sqrt()
        } else {
            (lo + hi) / 2.0
        };
        let (l, h) = window::zoom_values(lo, hi, factor, at.unwrap_or(mid), self.log);
        if l.is_finite() && h.is_finite() && l < h {
            self.autoscale = false;
            (self.lo, self.hi) = (l, h);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    /// Moving through time.
    Pan,
    /// Zooming values on the value axis with this id.
    Axis(u32),
    /// Drawing a box to zoom to, from here.
    Box(Pos2),
}

impl Plot {
    /// A plot subscribing through `pvs` (the caller's manager: a `tagged`
    /// handle from bobview repaints only the window holding this plot) and
    /// fetching history from `archiver`, or live-only with `None`. Archive
    /// fetches run as blocking tasks on `pvs.runtime()`.
    pub fn new(pvs: &PvManager, archiver: Option<ArchiverClient>) -> Self {
        let n = NEXT_PLOT.fetch_add(1, Ordering::Relaxed);
        Self::with_id(pvs, archiver, egui::Id::new(("archiveview-plot", n)))
    }

    /// As [`Plot::new`], with the egui ids salted by `salt` rather than a
    /// running count: a host that rebuilds its plots (or restores them
    /// across runs) keeps their popups and text fields stable this way.
    /// Plots shown at the same time need different salts.
    pub fn with_id_salt(
        pvs: &PvManager,
        archiver: Option<ArchiverClient>,
        salt: impl std::hash::Hash + std::fmt::Debug,
    ) -> Self {
        let id = egui::Id::new("archiveview-plot").with(salt);
        Self::with_id(pvs, archiver, id)
    }

    fn with_id(pvs: &PvManager, archiver: Option<ArchiverClient>, id: egui::Id) -> Self {
        let mut monitor = status::Monitor::default();
        monitor.reset(archiver.is_some());
        let recent = archiver.iter().map(|a| a.base().to_string()).collect();
        let mut plot = Self {
            pvs: pvs.clone(),
            archiver,
            id,
            traces: Vec::new(),
            next_trace: 0,
            window: Window::live(3600.0),
            home_span: 3600.0,
            shown: None,
            panes: vec![Pane::new(0)],
            next_pane: 1,
            next_axis: 0,
            default_log: false,
            renaming: None,
            chip_drag: None,
            moves: Vec::new(),
            overflow: None,
            #[cfg(test)]
            seen: Default::default(),
            fetcher: Fetcher::default(),
            changed_at: f64::NEG_INFINITY,
            pending: false,
            repaint: None,
            live_since: None,
            pulse: false,
            last_frame: None,
            snap: true,
            glide: 0.0,
            drag: None,
            highlight: None,
            #[cfg(test)]
            chips_shown: Vec::new(),
            active: false,
            search: bar::SearchState::default(),
            history: SearchHistory::default(),
            history_changed: false,
            custom: bar::CustomState::default(),
            monitor,
            recent,
            archiver_changed: false,
            switch_archivers: false,
            edit: bar::ArchiverEdit::default(),
            paste: paste::PasteState::default(),
            undo: undo::Undo::default(),
            view_name: None,
        };
        // What the host does before the first frame (PVs, a view) is the
        // first step.
        plot.commit(false);
        plot
    }

    /// Change (or remove) the archiver: refetches every trace's history,
    /// and checks that the new one can be reached.
    pub fn set_archiver(&mut self, archiver: Option<ArchiverClient>) {
        if let Some(a) = &archiver {
            remember(&mut self.recent, a.base());
        }
        self.archiver = archiver;
        self.monitor.reset(self.archiver.is_some());
        // Searches in flight were for the old archiver: drop their answers
        // and search again.
        self.search.restart();
        self.forget_parked_histories();
        for t in &mut self.traces {
            t.history = None;
            // The generation carries on, so a late answer from the old
            // archiver is still recognised as stale.
            t.fetch = series::FetchState {
                generation: t.fetch.generation + 1,
                ..Default::default()
            };
        }
    }

    pub fn archiver(&self) -> Option<&ArchiverClient> {
        self.archiver.as_ref()
    }

    /// Whether the archiver can be reached, as the archiver pill shows it.
    pub fn archiver_status(&self) -> &ArchiverStatus {
        &self.monitor.status
    }

    /// True (once) after the user switched archivers in the archiver
    /// pill: the plot already uses the new one ([`Plot::archiver`]), and
    /// the host may remember it (the standalone app writes it to its
    /// settings).
    pub fn take_archiver_change(&mut self) -> bool {
        std::mem::take(&mut self.archiver_changed)
    }

    /// Let the user switch archivers in the archiver pill, and have
    /// [`Plot::set_view`] switch to a view's archiver. Off by default: an
    /// embedding host that manages the archiver itself keeps the pill as
    /// a status display only, and its stored views don't move the plot to
    /// another archiver. The standalone app turns it on.
    pub fn set_archiver_switching(&mut self, on: bool) {
        self.switch_archivers = on;
        if !on {
            self.edit = bar::ArchiverEdit::default();
        }
    }

    /// The archivers the pill offers to switch to (retrieval URLs, newest
    /// first), e.g. as remembered from an earlier run. Switching adds to
    /// the front.
    pub fn set_recent_archivers(&mut self, urls: &[String]) {
        let current = self.archiver.as_ref().map(|a| a.base().to_string());
        self.recent.clear();
        for url in urls.iter().rev().chain(current.as_ref()) {
            remember(&mut self.recent, url);
        }
    }

    /// The archivers the pill offers, newest first.
    pub fn recent_archivers(&self) -> &[String] {
        &self.recent
    }

    /// Add traces (skipping names already plotted, compared by
    /// `pv::PvName::key`): subscribe live and fetch the visible history.
    /// One undo step ("add X"), however they were picked.
    pub fn add_pvs(&mut self, names: &[String]) {
        // What came before is a step of its own.
        self.commit(false);
        for name in names {
            self.add(name);
        }
    }

    fn add(&mut self, name: &str) -> Option<&mut Trace> {
        let name = name.trim();
        let key = pv::PvName::parse(name).key();
        if name.is_empty() || self.traces.iter().any(|t| t.pv.name().key() == key) {
            return None;
        }
        // The first palette colour no other trace has.
        let slot = (0..)
            .find(|s| !self.traces.iter().any(|t| t.slot == *s))
            .unwrap_or(0);
        let mut trace = Trace::new(self.next_trace, name, self.pvs.subscribe(name));
        trace.slot = slot;
        self.next_trace += 1;
        self.traces.push(trace);
        self.traces.last_mut()
    }

    /// Add the PVs named in pasted `text` (the primary selection after a
    /// middle-click, or the clipboard): see [`pv_names`]. More than
    /// [`MAX_PASTE`] adds none (a screenful selected by accident). Says what it did,
    /// as [`Plot::take_pasted`] does after.
    pub fn paste_pvs(&mut self, text: &str) -> Pasted {
        let mut pasted = Pasted::default();
        let names = pv_names(text);
        if names.len() > MAX_PASTE {
            pasted.too_many = names.len();
            return self.pasted(pasted);
        }
        let plotted: Vec<String> = self.traces.iter().map(|t| t.pv.name().key()).collect();
        let mut adding = Vec::new();
        for name in names {
            let key = pv::PvName::parse(&name).key();
            if plotted.contains(&key) {
                pasted.present.push(name);
            } else if !adding.contains(&key) {
                // Once, however it is spelt (`A:B`, `ca://A:B`).
                adding.push(key);
                pasted.added.push(name);
            }
        }
        // As the search box adds: one call, one step to undo.
        self.add_pvs(&pasted.added);
        if pasted.added.is_empty() && pasted.present.is_empty() {
            pasted.text = text.to_string();
        }
        self.pasted(pasted)
    }

    /// Keep what a paste did for [`Plot::take_pasted`], and draw again
    /// (a host answering a middle-click does so between frames).
    fn pasted(&mut self, pasted: Pasted) -> Pasted {
        self.paste.pasted = Some(pasted.clone());
        if let Some(r) = &self.repaint {
            r.ctx.request_repaint_of(r.viewport);
        }
        pasted
    }

    /// Paste `text` into the search box at its cursor (where the
    /// middle-click put it), as a Linux text field does.
    pub fn paste_into_search(&mut self, text: &str) {
        bar::paste(self, text);
    }

    /// Where the middle button was clicked since the host last asked: the
    /// host answers with the primary selection's text ([`Plot::paste_pvs`],
    /// [`Plot::paste_into_search`]). The widget reads no clipboard. A
    /// click stays here until taken (a later one replaces it), so a host
    /// that never asks leaves one waiting, which is harmless.
    pub fn take_middle_click(&mut self) -> Option<MiddleClick> {
        self.paste.middle.take()
    }

    /// What the last paste did, once: Ctrl+V over the plot (egui's
    /// `Event::Paste`) or [`Plot::paste_pvs`]. For the host's toast.
    pub fn take_pasted(&mut self) -> Option<Pasted> {
        self.paste.pasted.take()
    }

    /// Text newly selected in the search box, once, for the host to make
    /// the primary selection (egui does not).
    pub fn take_search_selection(&mut self) -> Option<String> {
        self.paste.selection.take()
    }

    /// Remove a trace by the name it was added with.
    pub fn remove_pv(&mut self, name: &str) {
        let key = pv::PvName::parse(name).key();
        let gone = self
            .traces
            .iter()
            .find(|t| t.name == name || t.pv.name().key() == key)
            .map(|t| t.id);
        if let Some(id) = gone {
            self.remove_trace(id);
        }
    }

    /// Remove the trace with id `id`, keeping it for an undo to bring
    /// back.
    fn remove_trace(&mut self, id: u64) {
        if let Some(i) = self.traces.iter().position(|t| t.id == id) {
            let t = self.traces.remove(i);
            self.park(t);
            self.prune_axes();
        }
    }

    /// The plotted PV names, in legend order.
    pub fn names(&self) -> Vec<String> {
        self.traces.iter().map(|t| t.name.clone()).collect()
    }

    /// A window title: the PV names, shortened (as bobview's
    /// `Trend::title`); empty with none.
    pub fn title(&self) -> String {
        let names: Vec<String> = self.traces.iter().map(Trace::label).collect();
        match names.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [a, b] => format!("{a}, {b}"),
            [first, rest @ ..] => format!("{first} and {} more", rest.len()),
        }
    }

    /// The current view (traces, range, panes, value axes, archiver), to
    /// save.
    pub fn view(&self) -> View {
        let range = if self.window.live {
            view::Range::Relative {
                secs: self.window.span,
            }
        } else {
            let (from, to) = self.window.range(now());
            view::Range::Absolute {
                start: view::format_time(from),
                end: view::format_time(to),
            }
        };
        // Only axes with traces, or kept for a PV yet to settle (pruned
        // once that PV is gone): no stale fixed range comes back for
        // whichever PV would take it.
        let mut kept: Vec<&ValueAxis> = Vec::new();
        let mut panes = Vec::new();
        for p in &self.panes {
            let before = kept.len();
            kept.extend((p.axes.iter()).filter(|a| self.has_traces(a) || a.reserved.is_some()));
            panes.push(view::PaneView {
                axes: kept.len() - before,
                height: p.weight,
            });
        }
        // One pane: as before there were panes.
        if panes.len() < 2 {
            panes.clear();
        }
        let mut axes = kept.iter().map(|a| a.view());
        View {
            version: view::VERSION,
            pvs: self
                .traces
                .iter()
                .map(|t| TraceView {
                    name: t.name.clone(),
                    color: t.color.clone(),
                    visible: t.visible,
                    axis: t.axis.and_then(|id| kept.iter().position(|a| a.id == id)),
                })
                .collect(),
            range,
            y: axes.next().unwrap_or_default(),
            axes: axes.collect(),
            panes,
            archiver: self.archiver.as_ref().map(|a| a.base().to_string()),
        }
    }

    /// Show `view`, replacing the traces, range, panes and value axes.
    /// Traces the view puts on an axis stay there (in its pane); others
    /// join one when their data arrives. With archiver switching on
    /// ([`Plot::set_archiver_switching`]), a view that names an archiver
    /// switches to it; otherwise, or for a view without one, the archiver
    /// stays as it is.
    pub fn set_view(&mut self, view: &View) {
        self.name_step(if view.pvs.is_empty() {
            "clear the plot"
        } else {
            "open view"
        });
        if let Some(url) = view.archiver.as_ref().filter(|_| self.switch_archivers) {
            match ArchiverClient::new(url) {
                Ok(a) if self.archiver.as_ref() != Some(&a) => self.set_archiver(Some(a)),
                Ok(_) => {}
                Err(e) => log::warn!("view archiver {url}: {e}"),
            }
        }
        for t in std::mem::take(&mut self.traces) {
            self.park(t);
        }
        let mut axes = view
            .value_axes()
            .map(|y| {
                let mut a = ValueAxis::from_view(self.next_axis, y);
                self.next_axis += 1;
                a.fit_log_range();
                a
            })
            .collect::<Vec<ValueAxis>>()
            .into_iter();
        // Each pane takes its count of the axes in order, the last any
        // left over; no panes (one, or an older file): one with them all.
        let one = [view::PaneView::default()];
        let shares = if view.panes.is_empty() {
            &one[..]
        } else {
            &view.panes[..]
        };
        self.panes = (shares.iter().enumerate())
            .map(|(k, share)| {
                let mine: Vec<ValueAxis> = if k + 1 == shares.len() {
                    axes.by_ref().collect()
                } else {
                    axes.by_ref().take(share.axes).collect()
                };
                let weight = if share.height.is_finite() && share.height > 0.0 {
                    share.height.max(MIN_WEIGHT)
                } else {
                    1.0
                };
                let pane = Pane::with_axes(self.next_pane, weight, mine);
                self.next_pane += 1;
                pane
            })
            .collect();
        let ids: Vec<u32> = self.all_axes().map(|a| a.id).collect();
        self.renaming = None;
        self.overflow = None;
        // A file from before there were several axes: its one axis is for
        // its first visible PV, and its log scale for every axis.
        let old = view.axes.is_empty() && view.pvs.iter().all(|t| t.axis.is_none());
        self.default_log = old && view.y.log;
        for tv in &view.pvs {
            let axis = tv.axis.and_then(|i| ids.get(i).copied());
            let pane = axis
                .and_then(|id| self.axis_pane(id))
                .map(|k| self.panes[k].id);
            if let Some(a) = axis.and_then(|id| self.axis_mut(id)) {
                a.used = true;
            }
            if let Some(t) = self.add(&tv.name) {
                t.color = tv.color.clone();
                t.visible = tv.visible;
                t.axis = axis;
                t.pane = pane;
            }
        }
        if old
            && let Some(first) = self.traces.iter().find(|t| t.visible)
            && let Some(y) = self.panes[0].axes.first_mut()
        {
            y.reserved = Some(first.id);
        }
        self.set_range(&view.range);
        self.snap = true;
    }

    /// Show `range`.
    pub fn set_range(&mut self, range: &view::Range) {
        self.window = match range {
            view::Range::Relative { secs } => Window::live(*secs),
            view::Range::Absolute { start, end } => {
                match (view::parse_time(start), view::parse_time(end)) {
                    (Some(a), Some(b)) => Window::fixed(a, b),
                    _ => {
                        log::warn!("bad view range {start}..{end}: showing the last hour");
                        Window::live(3600.0)
                    }
                }
            }
        };
        self.home_span = self.window.span;
        self.snap = true;
    }

    /// Offer `history` as the search box's recent searches (a plot starts
    /// with none). The plot keeps them in memory only: the standalone app
    /// loads and saves them as `history.json`.
    pub fn set_search_history(&mut self, history: SearchHistory) {
        self.history = history;
        self.history_changed = false;
    }

    /// The recent searches, as the user has added to and pruned them.
    pub fn search_history(&self) -> &SearchHistory {
        &self.history
    }

    /// True once after each change the user makes to the recent searches
    /// (a search used, one forgotten, all cleared): when a host that keeps
    /// them should save them.
    pub fn search_history_changed(&mut self) -> bool {
        std::mem::take(&mut self.history_changed)
    }

    /// True while any archive fetch or search is in flight, or one is due,
    /// or a trace with data waits to settle on an axis (snapshots wait for
    /// this).
    pub fn is_loading(&self) -> bool {
        self.fetcher.pending()
            || self
                .traces
                .iter()
                .any(|t| t.visible && !t.is_array() && t.axis.is_none() && t.data_since.is_some())
            || self.pending
            || (self.archiver.is_some() && self.monitor.due)
            || bar::search_due(self)
            || (self.archiver.is_some()
                && self.traces.iter().any(|t| {
                    t.archive_name().is_some()
                        && !t.fetch.not_archived
                        && t.fetch.requested.is_none()
                }))
    }

    /// Draw the whole widget (top bar, plot, legend chips) into the space
    /// `ui` offers, handle its input, and schedule its own repaints.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.show(ui, None);
    }

    /// As [`Plot::ui`], with `menu` adding the host's items (views,
    /// settings) to the top bar's `≡` menu.
    pub fn ui_with_menu(&mut self, ui: &mut egui::Ui, menu: &mut dyn FnMut(&mut egui::Ui)) {
        self.show(ui, Some(menu));
    }

    fn show(&mut self, ui: &mut egui::Ui, menu: Option<&mut dyn FnMut(&mut egui::Ui)>) {
        let ctx = ui.ctx().clone();
        self.repaint = Some(fetch::Repaint {
            ctx: ctx.clone(),
            viewport: ctx.viewport_id(),
        });
        let now = now();
        let time = ui.input(|i| i.time);
        let dt = self.last_frame.map_or(0.0, |t| (time - t).clamp(0.0, 0.1));
        self.last_frame = Some(time);
        self.take_results(now);
        self.pulse = self.pulsing(ui, time);
        // What the host changed since the last frame is a step of its own.
        // A held key (arrows repeating) is a gesture, as a held button is.
        let (pressed, down) = ui.input(|i| {
            let key = i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key {
                        pressed: true,
                        repeat: false,
                        ..
                    }
                )
            });
            (
                i.pointer.any_pressed() || key,
                i.pointer.any_down() || !i.keys_down.is_empty(),
            )
        });
        self.commit(false);
        if pressed {
            self.undo.press += 1;
        }
        let whole = ui.available_rect_before_wrap();
        self.keyboard(ui, whole, now);

        bar::top_bar(self, ui, menu, now);
        ui.add_space(6.0);

        // Kinetic panning after a drag.
        if self.glide != 0.0 && self.drag.is_none() {
            self.window.pan(self.glide * dt, now);
            self.glide *= (-5.0 * dt).exp();
            self.changed_at = now;
            self.snap = true;
            if self.glide.abs() < self.window.span * 0.01 || self.window.live {
                self.glide = 0.0;
            }
        }

        // The data, and the axes and panes it settles on, before anything
        // is laid out: a trace overflowing into a new pane makes room
        // for it this frame.
        let (from, to) = self.shown_range(now, dt);
        let dark = ui.visuals().dark_mode;
        let data: Vec<Vec<Pt>> = self
            .traces
            .iter()
            .map(|t| {
                if t.visible {
                    t.points(from, to, now)
                } else {
                    Vec::new()
                }
            })
            .collect();
        self.settle_axes(&data, now);

        // The legends' heights first, so the panes fill the rest; never
        // smaller than MIN_SIZE (an auto-sized window offers little).
        // Until the frame ends, the panes stay as they are: moves and
        // removals wait for it.
        let width = ui.available_width().max(MIN_SIZE.x);
        let mut chips: Vec<legend::Chips> = (0..self.panes.len())
            .map(|k| legend::layout(self, ui, width, k))
            .collect();
        let font = egui::FontId::proportional(11.5);
        let title_font = egui::FontId::proportional(13.5);
        let strip = render::time_strip(ui.painter(), &font);
        let legends: Vec<f32> = chips.iter().map(|c| c.height).collect();
        let around: f32 = legends.iter().map(|h| legend_room(*h)).sum();
        let gaps = PANE_GAP * (self.panes.len() - 1) as f32;
        let plot_h = (ui.available_height() - around).max(MIN_SIZE.y + gaps);
        let (area, _) = ui.allocate_exact_size(Vec2::new(width, plot_h + around), Sense::hover());
        let layout = self.pane_rects(area, strip, &legends);
        let rects: Vec<Rect> = layout.iter().map(|l| l.0).collect();
        for (k, (c, l)) in chips.iter_mut().zip(&layout).enumerate() {
            legend::interact(self, ui, c, l.1, k == 0);
        }
        // A middle-click on the plot or the legend (not on a popup over
        // them) asks the host for the selection.
        let middle = ui.input(|i| {
            let clicked = i.pointer.button_clicked(egui::PointerButton::Middle);
            clicked.then(|| i.pointer.interact_pos()).flatten()
        });
        if let Some(p) = middle
            && area.contains(p)
            && ui.ctx().layer_id_at(p) == Some(ui.layer_id())
        {
            self.paste.middle = Some(MiddleClick::Plot);
        }
        #[cfg(test)]
        {
            self.chips_shown = chips.iter().flat_map(legend::Chips::shown).collect();
        }

        // Each pane takes the pointer in its own rect, and the dividers
        // between them in theirs.
        let resps: Vec<egui::Response> = (self.panes.iter().zip(&rects))
            .map(|(pane, r)| {
                ui.interact(*r, self.id.with(("pane", pane.id)), Sense::click_and_drag())
            })
            .collect();
        self.dividers(ui, &layout, strip);
        let specs: Vec<Vec<render::AxisSpec>> = (0..self.panes.len())
            .map(|k| self.value_axes(k, &data, from, to, dt, dark))
            .collect();
        let mut frames =
            render::frames(ui.painter(), &rects, (from, to), &specs, &font, &title_font);

        // The keyboard follows the plot clicked last.
        let pressed_away =
            ui.input(|i| i.pointer.any_pressed()) && !resps.iter().any(|r| r.contains_pointer());
        if resps.iter().any(|r| r.clicked() || r.drag_started()) {
            self.active = true;
        } else if pressed_away {
            self.active = false;
        }
        for (k, resp) in resps.iter().enumerate() {
            self.interact(k, ui, resp, &frames[k], now);
        }
        for (k, f) in frames.iter_mut().enumerate() {
            self.axis_interact(k, ui, f);
        }
        #[cfg(test)]
        {
            self.seen.0 = (frames.iter())
                .flat_map(|f| {
                    (0..f.axis_count()).filter_map(|k| f.axis_rect(k).zip(f.title_rect(k)))
                })
                .collect();
            self.seen.2 = rects.clone();
        }
        let ppp = ui.ctx().pixels_per_point();
        self.schedule_fetches(now, frames[0].data.width() * ppp);
        self.schedule_probe(now);

        // The crosshair, first so its time tag can hide the label under
        // it: where the pointer is, in whichever pane, and across them all.
        let hair = (!self.traces.is_empty()
            && !matches!(self.drag, Some(Drag::Pan | Drag::Axis(_))))
        .then(|| {
            (0..frames.len())
                .find_map(|k| hover::hairline(self, ui, k, &frames[k], &data).map(|h| (k, h)))
        })
        .flatten();
        let last = frames.len() - 1;
        for (k, f) in frames.iter().enumerate() {
            let painter = ui.painter_at(f.outer);
            if self.traces.is_empty() {
                painter.rect_filled(f.outer, 10.0, ui.visuals().extreme_bg_color);
                hover::empty_state(&painter, f, ui.visuals());
                continue;
            }
            let lines = self.lines(k, &data, dark);
            let cover = hair
                .filter(|_| k == last)
                .map(|(_, (p, t))| hover::time_tag_rect(&painter, f, p.x, t));
            render::paint(&painter, f, &lines, ui.visuals(), cover);
            render::now_marker(&painter, f, now, ui.visuals(), &font, k == 0);
        }
        if let Some(Drag::Box(origin)) = self.drag
            && let Some(p) = ui.input(|i| i.pointer.latest_pos())
            && let Some(f) = frames.iter().find(|f| f.outer.contains(origin))
        {
            let r = Rect::from_two_pos(origin, p).intersect(f.data);
            let accent = ui.visuals().selection.bg_fill;
            ui.painter_at(f.outer).rect(
                r,
                2.0,
                accent.gamma_multiply(0.15),
                egui::Stroke::new(1.0, accent),
                egui::StrokeKind::Inside,
            );
        }
        if let Some((at, hair)) = hair {
            for (k, f) in frames.iter().enumerate() {
                let place = hover::Place {
                    pane: k,
                    pointer: k == at,
                    time_axis: k == last,
                };
                hover::crosshair(self, ui, f, &data, dark, hair, place);
            }
        }
        hover::status(self, ui, &frames[0]);
        for c in &chips {
            legend::paint(self, ui, c, dark);
        }
        self.rename_box(ui, &frames);
        self.chip_ghost(ui, dark);
        self.overflow_toast(ui, area, &rects);
        // What was asked for this frame (a chip dropped or removed, a
        // menu's move), now the panes are drawn.
        for (trace, to) in std::mem::take(&mut self.moves) {
            match to {
                Some(to) => self.move_trace(trace, to),
                None => {
                    self.remove_trace(trace);
                    self.highlight = None;
                }
            }
        }
        self.commit(down);

        // Keep moving: easing, gliding, and time passing while live.
        let target = self.window.range(now);
        let settled = self.shown.is_some_and(|(a, b)| {
            let tol = self.window.span * 1e-3;
            (a - target.0).abs() <= tol && (b - target.1).abs() <= tol
        });
        if self.glide != 0.0 || !settled || self.pending {
            ctx.request_repaint();
        } else if self.window.live {
            let per_pixel = self.window.span / frames[0].data.width().max(1.0) as f64;
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                per_pixel.clamp(1.0 / 15.0, 1.0),
            ));
            if self.pulse {
                ctx.request_repaint_after(std::time::Duration::from_millis(66));
            }
        }
    }

    /// Where each pane goes in `area`, top to bottom, with its legend
    /// (`legends`: their heights) under it: its rect, and where its
    /// legend starts. The panes share the height by their weights; the
    /// bottom one also holds the time labels (`strip` high), so their
    /// data areas still do.
    fn pane_rects(&self, area: Rect, strip: f32, legends: &[f32]) -> Vec<(Rect, Pos2)> {
        let n = self.panes.len();
        let around: f32 = legends.iter().map(|h| legend_room(*h)).sum();
        let room = (area.height() - around - strip - PANE_GAP * (n - 1) as f32).max(0.0);
        let total: f32 = self.panes.iter().map(|p| p.weight.max(MIN_WEIGHT)).sum();
        let mut y = area.top();
        (self.panes.iter().zip(legends).enumerate())
            .map(|(k, (p, legend))| {
                let mut h = room * p.weight.max(MIN_WEIGHT) / total;
                if k + 1 == n {
                    h = (area.bottom() - legend_room(*legend) - y).max(0.0);
                }
                let r = Rect::from_x_y_ranges(area.x_range(), y..=y + h);
                y += h + legend_room(*legend);
                let at = Pos2::new(area.left(), y - legend);
                y += PANE_GAP;
                (r, at)
            })
            .collect()
    }

    /// The dividers between the panes (`layout` from
    /// [`Plot::pane_rects`]): dragging one moves height from one pane to
    /// the next, and double-clicking makes them all equal.
    fn dividers(&mut self, ui: &egui::Ui, layout: &[(Rect, Pos2)], strip: f32) {
        let heights: Vec<f32> = (layout.iter().enumerate())
            .map(|(k, l)| l.0.height() - if k + 1 == layout.len() { strip } else { 0.0 })
            .collect();
        let total: f32 = self.panes.iter().map(|p| p.weight.max(MIN_WEIGHT)).sum();
        let room: f32 = heights.iter().sum();
        for k in 0..layout.len().saturating_sub(1) {
            let below = layout[k + 1].0.top();
            let r = Rect::from_x_y_ranges(layout[k].0.x_range(), below - PANE_GAP..=below);
            let resp = ui
                .interact(r, self.id.with(("divider", k)), Sense::click_and_drag())
                .on_hover_cursor(egui::CursorIcon::ResizeVertical)
                .on_hover_text("Drag to resize the panes · double-click: make them equal");
            if resp.double_clicked() {
                self.name_step("make the panes equal");
                for p in &mut self.panes {
                    p.weight = 1.0;
                }
            } else if resp.dragged() && room > 0.0 {
                // Points to weight; neither pane shorter than MIN_PANE.
                let per = total / room;
                let (a, b) = (self.panes[k].weight, self.panes[k + 1].weight);
                let min = MIN_PANE * per;
                if a + b > 2.0 * min {
                    let dw = (resp.drag_delta().y * per).clamp(min - a, b - min);
                    self.panes[k].weight = a + dw;
                    self.panes[k + 1].weight = b - dw;
                }
            }
            let v = ui.visuals();
            let hot = resp.hovered() || resp.dragged();
            let color = if hot {
                v.strong_text_color().gamma_multiply(0.6)
            } else {
                v.weak_text_color().gamma_multiply(0.35)
            };
            let grip = Rect::from_center_size(r.center(), Vec2::new(36.0, 3.0));
            ui.painter().rect_filled(grip, 1.5, color);
        }
    }

    /// The toast after a trace overflowed into a new pane, over the
    /// bottom of the plot (`area`, the panes in `rects`) for a few
    /// seconds: "X → new pane (4 axes here)", and Keep here, which puts
    /// it back in the pane it would have gone in, on an axis of its own.
    /// Clicking the text dismisses it.
    fn overflow_toast(&mut self, ui: &egui::Ui, area: Rect, rects: &[Rect]) {
        let Some(o) = &mut self.overflow else {
            return;
        };
        let time = ui.input(|i| i.time);
        let until = *o.until.get_or_insert(time + TOAST_SECS);
        let o = o.clone();
        // Gone, moved since, or with nowhere to go back to.
        let trace = (self.traces.iter())
            .find(|t| t.id == o.trace)
            .filter(|t| t.pane != Some(o.from) && self.pane_index(o.from).is_some());
        let Some(trace) = trace.filter(|_| time <= until) else {
            self.overflow = None;
            return;
        };
        let text = format!("{} → new pane ({} axes here)", trace.label(), o.axes);
        let bottom = rects.last().map_or(area.bottom(), |r| r.bottom());
        let pos = Pos2::new(area.center().x, bottom - 34.0);
        let (mut keep, mut dismiss) = (false, false);
        egui::Area::new(self.id.with("overflow"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(16.0)
                    .inner_margin(egui::Margin::symmetric(14, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let label = egui::Label::new(text).sense(Sense::click());
                            dismiss = ui.add(label).on_hover_text("Click to dismiss").clicked();
                            keep = ui
                                .button("Keep here")
                                .on_hover_text(
                                    "Back in the pane it would have gone in, on an axis of its own",
                                )
                                .clicked();
                        });
                    });
            });
        if keep {
            self.moves.push((o.trace, Some(MoveTo::NewAxis(o.from))));
            self.overflow = None;
        } else if dismiss {
            self.overflow = None;
        } else {
            let left = std::time::Duration::from_secs_f64((until - time).max(0.05));
            ui.ctx().request_repaint_after(left);
        }
    }

    /// The lines to draw in pane `k`, from `data` (this frame's points):
    /// its drawn traces in paint order, the one picked out on top. Bands
    /// are lighter the more traces in the pane have one; a picked-out
    /// trace keeps its band, the others lose theirs.
    fn lines<'a>(&self, k: usize, data: &'a [Vec<Pt>], dark: bool) -> Vec<render::Line<'a>> {
        let pane = &self.panes[k];
        let axis_of = |i: usize| pane.axis_of.get(i).copied().flatten();
        let binned: Vec<bool> = data.iter().map(|pts| pts.iter().any(Pt::is_bin)).collect();
        let bands = (0..self.traces.len())
            .filter(|&i| axis_of(i).is_some() && binned[i])
            .count();
        self.paint_order()
            .into_iter()
            .filter_map(|i| axis_of(i).map(|axis| (i, axis)))
            .map(|(i, axis)| {
                let t = &self.traces[i];
                let color = self.color(t, dark);
                let (color, width, band) = match (self.highlight, pane.axis_highlight) {
                    (Some(h), _) if h == t.id => (color, 2.5, render::band_alpha(1)),
                    (Some(_), _) => (color.gamma_multiply(0.25), 1.5, 0.0),
                    // An axis picked out: the others' traces dim.
                    (None, Some(a)) if t.axis != Some(a) => (color.gamma_multiply(0.25), 1.5, 0.0),
                    _ => (color, 1.5, render::band_alpha(bands)),
                };
                let band = if binned[i] { band } else { 0.0 };
                render::Line {
                    pts: &data[i],
                    color,
                    width,
                    axis,
                    band,
                }
            })
            .collect()
    }

    /// The visible traces (indices) in the order they are drawn: as listed,
    /// but the one whose chip is hovered last, on top of any it coincides
    /// with.
    fn paint_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.traces.len())
            .filter(|&i| self.traces[i].visible)
            .collect();
        order.sort_by_key(|&i| Some(self.traces[i].id) == self.highlight);
        order
    }

    /// Whether the Live pill's dot pulses this frame: for a few seconds
    /// after following now starts, and while the pointer is over the plot.
    /// Otherwise the dot is still, and the plot repaints only at the
    /// scroll pace (no 15 fps animation in an idle window).
    fn pulsing(&mut self, ui: &egui::Ui, time: f64) -> bool {
        if !self.window.live {
            self.live_since = None;
            return false;
        }
        let since = *self.live_since.get_or_insert(time);
        time - since < PULSE_SECS || ui.ui_contains_pointer()
    }

    /// The colour of trace `t` on a dark or light background.
    fn color(&self, t: &Trace, dark: bool) -> Color32 {
        color_of(t, dark)
    }

    /// The time range to draw this frame, easing toward the window.
    fn shown_range(&mut self, now: f64, dt: f64) -> (f64, f64) {
        let (from, to) = self.window.range(now);
        let (mut a, mut b) = match self.shown {
            Some(s) if !std::mem::take(&mut self.snap) => s,
            _ => (from, to),
        };
        let span = to - from;
        window::ease(&mut a, from, dt, span);
        window::ease(&mut b, to, dt, span);
        self.shown = Some((a, b));
        (a, b)
    }

    /// What trace `i`'s values look like, from `data` (this frame's
    /// points); `None` with nothing to show.
    fn trace_stats(&self, i: usize, data: &[Vec<Pt>]) -> Option<Stats> {
        let t = &self.traces[i];
        let mut s = Stats::of(data[i].iter().map(|p| p.v), &units(t), whole_valued(t))?;
        if s.whole {
            s.states = t.pv.state().labels;
        }
        Some(s)
    }

    /// Where trace `i`, with `stats`, settles: an axis that suits it, in
    /// its own pane (one put in a pane by hand or by a view) or in any;
    /// else an axis of its own in the first pane with room (or the one
    /// a view kept for it); else in a new pane, said by a toast. Its id.
    fn place(&mut self, i: usize, stats: &Stats) -> u32 {
        let t = &self.traces[i];
        let id = t.id;
        let panes: Vec<usize> = match t.pane.and_then(|p| self.pane_index(p)) {
            Some(k) => vec![k],
            None => (0..self.panes.len()).collect(),
        };
        let axes: Vec<PaneAxes> = panes.iter().map(|&k| self.pane_axes(k)).collect();
        let kept = (self.all_axes())
            .find(|a| !a.used && a.reserved == Some(id))
            .map(|a| a.id);
        match (axes::place(stats, &axes), kept) {
            (Place::Join(_, axis), _) => axis,
            (_, Some(axis)) => axis,
            (Place::New(k), None) => self.new_axis(panes[k], stats, id),
            (Place::NewPane, None) => {
                self.overflow = Some(Overflow {
                    trace: id,
                    from: self.panes[panes[0]].id,
                    axes: axes[0].values(),
                    until: None,
                });
                let k = self.open_pane();
                self.new_axis(k, stats, id)
            }
        }
    }

    /// A new pane at the bottom: its index.
    fn open_pane(&mut self) -> usize {
        self.panes.push(Pane::new(self.next_pane));
        self.next_pane += 1;
        self.panes.len() - 1
    }

    /// Pane `k`'s axes, for placing a trace.
    fn pane_axes(&self, k: usize) -> PaneAxes {
        let (known, unknown) = self.axis_stats(k);
        PaneAxes { known, unknown }
    }

    /// The stats of each axis with traces in pane `k`, by id, from its
    /// traces' stats as last seen (hidden ones too), and how many axes
    /// with traces have none yet (from a view, never shown).
    fn axis_stats(&self, k: usize) -> (Vec<(u32, Stats)>, usize) {
        let mut known = Vec::new();
        let mut unknown = 0;
        for a in &self.panes[k].axes {
            let on: Vec<&Trace> = self
                .traces
                .iter()
                .filter(|t| t.axis == Some(a.id))
                .collect();
            if on.is_empty() {
                continue;
            }
            match Stats::merge(on.iter().filter_map(|t| t.stats.as_ref())) {
                Some(s) => known.push((a.id, s)),
                None => unknown += 1,
            }
        }
        (known, unknown)
    }

    /// Every value axis, pane by pane.
    fn all_axes(&self) -> impl Iterator<Item = &ValueAxis> {
        self.panes.iter().flat_map(|p| &p.axes)
    }

    fn all_axes_mut(&mut self) -> impl Iterator<Item = &mut ValueAxis> {
        self.panes.iter_mut().flat_map(|p| &mut p.axes)
    }

    /// The value axis with id `id`, in whichever pane.
    fn axis_mut(&mut self, id: u32) -> Option<&mut ValueAxis> {
        self.panes.iter_mut().find_map(|p| p.axis_mut(id))
    }

    /// The pane (index) that axis `id` is in.
    fn axis_pane(&self, id: u32) -> Option<usize> {
        self.panes.iter().position(|p| p.axis(id).is_some())
    }

    /// The pane (index) with id `id`.
    fn pane_index(&self, id: u32) -> Option<usize> {
        self.panes.iter().position(|p| p.id == id)
    }

    /// The pane (index) trace `t` shows in: its own, or the first for one
    /// in none yet.
    fn home(&self, t: &Trace) -> usize {
        t.pane.and_then(|id| self.pane_index(id)).unwrap_or(0)
    }

    /// Whether trace `t` still waits for its first archived history (not
    /// from an archiver known to be unreachable).
    fn awaits_history(&self, t: &Trace) -> bool {
        self.archiver.is_some()
            && self.monitor.status.state != ArchiverState::Unreachable
            && t.archive_name().is_some()
            && !t.fetch.not_archived
            && t.history.is_none()
            && t.fetch.error.is_none()
    }

    /// Settle new traces on axes, once each, in the legend's order, so
    /// the outcome does not depend on which archive answer comes first.
    ///
    /// A trace is ready when it has data on screen and its archived
    /// history has come (live data alone: after [`LIVE_WAIT`], so one
    /// first sample does not decide). It waits while an earlier trace may
    /// still get data: until that one settles, or turns out to have none,
    /// or [`SETTLE_WAIT`] passes. Then the axes left with no trace go.
    fn settle_axes(&mut self, data: &[Vec<Pt>], now: f64) {
        // Settled traces shown since: their stats, for scoring.
        for i in 0..self.traces.len() {
            if self.traces[i].axis.is_some() && self.traces[i].stats.is_none() {
                self.traces[i].stats = self.trace_stats(i, data);
            }
        }
        let mut blocked = false;
        for i in 0..self.traces.len() {
            let t = &self.traces[i];
            // Arrays are not plotted: they take no axis, nor hold others up.
            if t.axis.is_some() || !t.visible || t.is_array() {
                continue;
            }
            let seen = *self.traces[i].seen_at.get_or_insert(now);
            let late = now - seen >= SETTLE_WAIT;
            let stats = self.trace_stats(i, data);
            if stats.is_some() {
                self.traces[i].data_since.get_or_insert(now);
            }
            let t = &self.traces[i];
            let pending = self.awaits_history(t);
            let archived = t.history.as_ref().is_some_and(|h| !h.samples.is_empty());
            let live = !pending && t.data_since.is_some_and(|s| now - s >= LIVE_WAIT);
            // No units from the archive: its live units may yet come, and
            // should decide (within the wait).
            let units_due = units(t).is_empty() && t.archive_name().is_some() && !t.pv.connected();
            if stats.is_none() || !(late || ((archived || live) && !units_due)) {
                // It may yet get data, or more of it: those after it wait.
                blocked |= !late && (pending || stats.is_some());
                if late && stats.is_none() {
                    // Nothing came in time: an axis kept for it (from an
                    // old view) is not kept any longer.
                    let id = self.traces[i].id;
                    for a in self.all_axes_mut() {
                        if a.reserved == Some(id) {
                            a.reserved = None;
                        }
                    }
                }
                continue;
            }
            if blocked {
                continue;
            }
            let stats = stats.expect("ready");
            let id = self.place(i, &stats);
            self.traces[i].stats = Some(stats);
            self.put_on(i, id);
        }
        self.prune_axes();
    }

    /// An axis for trace `trace`, with `stats`, to settle on: one from a
    /// view that no trace is on yet (kept for this trace, or for any),
    /// keeping its settings; else a new one in pane `k`, on a log scale
    /// for values over decades (or when the view said log). Its id.
    fn new_axis(&mut self, k: usize, stats: &Stats, trace: u64) -> u32 {
        if let Some(a) = self
            .all_axes()
            .find(|a| !a.used && a.reserved == Some(trace))
        {
            return a.id;
        }
        let mut a = ValueAxis::new(self.next_axis);
        self.next_axis += 1;
        a.log = (self.default_log && stats.positive) || stats.wants_log();
        let id = a.id;
        self.panes[k].axes.push(a);
        id
    }

    /// Put trace `i` on axis `id`, in its pane.
    fn put_on(&mut self, i: usize, id: u32) {
        let Some(k) = self.axis_pane(id) else {
            return;
        };
        self.traces[i].axis = Some(id);
        self.traces[i].pane = Some(self.panes[k].id);
        if let Some(a) = self.panes[k].axis_mut(id) {
            a.used = true;
            a.reserved = None;
        }
    }

    /// Move the trace with id `trace` by hand, to axis or pane `to`.
    ///
    /// Alone on its axis already, a new axis in its own pane leaves it
    /// there (keeping the axis' range, scale and name), as a new pane
    /// does when it is alone in its pane. In another pane it joins the
    /// axis there that suits it, else gets one of its own (on a log scale
    /// when its values want it); one not settled yet settles there.
    fn move_trace(&mut self, trace: u64, to: MoveTo) {
        let Some(i) = self.traces.iter().position(|t| t.id == trace) else {
            return;
        };
        let t = &self.traces[i];
        let on = t.axis;
        let alone = on.is_some() && self.traces.iter().filter(|x| x.axis == on).count() == 1;
        let lone = t.pane.is_some() && self.traces.iter().filter(|x| x.pane == t.pane).count() == 1;
        let home = self.panes[self.home(t)].id;
        let stats = t.stats.clone();
        let k = match to {
            MoveTo::Axis(id) if self.axis_pane(id).is_some() => {
                self.put_on(i, id);
                self.prune_axes();
                return;
            }
            MoveTo::Axis(_) => return,
            MoveTo::NewAxis(p) if p == home && alone => return,
            MoveTo::NewAxis(p) => match self.pane_index(p) {
                Some(k) => {
                    // A new axis in its own pane is as plain as ever; in
                    // another (keeping it where it overflowed from), as
                    // settling would make it.
                    let stats = stats.as_ref().filter(|_| p != home);
                    let id = self.fresh_axis(k, stats);
                    self.put_on(i, id);
                    self.prune_axes();
                    return;
                }
                None => return,
            },
            MoveTo::Pane(p) if Some(p) == self.traces[i].pane => return,
            MoveTo::Pane(p) => match self.pane_index(p) {
                Some(k) => k,
                None => return,
            },
            MoveTo::NewPane if lone => return,
            MoveTo::NewPane => self.open_pane(),
        };
        let Some(stats) = stats else {
            // Not settled: it settles there when its data comes.
            self.traces[i].axis = None;
            self.traces[i].pane = Some(self.panes[k].id);
            self.prune_axes();
            return;
        };
        let id = match axes::place(&stats, &[self.pane_axes(k)]) {
            Place::Join(_, id) => id,
            _ => self.fresh_axis(k, Some(&stats)),
        };
        self.put_on(i, id);
        self.prune_axes();
    }

    /// A new axis in pane `k`, not one waiting for a view's trace: on a
    /// log scale when `stats` (those of the trace for it) want it. Its
    /// id.
    fn fresh_axis(&mut self, k: usize, stats: Option<&Stats>) -> u32 {
        let mut a = ValueAxis::new(self.next_axis);
        self.next_axis += 1;
        a.log = stats.is_some_and(|s| (self.default_log && s.positive) || s.wants_log());
        let id = a.id;
        self.panes[k].axes.push(a);
        id
    }

    /// Drop the axes with no trace: those whose last trace has left, and
    /// one from a view kept for a trace that is gone, is an array, or
    /// got no data. Keep the others in the legend's order (of each one's
    /// first trace), so the first chip's axis is always the first. Then
    /// close the panes left with neither traces nor axes.
    fn prune_axes(&mut self) {
        let traces = &self.traces;
        for pane in &mut self.panes {
            pane.axes.retain(|a| {
                traces.iter().any(|t| t.axis == Some(a.id))
                    || (!a.used
                        && a.reserved
                            .is_some_and(|r| traces.iter().any(|t| t.id == r && !t.is_array())))
            });
            pane.axes.sort_by_key(|a| {
                traces
                    .iter()
                    .position(|t| t.axis == Some(a.id))
                    .unwrap_or(usize::MAX)
            });
            if let Some(h) = pane.axis_highlight
                && pane.axis(h).is_none()
            {
                pane.axis_highlight = None;
            }
        }
        let keep: Vec<bool> = (self.panes.iter())
            .map(|p| !p.axes.is_empty() || traces.iter().any(|t| t.pane == Some(p.id)))
            .collect();
        if keep.iter().any(|k| *k) {
            let mut keep = keep.into_iter();
            self.panes.retain(|_| keep.next().unwrap_or(true));
        } else {
            self.panes.truncate(1);
        }
    }

    /// Pane `k`'s axes' names, by id, for those with traces: as the user
    /// named them, else built from their traces' units and names.
    fn name_axes(&self, k: usize) -> Vec<(u32, String)> {
        let labels: Vec<String> = self.traces.iter().map(Trace::label).collect();
        let unit: Vec<String> = self.traces.iter().map(units).collect();
        let descs: Vec<String> = self
            .traces
            .iter()
            .map(|t| {
                t.history
                    .as_ref()
                    .and_then(|h| h.meta.desc.clone())
                    .unwrap_or_default()
            })
            .collect();
        let with: Vec<&ValueAxis> = self.panes[k]
            .axes
            .iter()
            .filter(|a| self.traces.iter().any(|t| t.axis == Some(a.id)))
            .collect();
        let members: Vec<Members> = with
            .iter()
            .map(|a| {
                let on: Vec<usize> = (0..self.traces.len())
                    .filter(|&i| self.traces[i].axis == Some(a.id))
                    .collect();
                Members {
                    units: on.iter().map(|&i| unit[i].as_str()).collect(),
                    names: on.iter().map(|&i| labels[i].as_str()).collect(),
                    descs: on.iter().map(|&i| descs[i].as_str()).collect(),
                    states: on.iter().all(|&i| whole_valued(&self.traces[i])),
                }
            })
            .collect();
        let auto = axes::names(&members);
        with.iter()
            .zip(auto)
            .map(|(a, auto)| (a.id, a.name.clone().unwrap_or(auto)))
            .collect()
    }

    /// Axis `id`'s name as last drawn, else as it would be named now.
    fn axis_name(&self, id: u32) -> Option<String> {
        let k = self.axis_pane(id)?;
        match self.panes[k].axis_name(id) {
            Some(name) => Some(name.to_string()),
            None => (self.name_axes(k).into_iter())
                .find(|(a, _)| *a == id)
                .map(|(_, n)| n),
        }
    }

    /// The value axes to draw in pane `k` this frame: each axis with a
    /// visible trace, in order. A trace that has not settled is not drawn
    /// yet. Sets the pane's `drawn`, `axis_of` and `axis_names`.
    fn value_axes(
        &mut self,
        k: usize,
        data: &[Vec<Pt>],
        from: f64,
        to: f64,
        dt: f64,
        dark: bool,
    ) -> Vec<render::AxisSpec> {
        let names = self.name_axes(k);
        let traces = &self.traces;
        let pane = &mut self.panes[k];
        pane.axis_names = names;
        let drawn: Vec<u32> = pane
            .axes
            .iter()
            .filter(|a| {
                traces
                    .iter()
                    .any(|t| t.visible && !t.is_array() && t.axis == Some(a.id))
            })
            .map(|a| a.id)
            .collect();
        if let Some(h) = pane.axis_highlight
            && !drawn.contains(&h)
        {
            pane.axis_highlight = None;
        }
        pane.axis_of = traces
            .iter()
            .map(|t| {
                t.axis
                    .filter(|_| t.visible && !t.is_array())
                    .and_then(|id| drawn.iter().position(|d| *d == id))
            })
            .collect();
        let several = drawn.len() > 1;
        let mut specs = Vec::new();
        for (n, id) in drawn.iter().enumerate() {
            let members: Vec<usize> = (0..traces.len())
                .filter(|&i| pane.axis_of[i] == Some(n))
                .collect();
            let whole = !members.is_empty() && members.iter().all(|&i| whole_valued(&traces[i]));
            let pts = members.iter().map(|&i| &data[i]);
            let a = pane.axes.iter_mut().find(|a| a.id == *id).expect("drawn");
            let (lo, hi) = shown_values(a, pts, (from, to), whole, dt);
            let log = a.log;
            let title = pane.axis_name(*id).unwrap_or_default().to_string();
            let colors: Vec<Color32> = members
                .iter()
                .map(|&i| color_of(&traces[i], dark))
                .collect();
            let (color, dots) = match colors.as_slice() {
                [one] => (Some(*one), Vec::new()),
                _ => (None, colors),
            };
            specs.push(render::AxisSpec {
                lo,
                hi,
                log,
                title,
                color,
                // With one axis, its name alone in its trace's colour.
                tint_labels: several,
                dots,
                whole,
                highlight: Some(*id) == pane.axis_highlight,
            });
        }
        pane.drawn = drawn;
        specs
    }

    /// Take in finished fetches, searches and archiver probes, and what
    /// they say of the archiver.
    fn take_results(&mut self, now: f64) {
        let mut back = false;
        if let Some((epoch, result)) = self.fetcher.take_probed() {
            back |= self.monitor.probed(epoch, result, now);
        }
        for done in self.fetcher.take() {
            let Some(t) = self.traces.iter_mut().find(|t| t.id == done.trace) else {
                continue;
            };
            if done.generation != t.fetch.generation {
                continue;
            }
            t.fetch.in_flight = false;
            match &done.result {
                Ok(_) => {
                    t.fetch.failures = 0;
                    back |= self.monitor.ok(now);
                }
                Err(e) if status::archiver_fault(e, false) => self.monitor.suspect(e, now),
                // Not archived, or something about this PV: the archiver
                // answered, but that says nothing of the others.
                Err(_) => {}
            }
            // The window really fetched: a raw-only trace's is shorter.
            if let Some(r) = &mut t.fetch.requested
                && !done.append
            {
                r.from = r.from.max(done.from);
            }
            if let Some(info) = &done.info {
                t.fetch.raw_only = Some(!info.binnable());
            }
            match done.result {
                Ok(series) if done.append => {
                    let series = series.unwrap_or_default();
                    let keep_from = t.fetch.requested.map_or(f64::NEG_INFINITY, |r| r.from);
                    match &mut t.history {
                        Some(h) if !h.binned && !series.binned => {
                            h.append(series.samples, keep_from, done.to);
                            t.fetch.error = None;
                            // Grown past what `optimized_N` returns raw:
                            // fetch it whole again, binned.
                            if fetch::outgrown(t.fetch.requested.map(|r| r.op), h.samples.len()) {
                                t.fetch.requested = None;
                            }
                        }
                        // Not what the history holds: fetch it all again.
                        _ => t.fetch.requested = None,
                    }
                }
                Ok(series) => {
                    let series = series.unwrap_or_default();
                    let scalar = series.kind == ValueKind::Scalar;
                    if series.samples.is_empty() {
                        // Nothing to tell the type by.
                    } else if !scalar && done.op != Operator::Raw {
                        // Text: ask again, raw. A waveform is not plotted,
                        // so what came is enough.
                        t.fetch.raw_only = Some(true);
                        if series.kind != ValueKind::Waveform {
                            t.fetch.requested = None;
                        }
                    } else if t.fetch.raw_only.is_none() {
                        t.fetch.raw_only = Some(!scalar);
                    }
                    let meta = if series.meta.name.is_empty() {
                        t.history
                            .as_ref()
                            .map(|h| h.meta.clone())
                            .unwrap_or_default()
                    } else {
                        series.meta
                    };
                    // An array's samples are not plotted: knowing it is
                    // archived is enough, so they are not kept.
                    let mut samples = series.samples;
                    if series.kind == ValueKind::Waveform {
                        samples = Vec::new();
                    }
                    t.history = Some(History {
                        to: done.to,
                        binned: series.binned,
                        kind: series.kind,
                        meta,
                        samples,
                    });
                    t.fetch.error = None;
                }
                Err(e) if e.is_not_archived() => {
                    t.fetch.not_archived = true;
                    t.fetch.error = None;
                    t.history = None;
                }
                Err(e) => {
                    log::warn!("{}: {e}", t.name);
                    t.fetch.archivers_fault = status::archiver_fault(&e, false);
                    t.fetch.error = Some(e.to_string());
                    t.fetch.failures += 1;
                }
            }
        }
        back |= bar::take_found(self, now);
        if back {
            // The archiver is back: ask again now for what failed because
            // it was down (not for what fails for reasons of its own,
            // which keeps backing off).
            for t in &mut self.traces {
                if t.fetch.error.is_some() && t.fetch.archivers_fault && !t.fetch.in_flight {
                    t.fetch.failures = 0;
                    t.fetch.requested = None;
                }
            }
        }
    }

    /// Check the archiver when one was just set, and every
    /// `status::REPROBE` seconds while it cannot be reached.
    fn schedule_probe(&mut self, now: f64) {
        let Some(client) = &self.archiver else {
            return;
        };
        if self.monitor.should_probe(now) {
            self.monitor.sent(now);
            self.fetcher
                .probe(&self.pvs, client, self.monitor.epoch, self.repaint.clone());
        }
        if self.monitor.status.state == ArchiverState::Unreachable {
            // Wake for the next probe even in an idle window.
            let next = self.monitor.probed_at + self.monitor.reprobe - now;
            if let Some(r) = &self.repaint {
                r.ctx.request_repaint_after_for(
                    std::time::Duration::from_secs_f64(next.clamp(0.05, status::REPROBE)),
                    r.viewport,
                );
            }
        }
    }

    /// Start the fetches the visible window needs, debounced while the
    /// user pans and zooms.
    fn schedule_fetches(&mut self, now: f64, width_px: f32) {
        self.pending = false;
        let Some(client) = self.archiver.clone() else {
            return;
        };
        let (from, to) = self.window.range(now);
        let settled = now - self.changed_at >= DEBOUNCE;
        let live = self.window.live;
        for t in &mut self.traces {
            let Some(name) = t.archive_name() else {
                continue;
            };
            // One request per trace at a time: when it is done, the next
            // frame asks for wherever the window is then.
            if t.fetch.not_archived || t.fetch.in_flight {
                continue;
            }
            // An array is not plotted: once it is known to be archived,
            // there is nothing more to fetch.
            if t.history
                .as_ref()
                .is_some_and(|h| h.kind == ValueKind::Waveform)
            {
                continue;
            }
            let raw = t.fetch.raw_only.unwrap_or(false);
            let from = fetch::needed_from(from, to, raw);
            // A live refresh of raw history: only from its last sample on.
            let mut tail = None;
            let need = match t.fetch.requested {
                None => true,
                Some(r) => {
                    let binned =
                        r.op != Operator::Raw && t.history.as_ref().is_none_or(|h| h.binned);
                    let covered = fetch::covers((r.from, r.to), binned, from, to);
                    // After an error: again once the backoff has passed, or
                    // after the base delay for a different window.
                    let wait = if covered {
                        fetch::backoff(RETRY, t.fetch.failures)
                    } else {
                        RETRY
                    };
                    if t.fetch.error.is_some() {
                        now - r.at > wait
                    } else if !covered {
                        true
                    } else if live
                        && now - r.at
                            > fetch::refresh_after(LIVE_REFRESH, (r.from, r.to), r.op, binned)
                    {
                        tail = t
                            .history
                            .as_ref()
                            .filter(|h| !h.binned)
                            .and_then(|h| h.samples.last())
                            .map(|s| s.t)
                            .filter(|t| *t > r.from)
                            .map(|t| (t, r));
                        true
                    } else {
                        false
                    }
                }
            };
            if !need {
                continue;
            }
            if t.fetch.requested.is_some() && !settled {
                self.pending = true;
                continue;
            }
            let (a, b) = fetch::padded(from, to);
            let op = fetch::operator(width_px, raw);
            let raw_from = fetch::padded(fetch::needed_from(from, to, true), to).0;
            t.fetch.generation += 1;
            t.fetch.in_flight = true;
            let job = match tail {
                // The history keeps what is still in the padded window.
                Some((last, r)) => {
                    t.fetch.requested = Some(series::Requested {
                        from: r.from.max(a).min(last),
                        to: b,
                        op: r.op,
                        at: now,
                    });
                    fetch::Job {
                        trace: t.id,
                        generation: t.fetch.generation,
                        pv: name,
                        from: last,
                        to: b,
                        op: Operator::Raw,
                        raw_from: last,
                        probe: false,
                        append: true,
                    }
                }
                None => {
                    t.fetch.requested = Some(series::Requested {
                        from: a,
                        to: b,
                        op,
                        at: now,
                    });
                    fetch::Job {
                        trace: t.id,
                        generation: t.fetch.generation,
                        pv: name,
                        from: a,
                        to: b,
                        op,
                        raw_from,
                        probe: t.fetch.raw_only.is_none(),
                        append: false,
                    }
                }
            };
            self.fetcher
                .fetch(&self.pvs, &client, job, self.repaint.clone());
        }
    }

    /// Pan, zoom, box zoom and reset on pane `k` (`resp` its response,
    /// `f` its frame): time for every pane, values for its own axes.
    fn interact(
        &mut self,
        k: usize,
        ui: &egui::Ui,
        resp: &egui::Response,
        f: &render::Frame,
        now: f64,
    ) {
        let data = f.data;
        let span = self.window.span;
        let per_px = span / data.width().max(1.0) as f64;

        if resp.drag_started() {
            let origin = ui
                .input(|i| i.pointer.press_origin())
                .unwrap_or(data.center());
            let shift = ui.input(|i| i.modifiers.shift);
            self.glide = 0.0;
            let axis = f
                .axis_at(origin)
                .and_then(|n| self.panes[k].drawn.get(n).copied());
            self.drag = Some(
                if shift || resp.dragged_by(egui::PointerButton::Secondary) {
                    Drag::Box(origin)
                } else if let Some(id) = axis {
                    Drag::Axis(id)
                } else {
                    Drag::Pan
                },
            );
        }
        if resp.dragged() {
            let d = resp.drag_delta();
            match self.drag {
                Some(Drag::Pan) => {
                    self.window.pan(-d.x as f64 * per_px, now);
                    if d.y != 0.0 {
                        // Axes with a fixed range move with the pointer.
                        let frac = -(d.y / data.height().max(1.0)) as f64;
                        for a in self.drawn_axes_mut(k).filter(|a| !a.autoscale) {
                            let (lo, hi) = a.shown.unwrap_or((a.lo, a.hi));
                            (a.lo, a.hi) = window::pan_values(lo, hi, frac, a.log);
                            a.shown = Some((a.lo, a.hi));
                        }
                    }
                    self.snap = true;
                    self.changed_at = now;
                }
                Some(Drag::Axis(id)) => {
                    let factor = (d.y as f64 * 0.01).exp();
                    if let Some(n) = self.panes[k].drawn.iter().position(|d| *d == id) {
                        self.zoom_axis(k, n, factor, None, f);
                    }
                    if let Some(a) = self.axis_mut(id) {
                        a.shown = Some((a.lo, a.hi));
                    }
                }
                _ => {}
            }
        }
        if resp.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Pan) => {
                    let v = ui.input(|i| i.pointer.velocity()).x as f64;
                    if v.abs() > 150.0 {
                        self.glide = -v * per_px;
                    }
                }
                Some(Drag::Box(origin)) => {
                    if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
                        let r = Rect::from_two_pos(origin, p).intersect(data);
                        if r.width() > 6.0 || r.height() > 6.0 {
                            self.name_step("zoom to box");
                        }
                        if r.width() > 6.0 {
                            self.window.set(f.time_at(r.left()), f.time_at(r.right()));
                        }
                        if r.height() > 6.0 {
                            let pane = &mut self.panes[k];
                            for (n, id) in pane.drawn.clone().into_iter().enumerate() {
                                let range = (f.value_on(n, r.bottom()), f.value_on(n, r.top()));
                                if let Some(a) = pane.axis_mut(id) {
                                    a.autoscale = false;
                                    (a.lo, a.hi) = range;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if resp.contains_pointer() {
            let (smooth, wheel, zoom, mods, pointer) = ui.input(|i| {
                (
                    i.smooth_scroll_delta,
                    (i.events.iter()).any(|e| matches!(e, egui::Event::MouseWheel { .. })),
                    i.zoom_delta(),
                    i.modifiers,
                    i.pointer.hover_pos(),
                )
            });
            // After an undo, the rest of a smoothed scroll is dropped
            // until the wheel turns again: it would zoom on from the
            // state undone to.
            if wheel {
                self.undo.hold_scroll = false;
            }
            let scroll = if self.undo.hold_scroll {
                Vec2::ZERO
            } else {
                smooth
            };
            let anchor = pointer.map_or(1.0, |p| ((p.x - data.left()) / data.width()) as f64);
            if mods.shift && (scroll.x != 0.0 || scroll.y != 0.0) {
                // Shift+wheel arrives as horizontal scrolling.
                let amount = (scroll.x + scroll.y) as f64;
                self.zoom_values(k, (-amount / 300.0).exp(), pointer, f);
            } else if zoom != 1.0 {
                if mods.ctrl || mods.command {
                    self.zoom_values(k, 1.0 / zoom as f64, pointer, f);
                } else {
                    self.window.zoom(1.0 / zoom as f64, anchor, now);
                    self.changed_at = now;
                }
            } else {
                if scroll.y != 0.0 {
                    self.window
                        .zoom((-scroll.y as f64 / 300.0).exp(), anchor, now);
                    self.changed_at = now;
                }
                if scroll.x != 0.0 {
                    self.window.pan(-scroll.x as f64 * per_px, now);
                    self.changed_at = now;
                    self.snap = true;
                }
            }
            if smooth != Vec2::ZERO {
                // Not for a scroll area around the plot too.
                ui.ctx().input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
            }
        }
        if resp.double_clicked() {
            self.reset();
        }
    }

    /// Zoom pane `k`'s values by `factor` about the pointer at `at`: the
    /// axis under it alone, or over the data every axis drawn, each about
    /// its own value there (the middle without a pointer). Leaves
    /// autoscale.
    fn zoom_values(&mut self, k: usize, factor: f64, at: Option<Pos2>, f: &render::Frame) {
        match at.and_then(|p| f.axis_at(p)) {
            Some(n) => self.zoom_axis(k, n, factor, at, f),
            None => {
                for n in 0..self.panes[k].drawn.len() {
                    self.zoom_axis(k, n, factor, at, f);
                }
            }
        }
    }

    /// Zoom pane `k`'s drawn axis `n` by `factor` about its value at
    /// `at`'s height.
    fn zoom_axis(&mut self, k: usize, n: usize, factor: f64, at: Option<Pos2>, f: &render::Frame) {
        let s = f.scale(n);
        let (shown, value) = ((s.lo, s.hi), at.map(|p| f.value_on(n, p.y)));
        let pane = &mut self.panes[k];
        if let Some(id) = pane.drawn.get(n).copied()
            && let Some(a) = pane.axis_mut(id)
        {
            a.zoom(factor, value, shown);
        }
    }

    /// The value axes drawn in pane `k` this frame.
    fn drawn_axes_mut(&mut self, k: usize) -> impl Iterator<Item = &mut ValueAxis> {
        let pane = &mut self.panes[k];
        let drawn = &pane.drawn;
        pane.axes.iter_mut().filter(|a| drawn.contains(&a.id))
    }

    /// Every value axis back to autoscaling.
    fn autoscale(&mut self) {
        self.name_step("autoscale");
        for a in self.all_axes_mut() {
            a.autoscale = true;
        }
    }

    /// Whether value axis `a` has traces on it.
    fn has_traces(&self, a: &ValueAxis) -> bool {
        self.traces.iter().any(|t| t.axis == Some(a.id))
    }

    /// Every value axis with traces autoscales.
    fn autoscaling(&self) -> bool {
        self.all_axes()
            .filter(|a| self.has_traces(a))
            .all(|a| a.autoscale)
    }

    /// Every value axis with traces is on a log scale (and there is one).
    fn all_log(&self) -> bool {
        let mut on = self.all_axes().filter(|a| self.has_traces(a)).peekable();
        on.peek().is_some() && on.all(|a| a.log)
    }

    /// Every value axis on a log scale, or none; new axes too.
    fn set_log(&mut self, log: bool) {
        self.default_log = log;
        for a in self.all_axes_mut() {
            a.log = log;
            a.fit_log_range();
        }
    }

    /// Back to the home span, following now, values fitted.
    fn reset(&mut self) {
        self.name_step("reset");
        self.window = Window::live(self.home_span);
        self.autoscale();
        self.glide = 0.0;
    }

    /// Toggle following now: pause at the range shown, or jump to now.
    fn toggle_live(&mut self, now: f64) {
        self.name_step(if self.window.live {
            "pause"
        } else {
            "follow now"
        });
        if self.window.live {
            let (from, to) = self.window.range(now);
            self.window = Window::fixed(from, to);
        } else {
            self.window = Window::live(self.window.span);
        }
        self.glide = 0.0;
    }

    /// Show a range picked in the bar: a preset, or one from the custom
    /// popover. Its span becomes the one reset goes back to.
    fn pick_range(&mut self, w: Window) {
        self.name_step(range_label(&w));
        self.window = w;
        self.home_span = w.span;
        self.glide = 0.0;
    }

    /// The keyboard shortcuts, when the pointer is over the plot or it was
    /// clicked last, and no text field has the keyboard.
    fn keyboard(&mut self, ui: &egui::Ui, whole: Rect, now: f64) {
        use egui::{Key, Modifiers, PointerButton};
        let over = ui.rect_contains_pointer(whole);
        // The mouse's back and forward buttons, as in a browser.
        if !self.undo.hidden && over {
            let (back, forward) = ui.input(|i| {
                (
                    i.pointer.button_pressed(PointerButton::Extra1),
                    i.pointer.button_pressed(PointerButton::Extra2),
                )
            });
            if back {
                self.undo();
            }
            if forward {
                self.redo();
            }
        }
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        if typing || !(self.active || over) {
            return;
        }
        if !self.undo.hidden {
            // Ctrl+Z would match Ctrl+Shift+Z too: redo first.
            let (undo, redo) = ui.input_mut(|i| {
                let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                    || i.consume_key(Modifiers::COMMAND, Key::Y);
                (i.consume_key(Modifiers::COMMAND, Key::Z), redo)
            });
            if undo {
                self.undo();
            }
            if redo {
                self.redo();
            }
        }
        let pressed = |k: Key| ui.input(|i| i.key_pressed(k) && !i.modifiers.ctrl);
        let span = self.window.span;
        if pressed(Key::Slash) {
            self.search.focus = true;
        }
        if pressed(Key::ArrowLeft) {
            self.window.pan(-0.1 * span, now);
            self.changed_at = now;
        }
        if pressed(Key::ArrowRight) {
            self.window.pan(0.1 * span, now);
            self.changed_at = now;
        }
        if pressed(Key::Plus) || pressed(Key::Equals) {
            self.window.zoom(0.8, 0.5, now);
            self.changed_at = now;
        }
        if pressed(Key::Minus) {
            self.window.zoom(1.25, 0.5, now);
            self.changed_at = now;
        }
        if pressed(Key::L) {
            self.toggle_live(now);
        }
        if pressed(Key::A) {
            self.autoscale();
        }
        if pressed(Key::Home) {
            self.reset();
        }
        // Ctrl+V: egui hands over the clipboard's text.
        let pasted = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Paste(text) => Some(text.clone()),
                _ => None,
            })
        });
        if let Some(text) = pasted {
            self.paste_pvs(&text);
        }
    }
}

/// Axis `a`'s value range to draw this frame: fitted to `data` (its
/// traces' visible points, envelopes included) or fixed, easing toward
/// it.
fn shown_values<'a>(
    a: &mut ValueAxis,
    data: impl Iterator<Item = &'a Vec<Pt>>,
    (from, to): (f64, f64),
    whole: bool,
    dt: f64,
) -> (f64, f64) {
    let target = if a.autoscale {
        fit_values(a.log, whole, data, from, to).or(a.shown)
    } else {
        Some((a.lo, a.hi))
    };
    let target = target.unwrap_or(if a.log { (1.0, 10.0) } else { (0.0, 1.0) });
    let shown = ease_values(a.shown.unwrap_or(target), target, dt);
    a.shown = Some(shown);
    shown
}

/// The height a legend `h` high takes under its pane, with the gap
/// above it.
fn legend_room(h: f32) -> f32 {
    if h > 0.0 { h + 6.0 } else { 0.0 }
}

/// What picking window `w` is called, as an undo step: "show the last
/// 90m", or "show 09:00 → 12:00" (with the dates when they differ).
fn range_label(w: &Window) -> String {
    if w.live {
        return format!("show the last {}", timeaxis::format_span(w.span));
    }
    let (from, to) = w.range(0.0);
    let (a, b) = (timeaxis::format_local(from), timeaxis::format_local(to));
    // "2026-10-17 09:00:00": the seconds only when there are some.
    let short = |t: &str| t.strip_suffix(":00").unwrap_or(t).to_string();
    let (a, b) = (short(&a), short(&b));
    match (a.split_once(' '), b.split_once(' ')) {
        (Some((da, ta)), Some((db, tb))) if da == db => format!("show {ta} → {tb}"),
        _ => format!("show {a} → {b}"),
    }
}

/// Put archiver `url` first in `recent`, at most [`MAX_RECENT`] long.
fn remember(recent: &mut Vec<String>, url: &str) {
    recent.retain(|u| u != url);
    recent.insert(0, url.to_string());
    recent.truncate(MAX_RECENT);
}

/// The colour of trace `t` on a dark or light background.
fn color_of(t: &Trace, dark: bool) -> Color32 {
    t.color
        .as_deref()
        .and_then(crate::style::parse_hex)
        .map(|c| crate::style::adapt(c, dark))
        .unwrap_or_else(|| crate::style::palette(t.slot, dark))
}

/// Whether trace `t` holds whole numbers: an enum (live with state
/// labels, or archived scalar data fetched raw, as only enums are).
fn whole_valued(t: &Trace) -> bool {
    !t.pv.state().labels.is_empty()
        || (t.fetch.raw_only == Some(true)
            && t.history
                .as_ref()
                .is_some_and(|h| h.kind == ValueKind::Scalar && !h.samples.is_empty()))
}

/// The autoscaled range for `data` between `from` and `to` (at least two
/// wide, for `whole` numbers on a linear scale).
fn fit_values<'a>(
    log: bool,
    whole: bool,
    data: impl Iterator<Item = &'a Vec<Pt>>,
    from: f64,
    to: f64,
) -> Option<(f64, f64)> {
    let values = data.flat_map(|pts| {
        let start = pts.partition_point(|p| p.t < from).saturating_sub(1);
        pts[start..]
            .iter()
            .take_while(|p| p.t <= to)
            .flat_map(|p| [p.lo, p.hi, p.v])
    });
    let fit = render::autoscale(log, values);
    if whole && !log {
        fit.map(|(lo, hi)| axis::whole_range(lo, hi))
    } else {
        fit
    }
}

/// A value range `shown` one frame (`dt`) closer to `target`.
fn ease_values(shown: (f64, f64), target: (f64, f64), dt: f64) -> (f64, f64) {
    let (lo, hi) = target;
    let (mut a, mut b) = shown;
    let scale = hi - lo;
    window::ease(&mut a, lo, dt, scale);
    window::ease(&mut b, hi, dt, scale);
    if a.partial_cmp(&b) != Some(std::cmp::Ordering::Less) {
        (a, b) = (lo, hi);
    }
    (a, b)
}

/// The units of trace `t`: the live PV's, else the archive's.
fn units(t: &Trace) -> String {
    let live = t.pv.state().units;
    if !live.is_empty() {
        return live;
    }
    t.history
        .as_ref()
        .and_then(|h| h.meta.egu.clone())
        .unwrap_or_default()
}

/// The precision of trace `t`: the live PV's, else the archive's.
fn precision(t: &Trace, state: &pv::PvState) -> Option<usize> {
    state
        .precision
        .filter(|p| *p >= 0)
        .map(|p| p as usize)
        .or_else(|| {
            t.history
                .as_ref()
                .and_then(|h| h.meta.prec)
                .map(|p| p as usize)
        })
}

/// A value for the readout and chips: at `prec` decimals (in exponent
/// form from a million up), else compactly.
fn format_value(v: f64, prec: Option<usize>, units: &str) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let text = match prec {
        // Large values as the axis labels them: 1.469e9, not 1468514835.908.
        Some(p) if v.abs() >= 1e6 => format!("{v:.*e}", p.min(12)),
        Some(p) => {
            let s = format!("{v:.*}", p.min(12));
            // -0.04 at one decimal reads "0.0", not "-0.0".
            match s.strip_prefix('-') {
                Some(rest) if rest.chars().all(|c| c == '0' || c == '.') => rest.to_string(),
                _ => s,
            }
        }
        None => {
            let a = v.abs();
            if a != 0.0 && !(1e-3..1e6).contains(&a) {
                format!("{v:.3e}")
            } else {
                let s = format!("{v:.4}");
                let s = s.trim_end_matches('0').trim_end_matches('.');
                if s == "-0" { "0".into() } else { s.to_string() }
            }
        }
    };
    if units.is_empty() {
        text
    } else {
        format!("{text} {units}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Plot {
        /// Move a trace to axis `to`, or to a new axis in its pane.
        fn move_by_hand(&mut self, trace: u64, to: Option<u32>) {
            let to = match to {
                Some(id) => MoveTo::Axis(id),
                None => {
                    let t = self.traces.iter().find(|t| t.id == trace).unwrap();
                    MoveTo::NewAxis(self.panes[self.home(t)].id)
                }
            };
            self.move_trace(trace, to);
        }
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn traces_are_added_once_and_named() {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        p.add_pvs(&names(&["A:B", "ca://A:B", " ", "sim://sine"]));
        assert_eq!(p.names(), ["A:B", "sim://sine"]);
        assert_eq!(p.title(), "A:B, sim://sine");
        p.add_pvs(&names(&["C", "D"]));
        assert_eq!(p.title(), "A:B and 3 more");
        p.remove_pv("ca://A:B");
        assert_eq!(p.names(), ["sim://sine", "C", "D"]);
        // A new trace takes the first free colour.
        p.add_pvs(&names(&["E"]));
        let slots: Vec<usize> = p.traces.iter().map(|t| t.slot).collect();
        assert_eq!(slots, [1, 2, 3, 0]);
        assert!(!p.is_loading(), "nothing to fetch without an archiver");
    }

    #[test]
    fn hovering_a_chip_brings_its_trace_to_the_front() {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        p.add_pvs(&names(&["sim://sine", "sim://ramp", "sim://noise"]));
        let mut h = Harness::new(p);
        // On their axes (after a second of live data), so drawn.
        h.until(|p| p.traces.iter().all(|t| t.axis.is_some()));
        assert_eq!(h.plot.paint_order(), [0, 1, 2]);
        let (id, rect, _) = h.plot.chips_shown[0].clone();
        assert_eq!(id, h.plot.traces[0].id);
        h.frame(vec![egui::Event::PointerMoved(rect.center())]);
        h.frame(vec![]);
        assert_eq!(h.plot.highlight, Some(h.plot.traces[0].id));
        assert_eq!(h.plot.paint_order(), [1, 2, 0]);
        // A hidden trace is not drawn at all.
        h.plot.traces[1].visible = false;
        assert_eq!(h.plot.paint_order(), [2, 0]);
    }

    #[test]
    fn every_chip_fits_the_legend() {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        let long = "a long status message that goes on ".repeat(8);
        p.add_pvs(&names(&[
            &format!("loc://status<VString>(\"{}\")", long.trim()),
            "loc://wave<VDoubleArray>(1.5, 2, 3, 4, 5)",
            "sim://sine",
        ]));
        let mut h = Harness::new(p);
        h.frame(vec![]);
        let chips = h.plot.chips_shown.clone();
        assert_eq!(chips.len(), 3);
        let width = 1000.0;
        for (_, rect, _) in &chips {
            assert!(rect.width() <= width, "{rect:?}");
        }
        // The text cut short to fit, the array shown compactly.
        assert!(chips[0].2.ends_with('…'), "{}", chips[0].2);
        assert!(chips[0].2.len() < long.len());
        assert!(chips[1].2.starts_with("[5] 1.5, 2"), "{}", chips[1].2);
        assert!(h.plot.traces[1].is_array());
        assert!(h.plot.traces[1].points(0.0, now(), now()).is_empty());
        // An array's chip picks out nothing: it has no line.
        h.frame(vec![egui::Event::PointerMoved(chips[1].1.center())]);
        h.frame(vec![]);
        assert_eq!(h.plot.highlight, None);
    }

    #[test]
    fn views_round_trip() {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        let v = View {
            pvs: vec![
                TraceView {
                    name: "SR-DI-DCCT-01:SIGNAL".into(),
                    color: None,
                    visible: true,
                    axis: Some(0),
                },
                TraceView {
                    name: "bl01t:SUM".into(),
                    color: Some("#d62728".into()),
                    visible: false,
                    axis: Some(1),
                },
                TraceView {
                    name: "SR-DI-DCCT-01:LIFETIME".into(),
                    color: None,
                    visible: true,
                    axis: Some(1),
                },
            ],
            range: view::Range::Relative { secs: 8.0 * 3600.0 },
            y: YAxis {
                autoscale: false,
                min: Some(0.0),
                max: Some(300.0),
                log: false,
                name: Some("Beam".into()),
            },
            axes: vec![YAxis {
                log: true,
                ..YAxis::default()
            }],
            ..View::default()
        };
        p.set_view(&v);
        assert_eq!(p.view(), v);
        // A file from before there were several axes: its `y` waits for
        // the first trace to settle, and nothing is lost meanwhile.
        let old = View {
            pvs: vec![TraceView {
                name: "A".into(),
                color: None,
                visible: true,
                axis: None,
            }],
            y: YAxis {
                log: true,
                ..YAxis::default()
            },
            ..View::default()
        };
        p.set_view(&old);
        assert_eq!(p.view(), old);
        let fixed = View {
            range: view::Range::Absolute {
                start: "2026-10-17T09:00:00Z".into(),
                end: "2026-10-17T12:00:00Z".into(),
            },
            ..View::default()
        };
        p.set_view(&fixed);
        assert_eq!(p.view(), fixed);
        assert!(p.names().is_empty());
    }

    /// A plot in a headless egui, fed events frame by frame.
    struct Harness {
        ctx: egui::Context,
        plot: Plot,
        t: f64,
        /// The modifier keys held.
        mods: egui::Modifiers,
        /// The window's width (points).
        width: f32,
    }

    impl Harness {
        fn new(plot: Plot) -> Self {
            let mut h = Self {
                ctx: egui::Context::default(),
                plot,
                t: 0.0,
                mods: egui::Modifiers::NONE,
                width: 1000.0,
            };
            h.frame(vec![]);
            h
        }

        fn frame(&mut self, mut events: Vec<egui::Event>) {
            events.insert(0, egui::Event::ModifiersChanged(self.mods));
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(self.width, 600.0),
                )),
                time: Some(self.t),
                events,
                ..Default::default()
            };
            self.t += 0.05;
            let Self { ctx, plot, .. } = self;
            let mut out = ctx.run_ui(input, |ui| plot.ui(ui));
            out.textures_delta.clear();
        }

        fn key(&mut self, key: egui::Key) {
            let ev = |pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(vec![ev(true)]);
            self.frame(vec![ev(false)]);
        }

        fn button(&mut self, at: Pos2, pressed: bool) {
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }]);
        }

        /// Frames until `done`, for up to 10 s of real time.
        fn until(&mut self, mut done: impl FnMut(&Plot) -> bool) {
            let start = std::time::Instant::now();
            while !done(&self.plot) {
                assert!(start.elapsed().as_secs() < 10, "timed out");
                std::thread::sleep(std::time::Duration::from_millis(20));
                self.frame(vec![]);
            }
        }
    }

    const MIDDLE: Pos2 = Pos2::new(500.0, 300.0);

    #[test]
    fn dragging_pans_into_the_past_and_double_click_resets() {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        p.add_pvs(&names(&["sim://sine"]));
        let mut h = Harness::new(p);
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.button(MIDDLE, true);
        for x in [520.0, 600.0, 700.0] {
            h.frame(vec![egui::Event::PointerMoved(Pos2::new(x, 300.0))]);
        }
        h.button(Pos2::new(700.0, 300.0), false);
        assert!(!h.plot.window.live, "dragged right: back in time");
        let (_, to) = h.plot.window.range(now());
        assert!(to < now() - 60.0, "about 200 px of an hour back");
        // Double-click: back to following now.
        for _ in 0..2 {
            h.button(MIDDLE, true);
            h.button(MIDDLE, false);
        }
        assert!(h.plot.window.live);
        assert!(h.plot.autoscaling());
    }

    #[test]
    fn the_wheel_zooms_time_and_keys_work() {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        p.add_pvs(&names(&["sim://sine"]));
        let mut h = Harness::new(p);
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.frame(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, 200.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        }]);
        for _ in 0..20 {
            h.frame(vec![]);
        }
        assert!(h.plot.window.span < 3600.0 * 0.8, "zoomed in");
        assert!(h.plot.window.live, "about now while live");
        // Keys: L pauses, the arrows pan, A autoscales, Home resets.
        h.key(egui::Key::L);
        assert!(!h.plot.window.live);
        let end = h.plot.window.end;
        h.key(egui::Key::ArrowLeft);
        assert!(h.plot.window.end < end);
        h.key(egui::Key::Home);
        assert!(h.plot.window.live);
        assert_eq!(h.plot.window.span, 3600.0);
    }

    #[test]
    fn search_finds_archived_pvs_and_enter_adds_one() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut h = Harness::new(Plot::new(&pvs, Some(client)));
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.key(egui::Key::Slash);
        h.frame(vec![egui::Event::Text("SINE".into())]);
        h.until(|p| p.search.results.iter().any(|n| n == "MOCK:SINE"));
        h.key(egui::Key::Enter);
        assert_eq!(h.plot.names(), ["MOCK:SINE"]);
        assert_eq!(h.plot.search.text, "SINE", "kept, to add more");
        // Its history arrives, binned.
        h.until(|p| !p.is_loading());
        let t = &h.plot.traces[0];
        let hist = t.history.as_ref().expect("history");
        assert!(!hist.samples.is_empty());
        assert_eq!(hist.meta.egu.as_deref(), Some("mA"));
        assert_eq!(t.fetch.raw_only, Some(false));
    }

    /// The search box has the keyboard.
    fn search_focused(h: &Harness) -> bool {
        h.ctx.memory(|m| m.has_focus(h.plot.id.with("search")))
    }

    /// The search box's selection, as character indices.
    fn search_selection(h: &Harness) -> Option<(usize, usize)> {
        let state = egui::text_edit::TextEditState::load(&h.ctx, h.plot.id.with("search"))?;
        let r = state.cursor.char_range()?;
        let (a, b) = (r.primary.index, r.secondary.index);
        let (a, b): (usize, usize) = (a.into(), b.into());
        Some((a.min(b), a.max(b)))
    }

    #[test]
    fn the_search_stays_after_adding_and_is_remembered() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut h = Harness::new(Plot::new(&pvs, Some(client)));
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.key(egui::Key::Slash);
        h.frame(vec![egui::Event::Text("MOCK:S".into())]);
        h.until(|p| p.search.results.len() >= 2);
        let first = h.plot.search.results[0].clone();
        let second = h.plot.search.results[1].clone();

        // Enter adds the first match, and the text, list and focus stay.
        h.key(egui::Key::Enter);
        assert_eq!(h.plot.names(), std::slice::from_ref(&first));
        assert_eq!(h.plot.search.text, "MOCK:S");
        assert!(h.plot.search.open && h.plot.search.results.len() >= 2);
        assert!(search_focused(&h));
        // The search is remembered, once.
        assert!(h.plot.search_history_changed());
        assert!(!h.plot.search_history_changed());
        let texts = |p: &Plot| -> Vec<String> {
            let h = p.search_history().searches();
            h.iter().map(|s| s.text.clone()).collect()
        };
        assert_eq!(texts(&h.plot), ["MOCK:S"]);
        // Enter on the ticked row again does nothing; the next adds.
        h.key(egui::Key::Enter);
        assert_eq!(h.plot.names().len(), 1);
        h.key(egui::Key::ArrowDown);
        h.key(egui::Key::Enter);
        assert_eq!(h.plot.names(), [first, second]);

        // Esc closes the list and keeps the text; a second Esc clears it.
        h.key(egui::Key::Escape);
        assert!(!h.plot.search.open && search_focused(&h));
        assert_eq!(h.plot.search.text, "MOCK:S");
        h.key(egui::Key::Escape);
        assert!(h.plot.search.text.is_empty() && !search_focused(&h));

        // The empty box lists the recent search; picking one searches it.
        h.key(egui::Key::Slash);
        assert!(h.plot.search.open && search_focused(&h));
        let r = super::bar::rows_text(&h.plot);
        assert_eq!(r, ["MOCK:S"]);
        h.key(egui::Key::ArrowDown);
        h.key(egui::Key::Enter);
        assert_eq!(h.plot.search.text, "MOCK:S");
        assert_eq!(h.plot.names().len(), 2, "nothing added");
        assert_eq!(search_selection(&h), Some((6, 6)), "the cursor at the end");
        h.until(|p| p.search.searched == "MOCK:S" && !p.search.results.is_empty());
    }

    #[test]
    fn a_recent_search_is_searched_again_in_its_own_mode() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut p = Plot::new(&pvs, Some(client));
        let mut hist = SearchHistory::default();
        hist.record("MOCK:S", true);
        p.set_search_history(hist);
        let mut h = Harness::new(p);
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.key(egui::Key::Slash);
        // As a substring: several matches.
        h.frame(vec![egui::Event::Text("MOCK:S".into())]);
        h.until(|p| p.search.searched == "MOCK:S" && !p.is_loading());
        assert!(h.plot.search.results.len() >= 2);
        // Emptied, then the recent regex search picked: asked again as a
        // regex, which must match the whole name, so nothing.
        for _ in 0..6 {
            h.key(egui::Key::Backspace);
        }
        assert_eq!(super::bar::rows_text(&h.plot), ["MOCK:S"]);
        h.key(egui::Key::ArrowDown);
        h.key(egui::Key::Enter);
        assert!(h.plot.search.regex);
        assert!(h.plot.search.results.is_empty(), "not the substring's");
        h.until(|p| p.search.searched == "MOCK:S" && !p.is_loading());
        assert!(h.plot.search.results.is_empty());
    }

    #[test]
    fn esc_in_an_empty_box_with_no_list_leaves_it() {
        let pvs = PvManager::new();
        let mut h = Harness::new(Plot::new(&pvs, None));
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.key(egui::Key::Slash);
        assert!(search_focused(&h));
        // No recent searches: nothing is shown, so one Esc is enough.
        h.key(egui::Key::Escape);
        assert!(!search_focused(&h));
    }

    /// A harness with the custom range popover open (as a click on `⋯`).
    fn custom_open() -> Harness {
        let pvs = PvManager::new();
        let mut h = Harness::new(Plot::new(&pvs, None));
        super::bar::open_custom(&mut h.plot, now());
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    /// The custom popover's Last field has the keyboard.
    fn span_focused(h: &Harness) -> bool {
        h.ctx.memory(|m| m.has_focus(h.plot.id.with("custom-span")))
    }

    #[test]
    fn enter_in_last_shows_that_span() {
        let mut h = custom_open();
        assert!(h.plot.custom.open);
        // Focused, its text selected, so typing replaces it.
        assert!(span_focused(&h));
        assert_eq!(h.plot.custom.span, "1h");
        h.frame(vec![egui::Event::Text("90m".into())]);
        assert_eq!(h.plot.custom.span, "90m");
        assert!(h.plot.custom.open, "nothing shown until Enter");
        assert_eq!(h.plot.window.span, 3600.0);
        h.key(egui::Key::Enter);
        assert!(!h.plot.custom.open);
        assert!(h.plot.window.live);
        assert_eq!(h.plot.window.span, 5400.0);
        assert_eq!(h.plot.home_span, 5400.0, "reset comes back to it");
    }

    #[test]
    fn enter_in_to_shows_the_fixed_range() {
        let mut h = custom_open();
        h.plot.custom.from = "2026-10-01 09:00".into();
        h.plot.custom.to = "2026-10-01 12:00".into();
        // Tab: Last → From → To.
        h.key(egui::Key::Tab);
        h.key(egui::Key::Tab);
        assert!(h.plot.custom.open);
        h.key(egui::Key::Enter);
        assert!(!h.plot.custom.open);
        assert!(!h.plot.window.live);
        assert_eq!(h.plot.window.span, 3.0 * 3600.0);
        let (from, _) = h.plot.window.range(now());
        assert_eq!(
            from,
            super::timeaxis::parse_local("2026-10-01 09:00").unwrap()
        );
    }

    #[test]
    fn a_range_picked_in_the_popover_is_one_undo_step() {
        let mut h = custom_open();
        h.frame(vec![egui::Event::Text("90m".into())]);
        h.key(egui::Key::Enter);
        h.frame(vec![]);
        assert_eq!(h.plot.undo.len(), (1, 0), "typing is no step");
        assert_eq!(h.plot.undo_label(), Some("show the last 1h 30m"));
        // A fixed range, the same day: its times.
        super::bar::open_custom(&mut h.plot, now());
        h.frame(vec![]);
        h.frame(vec![]);
        h.plot.custom.from = "2026-10-01 09:00".into();
        h.plot.custom.to = "2026-10-01 12:00".into();
        h.key(egui::Key::Tab);
        h.key(egui::Key::Tab);
        h.key(egui::Key::Enter);
        h.frame(vec![]);
        assert_eq!(h.plot.undo.len(), (2, 0));
        assert_eq!(h.plot.undo_label(), Some("show 09:00 → 12:00"));
        // Undone in turn: the 90 minutes, then the hour, and reset's
        // span with them.
        h.plot.undo();
        assert!(h.plot.window.live);
        assert_eq!(h.plot.window.span, 5400.0);
        h.plot.undo();
        assert_eq!(h.plot.window.span, 3600.0);
        assert_eq!(h.plot.home_span, 3600.0);
        // Across days, with the dates.
        let w = Window::fixed(
            super::timeaxis::parse_local("2026-10-01 09:00").unwrap(),
            super::timeaxis::parse_local("2026-10-02 12:00:30").unwrap(),
        );
        assert_eq!(
            range_label(&w),
            "show 2026-10-01 09:00 → 2026-10-02 12:00:30"
        );
    }

    #[test]
    fn bad_input_keeps_the_popover_open_with_the_error() {
        let mut h = custom_open();
        h.frame(vec![egui::Event::Text("soon".into())]);
        h.key(egui::Key::Enter);
        assert!(h.plot.custom.open);
        assert_eq!(
            h.plot.custom.span_error.as_deref(),
            Some("Not a span: \"soon\"")
        );
        assert_eq!(h.plot.window.span, 3600.0);
        // The field keeps the keyboard, to try again; editing clears it.
        h.frame(vec![]);
        assert!(span_focused(&h));
        h.key(egui::Key::Backspace);
        assert_eq!(h.plot.custom.span_error, None);
        // A range ending before it starts.
        h.plot.custom.from = "2026-10-01 12:00".into();
        h.plot.custom.to = "2026-10-01 09:00".into();
        h.key(egui::Key::Tab);
        h.key(egui::Key::Enter);
        assert!(h.plot.custom.open);
        assert_eq!(
            h.plot.custom.range_error.as_deref(),
            Some("From must be before to")
        );
        assert_eq!(h.plot.window.span, 3600.0);
    }

    #[test]
    fn esc_or_a_click_away_shows_nothing() {
        let mut h = custom_open();
        h.frame(vec![egui::Event::Text("5h".into())]);
        h.key(egui::Key::Escape);
        assert!(!h.plot.custom.open);
        assert_eq!(h.plot.window.span, 3600.0);
        let mut h = custom_open();
        h.frame(vec![egui::Event::Text("5h".into())]);
        h.button(MIDDLE, true);
        h.button(MIDDLE, false);
        assert!(!h.plot.custom.open);
        assert_eq!(h.plot.window.span, 3600.0);
    }

    #[test]
    fn coming_back_to_the_search_box_selects_its_text() {
        let pvs = PvManager::new();
        let mut h = Harness::new(Plot::new(&pvs, None));
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.key(egui::Key::Slash);
        h.frame(vec![egui::Event::Text("A:B".into())]);
        assert_eq!(search_selection(&h), Some((3, 3)));
        // Esc closes the list; a click in the box brings it back.
        let bx = Pos2::new(60.0, 14.0);
        h.key(egui::Key::Escape);
        assert!(search_focused(&h) && !h.plot.search.open);
        h.frame(vec![egui::Event::PointerMoved(bx)]);
        h.button(bx, true);
        h.button(bx, false);
        assert!(search_focused(&h) && h.plot.search.open);
        assert_eq!(h.plot.search.text, "A:B");
        // A click on the plot closes the list and leaves the text.
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.button(MIDDLE, true);
        h.button(MIDDLE, false);
        assert!(!search_focused(&h) && !h.plot.search.open);
        assert_eq!(h.plot.search.text, "A:B");
        // Clicking the box again selects it all, so typing replaces it.
        h.frame(vec![egui::Event::PointerMoved(bx)]);
        h.button(bx, true);
        h.button(bx, false);
        h.frame(vec![]);
        assert!(search_focused(&h) && h.plot.search.open);
        assert_eq!(search_selection(&h), Some((0, 3)));
        h.frame(vec![egui::Event::Text("C".into())]);
        assert_eq!(h.plot.search.text, "C");
    }

    #[test]
    fn a_recent_search_shows_while_typing_and_can_be_forgotten() {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        let mut hist = SearchHistory::default();
        hist.record("A:C", false);
        hist.record("A:B", false);
        p.set_search_history(hist);
        assert!(!p.search_history_changed(), "set by the host: not a change");
        let mut h = Harness::new(p);
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.key(egui::Key::Slash);
        h.frame(vec![egui::Event::Text("a:".into())]);
        assert_eq!(
            super::bar::rows_text(&h.plot),
            ["A:B", "A:C", "+  Add “a:” as typed"]
        );
        // The ✕ at the right of the first row, under the "Recent" heading.
        let popup = h
            .ctx
            .memory(|m| m.area_rect(h.plot.id.with("search-popup")))
            .expect("the popup");
        let margin = h.ctx.global_style().spacing.menu_margin;
        let x = Pos2::new(
            popup.right() - f32::from(margin.right) - 13.0,
            popup.top() + f32::from(margin.top) + 26.0,
        );
        h.frame(vec![egui::Event::PointerMoved(x)]);
        h.button(x, true);
        h.button(x, false);
        let left: Vec<&str> = h
            .plot
            .search_history()
            .searches()
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(left, ["A:C"]);
        assert!(h.plot.search_history_changed());
        assert_eq!(h.plot.search.text, "a:", "the box is left as it was");
        assert!(h.plot.names().is_empty());
    }

    #[test]
    fn unarchived_and_waveform_pvs_are_handled() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut p = Plot::new(&pvs, Some(client));
        p.add_pvs(&names(&[
            "NOT:ARCHIVED",
            "MOCK:WAVE",
            "sim://sine",
            "MOCK:ENUM",
        ]));
        assert!(p.is_loading(), "fetches due");
        let mut h = Harness::new(p);
        h.until(|p| !p.is_loading());
        let t = &h.plot.traces;
        assert!(t[0].fetch.not_archived);
        assert!(t[0].fetch.error.is_none());
        assert_eq!(t[1].fetch.raw_only, Some(true));
        assert!(t[1].history.as_ref().is_some_and(|h| !h.binned));
        // Archived, but an array: known from its metadata, its samples
        // neither fetched nor kept, and not plotted.
        let wave = t[1].history.as_ref().unwrap();
        assert_eq!(wave.kind, ValueKind::Waveform);
        assert!(wave.samples.is_empty());
        assert!(t[1].is_array());
        assert!(
            t[1].points(f64::NEG_INFINITY, f64::INFINITY, now())
                .is_empty()
        );
        assert!(t[2].history.is_none(), "no archive for sim://");
        // Enums are fetched raw: whole state indices, not bin means.
        assert_eq!(t[3].fetch.raw_only, Some(true));
        let enum_hist = t[3].history.as_ref().expect("enum history");
        assert!(!enum_hist.binned);
        assert!(enum_hist.samples.iter().all(|s| s.val.fract() == 0.0));
        // A PV that is not archived is no fault of the archiver's.
        let st = h.plot.archiver_status();
        assert_eq!(st.state, ArchiverState::Reachable);
        assert_eq!(st.last_error, None);
    }

    #[test]
    fn the_raw_history_hint_leaves_out_arrays() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut p = Plot::new(&pvs, Some(client));
        p.set_range(&view::Range::Relative {
            secs: 2.0 * 86_400.0,
        });
        p.add_pvs(&names(&["MOCK:WAVE"]));
        let mut h = Harness::new(p);
        h.until(|p| !p.is_loading());
        assert_eq!(h.plot.traces[0].fetch.raw_only, Some(true));
        assert!(
            hover::raw_capped(&h.plot).is_none(),
            "an array is not plotted"
        );
        // An enum is fetched raw and plotted: its history is cut.
        h.plot.add_pvs(&names(&["MOCK:ENUM"]));
        h.until(|p| !p.is_loading());
        let capped = hover::raw_capped(&h.plot).map(|t| t.label());
        assert_eq!(capped.as_deref(), Some("MOCK:ENUM"));
    }

    #[test]
    fn history_comes_even_when_get_metadata_is_a_404() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        mock.serve_metadata(false);
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut p = Plot::new(&pvs, Some(client));
        p.add_pvs(&names(&["MOCK:SINE"]));
        let mut h = Harness::new(p);
        h.until(|p| !p.is_loading());
        let t = &h.plot.traces[0];
        assert!(!t.fetch.not_archived);
        assert!(t.fetch.error.is_none());
        assert!(t.history.as_ref().is_some_and(|h| !h.samples.is_empty()));
    }

    #[test]
    fn a_wrong_archiver_url_is_an_error_not_live_only() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        // Every URL under this base is a 404.
        let client = ArchiverClient::new(&format!("{}/wrong", mock.url())).unwrap();
        let mut p = Plot::new(&pvs, Some(client));
        p.add_pvs(&names(&["MOCK:SINE"]));
        let mut h = Harness::new(p);
        h.until(|p| p.traces[0].fetch.error.is_some());
        let t = &h.plot.traces[0];
        assert!(!t.fetch.not_archived);
        assert!(t.fetch.error.as_ref().unwrap().contains("not an archiver"));
        assert_eq!(t.fetch.failures, 1);
        h.until(|p| !p.is_loading());
        let st = h.plot.archiver_status();
        assert_eq!(st.state, ArchiverState::Unreachable);
        let (e, _) = st.last_error.as_ref().unwrap();
        assert!(e.contains("not an archiver"), "{e}");
    }

    #[test]
    fn the_archiver_state_follows_the_archiver_down_and_up() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let addr = mock.addr();
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut p = Plot::new(&pvs, Some(client));
        p.monitor.reprobe = 0.2;
        p.add_pvs(&names(&["MOCK:SINE"]));
        assert_eq!(p.archiver_status().state, ArchiverState::Checking);
        assert!(p.is_loading(), "the probe is due");
        let mut h = Harness::new(p);
        // The start-up probe: reachable, with its version.
        h.until(|p| !p.is_loading());
        let st = h.plot.archiver_status().clone();
        assert_eq!(st.state, ArchiverState::Reachable);
        assert!(
            st.version.as_deref().unwrap_or("").contains("mock"),
            "{st:?}"
        );

        // Down: the next fetch fails, and that turns it unreachable.
        drop(mock);
        h.plot.traces[0].fetch.requested = None;
        h.until(|p| p.archiver_status().state == ArchiverState::Unreachable);
        assert!(h.plot.archiver_status().last_error.is_some());
        h.until(|p| !p.is_loading());
        assert!(h.plot.traces[0].fetch.error.is_some());
        // Still down: re-probed, still unreachable.
        let probed = h.plot.monitor.probed_at;
        h.until(|p| p.monitor.probed_at > probed && !p.is_loading());
        assert_eq!(h.plot.archiver_status().state, ArchiverState::Unreachable);

        // Back up on the same port: a re-probe finds it, and the trace
        // that failed asks again at once.
        let _mock = mockarchiver::MockArchiver::start(addr).unwrap();
        h.until(|p| p.archiver_status().state == ArchiverState::Reachable);
        h.until(|p| p.traces[0].fetch.error.is_none() && !p.is_loading());
        assert!(h.plot.traces[0].history.is_some());
    }

    #[test]
    fn an_archiver_that_cannot_be_reached_is_unreachable() {
        let pvs = PvManager::new();
        let client = ArchiverClient::new("http://127.0.0.1:1").unwrap();
        let mut h = Harness::new(Plot::new(&pvs, Some(client)));
        h.until(|p| !p.is_loading());
        assert_eq!(h.plot.archiver_status().state, ArchiverState::Unreachable);
        // None: nothing to reach.
        h.plot.set_archiver(None);
        assert_eq!(h.plot.archiver_status().state, ArchiverState::None);
        assert!(!h.plot.is_loading());
    }

    #[test]
    fn an_archiver_is_picked_in_the_pill() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let mut h = Harness::new(Plot::new(&pvs, None));
        assert_eq!(h.plot.archiver_status().state, ArchiverState::None);
        let pill = h.plot.edit.pill.expect("drawn").center();
        h.frame(vec![egui::Event::PointerMoved(pill)]);
        h.button(pill, true);
        h.button(pill, false);
        assert!(!h.plot.edit.open, "embedded: a status display only");
        h.plot.set_archiver_switching(true);
        h.button(pill, true);
        h.button(pill, false);
        assert!(h.plot.edit.open, "the popover opens");
        h.frame(vec![]);
        h.frame(vec![egui::Event::Text(mock.url())]);
        h.key(egui::Key::Enter);
        assert!(!h.plot.edit.open);
        let base = ArchiverClient::new(&mock.url()).unwrap().base().to_string();
        assert_eq!(h.plot.archiver().map(|a| a.base()), Some(base.as_str()));
        assert!(h.plot.take_archiver_change(), "the host is told");
        assert!(!h.plot.take_archiver_change(), "once");
        assert_eq!(h.plot.recent_archivers(), [base]);
        h.until(|p| p.archiver_status().state == ArchiverState::Reachable && !p.is_loading());

        // "Use" on the same archiver checks it again.
        let probed = h.plot.monitor.probed_at;
        h.button(pill, true);
        h.button(pill, false);
        assert!(h.plot.edit.open);
        h.frame(vec![]);
        h.key(egui::Key::Enter);
        assert!(!h.plot.take_archiver_change(), "no change");
        assert!(h.plot.monitor.probed_at > probed, "probed again");
    }

    #[test]
    fn switching_archivers_refetches_history() {
        let a = mockarchiver::MockArchiver::start_local().unwrap();
        let b = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, Some(ArchiverClient::new(&a.url()).unwrap()));
        p.add_pvs(&names(&["MOCK:SINE"]));
        let mut h = Harness::new(p);
        h.until(|p| !p.is_loading());
        assert!(h.plot.traces[0].history.is_some());
        h.plot.fetcher.sent.lock().clear();

        // As a view saved with archiver b does (once allowed to: an
        // embedded plot keeps its host's archiver).
        let mut v = h.plot.view();
        let a_url = v.archiver.clone();
        let b_url = ArchiverClient::new(&b.url()).unwrap().base().to_string();
        v.archiver = Some(b_url.clone());
        h.plot.set_view(&v);
        assert_eq!(h.plot.archiver().map(|a| a.base().to_string()), a_url);
        h.plot.set_archiver_switching(true);
        h.plot.set_view(&v);
        assert_eq!(h.plot.archiver().unwrap().base(), b_url);
        assert_eq!(h.plot.archiver_status().state, ArchiverState::Checking);
        assert!(h.plot.traces[0].history.is_none(), "a's history is gone");
        assert!(!h.plot.take_archiver_change(), "not the user's pick");
        h.until(|p| !p.is_loading());
        assert_eq!(h.plot.fetcher.sent.lock().len(), 1, "fetched again");
        assert!(h.plot.traces[0].history.is_some());
        assert_eq!(h.plot.archiver_status().state, ArchiverState::Reachable);
        assert_eq!(h.plot.view().archiver, Some(b_url.clone()));
        // Both are offered in the pill, newest first.
        assert_eq!(h.plot.recent_archivers()[0], b_url);
        assert_eq!(h.plot.recent_archivers().len(), 2);
        // A view without an archiver leaves it be.
        v.archiver = None;
        h.plot.set_view(&v);
        assert_eq!(h.plot.archiver().unwrap().base(), b_url);
    }

    #[test]
    fn a_name_with_regex_characters_is_not_archived_not_a_bad_url() {
        // getData says 404 (or 400 where the name looks like an operator
        // call); a naive glob search for the name would be a server error
        // (an unbalanced `{`), but the exact search is not, so neither is
        // taken for a wrong archiver URL.
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut p = Plot::new(&pvs, Some(client));
        p.add_pvs(&names(&[
            "XF:23ID1-ES{Dif}Mtr",
            "XF:23ID1-ES{Dif-Ax:TH(1)}+",
        ]));
        let mut h = Harness::new(p);
        h.until(|p| !p.is_loading());
        let t = &h.plot.traces;
        assert!(t[0].fetch.not_archived, "{:?}", t[0].fetch.error);
        assert_eq!(t[0].fetch.error, None);
        let error = t[1].fetch.error.as_deref().unwrap_or("");
        assert!(!error.contains("not an archiver"), "{error}");
    }

    /// A live plot of MOCK:SINE over two minutes: few enough samples that
    /// `optimized_N` answers raw.
    fn live_raw_sine() -> (mockarchiver::MockArchiver, Harness) {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut p = Plot::new(&pvs, Some(client));
        p.set_range(&view::Range::Relative { secs: 120.0 });
        p.add_pvs(&names(&["MOCK:SINE"]));
        let mut h = Harness::new(p);
        h.until(|p| !p.is_loading());
        let t = &h.plot.traces[0];
        assert!(matches!(
            t.fetch.requested.map(|r| r.op),
            Some(Operator::Optimized(_))
        ));
        assert!(!t.history.as_ref().expect("history").binned);
        (mock, h)
    }

    /// Make trace 0's last request old enough for a live refresh.
    fn age_request(h: &mut Harness) {
        if let Some(r) = &mut h.plot.traces[0].fetch.requested {
            r.at -= LIVE_REFRESH + 1.0;
        }
    }

    #[test]
    fn live_raw_history_is_refreshed_by_its_tail() {
        let (_mock, mut h) = live_raw_sine();
        let r0 = h.plot.traces[0].fetch.requested.unwrap();
        let last = h.plot.traces[0]
            .history
            .as_ref()
            .unwrap()
            .samples
            .last()
            .unwrap()
            .t;
        // Let the archive move on a little.
        std::thread::sleep(std::time::Duration::from_millis(1200));
        h.plot.fetcher.sent.lock().clear();
        age_request(&mut h);
        h.frame(vec![]);
        let sent = h.plot.fetcher.sent.lock().clone();
        assert_eq!(sent.len(), 1, "{sent:?}");
        let (_, from, to, append) = sent[0];
        assert!(append, "only the tail");
        assert_eq!(from, last, "from the last sample on");
        assert!(to > r0.to);
        h.until(|p| !p.is_loading());

        let t = &h.plot.traces[0];
        let r = t.fetch.requested.unwrap();
        assert_eq!(r.op, r0.op);
        assert_eq!(r.to, to, "the window fetched now ends later");
        assert!(r.from >= r0.from && r.from <= last);
        let hist = t.history.as_ref().unwrap();
        assert!(!hist.binned);
        assert!(hist.samples.last().unwrap().t > last, "new samples added");
        assert!(
            hist.samples.windows(2).all(|w| w[0].t < w[1].t),
            "no sample twice"
        );
        // The head is what holds at the window's start, and no older.
        assert!(hist.samples[0].t <= r.from && hist.samples[1].t >= r.from);
    }

    #[test]
    fn raw_history_grown_past_n_is_fetched_again_whole() {
        let (_mock, mut h) = live_raw_sine();
        // As if `optimized_10` had answered raw: the tail now outgrows it.
        if let Some(r) = &mut h.plot.traces[0].fetch.requested {
            r.op = Operator::Optimized(10);
        }
        h.plot.fetcher.sent.lock().clear();
        age_request(&mut h);
        h.until(|p| p.fetcher.sent.lock().len() >= 2 && !p.is_loading());
        let sent = h.plot.fetcher.sent.lock().clone();
        assert!(sent[0].3, "first the tail");
        assert!(!sent[1].3, "then the whole window");
        let r = h.plot.traces[0].fetch.requested.unwrap();
        assert!(matches!(r.op, Operator::Optimized(n) if n > 10));
    }

    #[test]
    fn binned_history_is_refetched_whole_at_its_own_pace() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut p = Plot::new(&pvs, Some(client));
        p.set_range(&view::Range::Relative { secs: 86_400.0 });
        p.add_pvs(&names(&["MOCK:SINE"]));
        let mut h = Harness::new(p);
        h.until(|p| !p.is_loading());
        assert!(h.plot.traces[0].history.as_ref().unwrap().binned);
        h.plot.fetcher.sent.lock().clear();
        // 30 s on: a day's bins are a minute wide, so not yet.
        age_request(&mut h);
        h.frame(vec![]);
        assert!(h.plot.fetcher.sent.lock().is_empty());
        // Past three bin widths: the whole window again, binned.
        if let Some(r) = &mut h.plot.traces[0].fetch.requested {
            r.at -= 300.0;
        }
        h.frame(vec![]);
        let sent = h.plot.fetcher.sent.lock().clone();
        assert_eq!(sent.len(), 1);
        assert!(!sent[0].3, "bins are never appended to");
    }

    #[test]
    fn a_trace_has_one_request_in_flight() {
        // An archiver that answers nothing for 1.5 s, then hangs up.
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.local_addr().unwrap());
        std::thread::spawn(move || {
            for conn in server.incoming().flatten() {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    drop(conn);
                });
            }
        });
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, Some(ArchiverClient::new(&url).unwrap()));
        // No probe: only the data requests are counted.
        p.monitor.due = false;
        p.add_pvs(&names(&["A:B"]));
        let mut h = Harness::new(p);
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.until(|p| p.fetcher.busy());
        for _ in 0..3 {
            // Out of the window fetched, and still past the debounce.
            for _ in 0..4 {
                h.key(egui::Key::ArrowLeft);
            }
            let start = std::time::Instant::now();
            while start.elapsed().as_millis() < 350 {
                std::thread::sleep(std::time::Duration::from_millis(20));
                h.frame(vec![]);
                assert_eq!(h.plot.fetcher.in_flight(), 1);
            }
        }
    }

    /// A plot of `traces` (name, units, values), each given its units
    /// as archived, and its points.
    fn plot_of(traces: &[(&str, &str, &[f64])]) -> (Plot, Vec<Vec<Pt>>) {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        let mut data = Vec::new();
        for (name, egu, values) in traces {
            p.add_pvs(&names(&[name]));
            let t = p.traces.last_mut().unwrap();
            t.history = Some(History {
                meta: archiver::Meta {
                    egu: Some(egu.to_string()).filter(|u| !u.is_empty()),
                    ..Default::default()
                },
                // As archived: no wait for more live data.
                samples: values
                    .iter()
                    .enumerate()
                    .map(|(i, v)| archiver::Sample::raw(i as f64, *v))
                    .collect(),
                ..Default::default()
            });
            data.push(
                values
                    .iter()
                    .enumerate()
                    .map(|(i, v)| Pt::raw(i as f64, *v))
                    .collect(),
            );
        }
        (p, data)
    }

    /// An archived history of `values` in `egu`.
    fn hist_of(egu: &str, values: &[f64]) -> Option<History> {
        Some(History {
            meta: archiver::Meta {
                egu: Some(egu.to_string()),
                ..Default::default()
            },
            samples: values
                .iter()
                .enumerate()
                .map(|(i, v)| archiver::Sample::raw(i as f64, *v))
                .collect(),
            ..Default::default()
        })
    }

    /// Each trace's axis, as an index into the plot's axes.
    fn axis_indices(p: &Plot) -> Vec<Option<usize>> {
        p.traces
            .iter()
            .map(|t| t.axis.and_then(|id| p.all_axes().position(|a| a.id == id)))
            .collect()
    }

    /// Each trace's pane, as an index.
    fn pane_indices(p: &Plot) -> Vec<Option<usize>> {
        (p.traces.iter())
            .map(|t| t.pane.and_then(|id| p.pane_index(id)))
            .collect()
    }

    /// Every axis' name, pane by pane.
    fn all_names(p: &Plot) -> Vec<String> {
        (0..p.panes.len())
            .flat_map(|k| p.name_axes(k))
            .map(|(_, n)| n)
            .collect()
    }

    #[test]
    fn traces_settle_on_axes_once_and_name_them() {
        let (mut p, mut data) = plot_of(&[
            ("SR-DI-DCCT-01:SIGNAL", "mA", &[299.5, 300.0, 0.0, 299.8]),
            ("SR-DI-DCCT-01:LIFETIME", "h", &[5.2, 11.9, 12.7]),
            ("SR01C-DI-EBPM-01:SA:X", "mm", &[0.0, 0.026, 0.04]),
            ("SR01C-DI-EBPM-01:SA:Y", "mm", &[0.0, 0.58, 0.6]),
            ("VAC:P", "mbar", &[]),
        ]);
        p.settle_axes(&data, 0.0);
        assert_eq!(
            axis_indices(&p),
            [Some(0), Some(1), Some(2), Some(2), None],
            "no data, no axis yet"
        );
        let names: Vec<String> = p.name_axes(0).into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, ["Current (mA)", "Time (h)", "Position (mm)"]);
        // Pressure, live only (no history): an axis of its own, on a log
        // scale, once it has been live for a second.
        data[4] = (0..50)
            .map(|i| Pt::raw(i as f64, 1e-10 * 1.2f64.powi(i)))
            .collect();
        p.settle_axes(&data, 1.0);
        assert_eq!(axis_indices(&p)[4], None, "one live sample does not decide");
        p.settle_axes(&data, 1.0 + LIVE_WAIT);
        assert_eq!(axis_indices(&p)[4], Some(3));
        assert!(p.panes[0].axes[3].log && !p.panes[0].axes[0].log);
        // Decisions stick, whatever the data does next.
        data[1] = data[0].clone();
        p.settle_axes(&data, 3.0);
        assert_eq!(
            axis_indices(&p),
            [Some(0), Some(1), Some(2), Some(2), Some(3)]
        );
        // Moved by hand to its own axis, then back: the empty axis goes.
        let y = p.traces[3].id;
        p.move_by_hand(y, None);
        assert_eq!(p.panes[0].axes.len(), 5);
        let names: Vec<String> = p.name_axes(0).into_iter().map(|(_, n)| n).collect();
        assert_eq!(names[2], "Position X (mm)");
        assert_eq!(names[3], "Position Y (mm)", "in the legend's order");
        let x_axis = p.traces[2].axis;
        p.move_by_hand(y, x_axis);
        assert_eq!(p.panes[0].axes.len(), 4);
        // Removing a trace removes its axis with it.
        p.remove_pv("SR-DI-DCCT-01:LIFETIME");
        assert_eq!(p.panes[0].axes.len(), 3);
        assert_eq!(axis_indices(&p), [Some(0), Some(1), Some(1), Some(2)]);
    }

    #[test]
    fn an_old_views_axis_is_kept_for_its_first_pv() {
        let (mut p, mut data) =
            plot_of(&[("SIG", "mA", &[300.0, 290.0]), ("LT", "h", &[10.0, 12.0])]);
        // A file from before there were several axes: no trace axes.
        let mut v = p.view();
        v.y = YAxis {
            autoscale: false,
            min: Some(1.0),
            max: Some(300.0),
            log: true,
            name: None,
        };
        let hist: Vec<Option<History>> = p.traces.iter().map(|t| t.history.clone()).collect();
        p.set_view(&v);
        for (t, h) in p.traces.iter_mut().zip(hist) {
            t.history = h;
        }
        assert_eq!(axis_indices(&p), [None, None]);
        // The lifetime's data comes first, but the fixed 1..300 is SIG's.
        let sig = std::mem::take(&mut data[0]);
        p.settle_axes(&data, 0.0);
        data[0] = sig;
        p.settle_axes(&data, 1.0);
        let axis = |i: usize| {
            let id = p.traces[i].axis.unwrap();
            p.panes[0].axes.iter().find(|a| a.id == id).unwrap()
        };
        let a = axis(0);
        assert!(
            a.log && !a.autoscale && a.hi == 300.0,
            "the view's settings"
        );
        let b = axis(1);
        assert!(b.autoscale, "the lifetime's own range");
        assert!(b.log, "the old file's log scale, for every axis");
        assert_eq!(axis_indices(&p), [Some(0), Some(1)], "in legend order");
    }

    #[test]
    fn settling_does_not_depend_on_the_order_answers_come_in() {
        let pvs: [(&str, &str, &[f64]); 6] = [
            ("SR-DI-DCCT-01:SIGNAL", "mA", &[299.6, 299.8, 0.0]),
            ("SR-DI-DCCT-01:LIFETIME", "h", &[5.2, 11.9, 12.7]),
            ("BR01C-DI-DCCT-01:CHARGE", "nC", &[0.0013, 0.0068, 0.01]),
            ("LB-DI-ICT-01:SIGNAL", "nC", &[-0.029, 0.029, 0.043]),
            ("CS-DI-XFER-01:BR-SR", "%", &[0.0, 16.2, 19.2]),
            ("SR01A-VA-GAUGE-01:P", "mbar", &[2e-10, 2.1e-10, 1.3e-8]),
        ];
        let settle = |order: &[usize]| {
            let (mut p, all) = plot_of(&pvs);
            // Archived PVs, their answers not yet in.
            p.archiver = Some(ArchiverClient::new("http://127.0.0.1:9").unwrap());
            let hist: Vec<Option<History>> =
                p.traces.iter_mut().map(|t| t.history.take()).collect();
            let mut data = vec![Vec::new(); pvs.len()];
            for (step, &i) in order.iter().enumerate() {
                p.traces[i].history = hist[i].clone();
                data[i] = all[i].clone();
                p.settle_axes(&data, step as f64 * 0.1);
            }
            assert!(p.traces.iter().all(|t| t.axis.is_some()), "{order:?}");
            (axis_indices(&p), all_names(&p), pane_indices(&p))
        };
        let first = settle(&[0, 1, 2, 3, 4, 5]);
        assert_eq!(
            first.0,
            [Some(0), Some(1), Some(2), Some(2), Some(3), Some(4)]
        );
        // Four axes fill the first pane: the gauge has a pane of its own.
        assert_eq!(
            first.2,
            [Some(0), Some(0), Some(0), Some(0), Some(0), Some(1)]
        );
        assert_eq!(
            first.1,
            [
                "Current (mA)",
                "Time (h)",
                "Charge (nC)",
                "(%)",
                "Pressure (mbar)"
            ]
        );
        for order in [[5, 4, 3, 2, 1, 0], [1, 3, 5, 0, 2, 4], [2, 0, 5, 1, 4, 3]] {
            assert_eq!(settle(&order), first, "{order:?}");
        }
    }

    #[test]
    fn an_answer_that_never_comes_holds_the_others_back_only_so_long() {
        let (mut p, data) = plot_of(&[("A", "mA", &[]), ("B", "h", &[10.0])]);
        p.archiver = Some(ArchiverClient::new("http://127.0.0.1:9").unwrap());
        p.traces[0].history = None;
        p.settle_axes(&data, 0.0);
        assert_eq!(axis_indices(&p), [None, None], "B waits for A");
        p.settle_axes(&data, SETTLE_WAIT);
        assert_eq!(axis_indices(&p), [None, Some(0)]);
    }

    #[test]
    fn a_trace_alone_dropped_on_the_plot_keeps_its_axis() {
        let (mut p, data) = plot_of(&[("A", "mbar", &[1e-10, 1e-8, 1e-6])]);
        p.settle_axes(&data, 0.0);
        assert!(p.panes[0].axes[0].log);
        p.panes[0].axes[0].name = Some("Vac".into());
        let id = p.traces[0].id;
        p.move_by_hand(id, None);
        assert_eq!(p.panes[0].axes.len(), 1);
        assert!(p.panes[0].axes[0].log);
        assert_eq!(p.panes[0].axes[0].name.as_deref(), Some("Vac"));
    }

    #[test]
    fn hidden_traces_still_count_for_their_axes() {
        let (mut p, mut data) = plot_of(&[
            ("A", "mA", &[300.0]),
            ("B", "h", &[10.0]),
            ("C", "nC", &[0.01]),
            ("SR01C-DI-EBPM-01:SA:X", "mm", &[0.04]),
            ("SR01C-DI-EBPM-01:SA:Y", "mm", &[0.6]),
            ("E", "mm", &[50.0, 90.0]),
        ]);
        let later: Vec<Vec<Pt>> = data.drain(4..).collect();
        data.extend([Vec::new(), Vec::new()]);
        p.settle_axes(&data, 0.0);
        assert_eq!(p.panes[0].axes.len(), 4);
        // Hide SA:X: SA:Y still joins it, and the four axes are still
        // four, so E (mm, but far bigger) needs a pane of its own.
        p.traces[3].visible = false;
        data[3] = Vec::new();
        data[4] = later[0].clone();
        data[5] = later[1].clone();
        p.settle_axes(&data, 1.0);
        assert_eq!(p.panes[0].axes.len(), 4);
        assert_eq!(p.traces[4].axis, p.traces[3].axis);
        assert_eq!(p.panes.len(), 2);
        assert_eq!(pane_indices(&p)[5], Some(1));
    }

    #[test]
    fn switching_archivers_keeps_the_axes() {
        let (mut p, data) = plot_of(&[("A", "mA", &[300.0]), ("B", "h", &[10.0])]);
        p.settle_axes(&data, 0.0);
        let before = axis_indices(&p);
        p.set_archiver(Some(ArchiverClient::new("http://127.0.0.1:9").unwrap()));
        p.settle_axes(&[Vec::new(), Vec::new()], 1.0);
        assert_eq!(axis_indices(&p), before);
        assert_eq!(p.panes[0].axes.len(), 2);
    }

    #[test]
    fn an_old_views_axis_goes_with_its_pv() {
        // The reviewer's probe: SIG is removed before it settles.
        let (mut p, data) = plot_of(&[("SIG", "mA", &[300.0, 290.0]), ("LT", "h", &[10.0, 12.0])]);
        let mut v = p.view();
        v.y = YAxis {
            autoscale: false,
            min: Some(1.0),
            max: Some(300.0),
            log: false,
            name: None,
        };
        let hist: Vec<Option<History>> = p.traces.iter().map(|t| t.history.clone()).collect();
        p.set_view(&v);
        for (t, h) in p.traces.iter_mut().zip(hist) {
            t.history = h;
        }
        // Saved before anything settles, the view keeps its `y`.
        assert_eq!(p.view().y, v.y);
        p.remove_pv("SIG");
        p.settle_axes(&data[1..], 0.0);
        assert_eq!(p.panes[0].axes.len(), 1, "no axis left for SIG");
        assert!(p.autoscaling());
        let saved = p.view();
        assert!(saved.y.autoscale && saved.axes.is_empty(), "{saved:?}");
        // Reloaded, a new PV gets an axis of its own, autoscaled.
        p.set_view(&saved);
        p.traces[0].history = hist_of("h", &[10.0, 12.0]);
        p.add_pvs(&names(&["V1"]));
        p.traces[1].history = hist_of("V", &[0.001, 0.002]);
        let volts = vec![Pt::raw(0.0, 0.001), Pt::raw(1.0, 0.002)];
        p.settle_axes(&[Vec::new(), volts], 0.0);
        let id = p.traces[1].axis.unwrap();
        assert!(
            p.panes[0]
                .axes
                .iter()
                .find(|a| a.id == id)
                .unwrap()
                .autoscale
        );

        // A reserved PV that never gets data frees its axis after the wait.
        let (mut p, data) = plot_of(&[("DEAD", "mA", &[]), ("LT", "h", &[10.0])]);
        let hist: Vec<Option<History>> = p.traces.iter().map(|t| t.history.clone()).collect();
        p.set_view(&v);
        for (t, h) in p.traces.iter_mut().zip(hist) {
            t.history = h;
        }
        p.settle_axes(&data, 0.0);
        p.settle_axes(&data, SETTLE_WAIT);
        assert_eq!(p.panes[0].axes.len(), 1);
        assert!(p.autoscaling() && p.view().axes.is_empty());
    }

    #[test]
    fn turning_log_off_turns_it_off_for_new_axes() {
        let (mut p, data) = plot_of(&[("A", "mA", &[300.0]), ("B", "h", &[10.0])]);
        let mut v = p.view();
        v.y.log = true;
        let hist: Vec<Option<History>> = p.traces.iter().map(|t| t.history.clone()).collect();
        p.set_view(&v);
        for (t, h) in p.traces.iter_mut().zip(hist) {
            t.history = h;
        }
        p.set_log(false);
        p.settle_axes(&data, 0.0);
        assert!(p.panes[0].axes.iter().all(|a| !a.log));
    }

    #[test]
    fn an_unreachable_archiver_owes_no_history() {
        let (mut p, _) = plot_of(&[("A", "mA", &[300.0])]);
        p.archiver = Some(ArchiverClient::new("http://127.0.0.1:9").unwrap());
        p.traces[0].history = None;
        assert!(p.awaits_history(&p.traces[0]));
        p.monitor.status.state = ArchiverState::Unreachable;
        assert!(!p.awaits_history(&p.traces[0]), "live data settles it");
    }

    #[test]
    fn a_zero_fits_no_scale() {
        let (mut p, data) = plot_of(&[("A", "", &[300.0]), ("B", "", &[0.0])]);
        // No units archived: they wait (up to the limit) for live units.
        p.settle_axes(&data, 0.0);
        assert_eq!(axis_indices(&p), [None, None]);
        p.settle_axes(&data, SETTLE_WAIT);
        assert_eq!(axis_indices(&p), [Some(0), Some(1)]);
    }

    #[test]
    fn traces_on_the_mock_settle_and_the_view_keeps_them() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut p = Plot::new(&pvs, Some(client));
        p.add_pvs(&names(&[
            "MOCK:SINE",
            "MOCK:NOISE",
            "BL01T-MO-SIMC-01:M1.RBV",
            "BL01T-MO-SIMC-01:M2.RBV",
            "MOCK:ENUM",
        ]));
        let mut h = Harness::new(p);
        h.until(|p| !p.is_loading() && p.traces.iter().all(|t| t.axis.is_some()));
        assert_eq!(
            axis_indices(&h.plot),
            [Some(0), Some(1), Some(2), Some(2), Some(3)]
        );
        h.frame(vec![]);
        assert_eq!(h.plot.panes[0].drawn.len(), 4);
        let names: Vec<String> = h.plot.panes[0]
            .axis_names
            .iter()
            .map(|(_, n)| n.clone())
            .collect();
        assert_eq!(
            names,
            ["Current (mA)", "Voltage (V)", "Position (mm)", "MOCK:ENUM"]
        );
        let v = h.plot.view();
        assert_eq!(v.axes.len(), 3);
        assert_eq!(
            v.pvs.iter().map(|t| t.axis).collect::<Vec<_>>(),
            [Some(0), Some(1), Some(2), Some(2), Some(3)]
        );
        // Shift+wheel over the second axis (right of the data) zooms it
        // alone.
        let edge = Pos2::new(985.0, 300.0);
        h.frame(vec![egui::Event::PointerMoved(edge)]);
        let shift = egui::Modifiers::SHIFT;
        h.mods = shift;
        h.frame(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, 100.0),
            phase: egui::TouchPhase::Move,
            modifiers: shift,
        }]);
        for _ in 0..20 {
            h.frame(vec![]);
        }
        let auto: Vec<bool> = h.plot.panes[0].axes.iter().map(|a| a.autoscale).collect();
        assert_eq!(auto, [true, true, true, false], "the outer right axis");
        // ... and over the data, every axis.
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.frame(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, 100.0),
            phase: egui::TouchPhase::Move,
            modifiers: shift,
        }]);
        for _ in 0..20 {
            h.frame(vec![]);
        }
        assert!(h.plot.panes[0].axes.iter().all(|a| !a.autoscale));
        h.mods = egui::Modifiers::NONE;
        h.key(egui::Key::A);
        assert!(h.plot.autoscaling());
    }

    #[test]
    fn axes_are_renamed_picked_out_and_given_traces_by_hand() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut p = Plot::new(&pvs, Some(client));
        p.add_pvs(&names(&["MOCK:SINE", "MOCK:NOISE"]));
        let mut h = Harness::new(p);
        h.until(|p| !p.is_loading() && p.traces.iter().all(|t| t.axis.is_some()));
        h.frame(vec![]);
        assert_eq!(h.plot.seen.0.len(), 2);
        let click = |h: &mut Harness, at: Pos2| {
            h.frame(vec![egui::Event::PointerMoved(at)]);
            h.button(at, true);
            h.button(at, false);
        };

        // Double-click the first axis' name, type, Enter: renamed.
        let name = h.plot.seen.0[0].1.center();
        click(&mut h, name);
        click(&mut h, name);
        assert!(h.plot.renaming.is_some());
        assert_eq!(
            h.plot.panes[0].axis_highlight, None,
            "renaming picks nothing out"
        );
        // The name so far is selected, to type over.
        h.frame(vec![]);
        h.frame(vec![egui::Event::Text("Beam".into())]);
        h.key(egui::Key::Enter);
        assert!(h.plot.renaming.is_none());
        assert_eq!(h.plot.panes[0].axes[0].name.as_deref(), Some("Beam"));
        assert_eq!(h.plot.view().y.name.as_deref(), Some("Beam"));

        // Click the second axis: its trace is picked out; again, not.
        let second = h.plot.seen.0[1].0.center();
        click(&mut h, second);
        assert_eq!(
            h.plot.panes[0].axis_highlight,
            Some(h.plot.panes[0].axes[1].id)
        );
        h.frame(vec![]);
        click(&mut h, second);
        assert_eq!(h.plot.panes[0].axis_highlight, None);
        // Picked out, then its trace hidden: the axis goes, and so does
        // the dimming.
        click(&mut h, second);
        assert!(h.plot.panes[0].axis_highlight.is_some());
        h.plot.traces[1].visible = false;
        h.frame(vec![]);
        assert_eq!(h.plot.panes[0].axis_highlight, None);
        h.plot.traces[1].visible = true;
        h.frame(vec![]);

        // Drag the second chip onto the first axis: one axis left.
        h.frame(vec![]);
        let chip = h.plot.seen.1[1].0.center();
        let onto = h.plot.seen.0[0].0.center();
        h.frame(vec![egui::Event::PointerMoved(chip)]);
        h.button(chip, true);
        for i in 1..=10 {
            let at = chip + (onto - chip) * (i as f32 / 10.0);
            h.frame(vec![egui::Event::PointerMoved(at)]);
        }
        assert!(h.plot.chip_drag.is_some());
        h.button(onto, false);
        h.frame(vec![]);
        assert!(h.plot.chip_drag.is_none());
        assert_eq!(h.plot.panes[0].axes.len(), 1);
        assert_eq!(h.plot.traces[1].axis, h.plot.traces[0].axis);

        // Its dot's menu, "New axis": back on an axis of its own.
        let dot = h.plot.seen.1[1].1.center();
        click(&mut h, dot);
        h.frame(vec![]);
        let open = egui::Popup::is_any_open(&h.ctx);
        assert!(open, "the dot opens its menu");
        h.plot.move_by_hand(h.plot.traces[1].id, None);
        assert_eq!(h.plot.panes[0].axes.len(), 2);
    }

    #[test]
    fn log_scale_leaves_a_fixed_range_through_zero() {
        let mut a = ValueAxis::new(0);
        a.autoscale = false;
        (a.lo, a.hi) = (0.0, 100.0);
        a.log = true;
        a.fit_log_range();
        assert!(a.autoscale);
        (a.autoscale, a.lo) = (false, 1.0);
        a.fit_log_range();
        assert!(!a.autoscale, "a positive range stays");
    }

    #[test]
    fn a_new_archiver_keeps_counting_generations() {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        p.add_pvs(&names(&["A:B"]));
        p.traces[0].fetch.generation = 3;
        p.set_archiver(None);
        assert_eq!(p.traces[0].fetch.generation, 4, "old answers stay stale");
        assert!(p.traces[0].fetch.requested.is_none());
    }

    /// Press and release `button` at `at`, `moves` on the way.
    fn click_by(h: &mut Harness, button: egui::PointerButton, at: Pos2, moves: &[Pos2]) {
        let ev = |pos, pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.frame(vec![egui::Event::PointerMoved(at), ev(at, true)]);
        let mut last = at;
        for &p in moves {
            h.frame(vec![egui::Event::PointerMoved(p)]);
            last = p;
        }
        h.frame(vec![ev(last, false)]);
    }

    #[test]
    fn middle_click_on_the_plot_or_legend_asks_for_the_selection() {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        p.add_pvs(&names(&["sim://sine"]));
        let mut h = Harness::new(p);
        h.frame(vec![]);
        let middle = egui::PointerButton::Middle;
        click_by(&mut h, middle, MIDDLE, &[]);
        assert_eq!(h.plot.take_middle_click(), Some(MiddleClick::Plot));
        assert_eq!(h.plot.take_middle_click(), None, "taken once");
        assert!(h.plot.window.live, "a click is no pan");
        // On the legend, between the chips too.
        let chip = h.plot.chips_shown[0].1;
        click_by(&mut h, middle, chip.center(), &[]);
        assert_eq!(h.plot.take_middle_click(), Some(MiddleClick::Plot));
        click_by(&mut h, middle, Pos2::new(900.0, chip.center().y), &[]);
        assert_eq!(h.plot.take_middle_click(), Some(MiddleClick::Plot));
        // A middle drag pans as before and asks for nothing; a primary
        // click asks for nothing either.
        let moves = [Pos2::new(560.0, 300.0), Pos2::new(700.0, 300.0)];
        click_by(&mut h, middle, MIDDLE, &moves);
        assert_eq!(h.plot.take_middle_click(), None);
        assert!(!h.plot.window.live, "dragged: back in time");
        click_by(&mut h, egui::PointerButton::Primary, MIDDLE, &[]);
        assert_eq!(h.plot.take_middle_click(), None);
        // Nor does one on the top bar.
        click_by(&mut h, middle, Pos2::new(990.0, 5.0), &[]);
        assert_ne!(h.plot.take_middle_click(), Some(MiddleClick::Plot));
    }

    #[test]
    fn pasted_text_adds_the_pvs_it_names() {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        p.add_pvs(&names(&["A:B"]));
        let pasted = p.paste_pvs("A:B, C:D\n12:30:00 pva://E:F ca://C:D");
        assert_eq!(pasted.added, ["C:D", "pva://E:F"]);
        assert_eq!(pasted.present, ["A:B"]);
        assert_eq!(p.take_pasted(), Some(pasted));
        assert_eq!(p.names(), ["A:B", "C:D", "pva://E:F"]);
        let none = p.paste_pvs("42, 12:30:00");
        assert!(none.added.is_empty() && none.text == "42, 12:30:00");
        // A screenful adds nothing.
        let screen: Vec<String> = (0..30).map(|i| format!("W{i}")).collect();
        let many = p.paste_pvs(&screen.join(" "));
        assert_eq!((many.too_many, many.added.len()), (30, 0));
        assert_eq!(p.names().len(), 3);
        // MAX_PASTE of them is fine.
        let twenty = p.paste_pvs(&screen[..MAX_PASTE].join(" "));
        assert_eq!(twenty.added.len(), MAX_PASTE);

        // Ctrl+V over the plot: egui's paste event.
        let mut h = Harness::new(p);
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.plot.take_pasted();
        h.frame(vec![egui::Event::Paste("G:H".into())]);
        assert_eq!(h.plot.names().last().map(String::as_str), Some("G:H"));
        assert_eq!(h.plot.take_pasted().unwrap().message(), "Added G:H");
        // Not while the search box has the keyboard: it is pasted there.
        h.key(egui::Key::Slash);
        h.frame(vec![egui::Event::Paste("I:J".into())]);
        assert_eq!(h.plot.names().len(), 4 + MAX_PASTE);
        assert_eq!(h.plot.search.text(), "I:J");
    }

    #[test]
    fn middle_click_pastes_into_the_search_box_and_selecting_reports() {
        let pvs = PvManager::new();
        let mut h = Harness::new(Plot::new(&pvs, None));
        h.frame(vec![]);
        // The box is on the left of the top bar.
        let in_box = Pos2::new(80.0, 12.0);
        click_by(&mut h, egui::PointerButton::Middle, in_box, &[]);
        assert_eq!(h.plot.take_middle_click(), Some(MiddleClick::Search));
        h.plot.paste_into_search(" SR-DI-DCCT-01:SIGNAL\n");
        h.frame(vec![]);
        assert_eq!(h.plot.search.text(), "SR-DI-DCCT-01:SIGNAL");
        // At the cursor, which is after what was pasted.
        h.plot.paste_into_search("X");
        h.frame(vec![]);
        assert_eq!(h.plot.search.text(), "SR-DI-DCCT-01:SIGNALX");
        assert!(h.ctx.memory(|m| m.has_focus(h.plot.id.with("search"))));

        // Selecting all of it (as a drag or a double-click would) reports
        // it once, for the primary selection.
        h.plot.take_search_selection();
        let select_all = egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        h.frame(vec![select_all]);
        h.frame(vec![]);
        assert_eq!(
            h.plot.take_search_selection().as_deref(),
            Some("SR-DI-DCCT-01:SIGNALX")
        );
        h.frame(vec![]);
        assert_eq!(h.plot.take_search_selection(), None);

        // Back in the box from elsewhere, a middle-click pastes where it
        // lands (here past the end), not over the whole text as a primary
        // click into the box selects it.
        click_by(&mut h, egui::PointerButton::Primary, MIDDLE, &[]);
        assert!(!h.ctx.memory(|m| m.has_focus(h.plot.id.with("search"))));
        click_by(
            &mut h,
            egui::PointerButton::Middle,
            Pos2::new(300.0, 12.0),
            &[],
        );
        assert_eq!(h.plot.take_middle_click(), Some(MiddleClick::Search));
        assert_eq!(h.plot.take_search_selection(), None);
        h.plot.paste_into_search("Y");
        h.frame(vec![]);
        assert_eq!(h.plot.search.text(), "SR-DI-DCCT-01:SIGNALXY");
        // A primary click into the box selects all of it, but that is
        // not the user's selection: the primary selection keeps theirs.
        click_by(&mut h, egui::PointerButton::Primary, MIDDLE, &[]);
        h.plot.take_search_selection();
        click_by(
            &mut h,
            egui::PointerButton::Primary,
            Pos2::new(80.0, 12.0),
            &[],
        );
        h.frame(vec![]);
        assert!(h.ctx.memory(|m| m.has_focus(h.plot.id.with("search"))));
        assert_eq!(h.plot.take_search_selection(), None);
        // Selecting the same text on purpose then is reported.
        let select_all = egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        h.frame(vec![select_all]);
        h.frame(vec![]);
        assert_eq!(
            h.plot.take_search_selection().as_deref(),
            Some("SR-DI-DCCT-01:SIGNALXY")
        );
        // A middle-click on the ✕ pastes nothing.
        let cross = h.plot.paste.clear_rect.unwrap().center();
        click_by(&mut h, egui::PointerButton::Middle, cross, &[]);
        assert_eq!(h.plot.take_middle_click(), None);
        assert_eq!(h.plot.search.text(), "SR-DI-DCCT-01:SIGNALXY");
    }

    #[test]
    fn values_format_compactly() {
        assert_eq!(format_value(300.12345, Some(3), "mA"), "300.123 mA");
        assert_eq!(format_value(2.5, None, ""), "2.5");
        assert_eq!(format_value(-0.00001, None, ""), "-1.000e-5");
        assert_eq!(format_value(42.0, None, "V"), "42 V");
        assert_eq!(format_value(-0.04, Some(1), "counts"), "0.0 counts");
        assert_eq!(format_value(-0.4, Some(1), ""), "-0.4");
        assert_eq!(format_value(f64::NAN, None, "V"), "—");
        assert_eq!(format_value(1468514835.908, Some(3), "Pa"), "1.469e9 Pa");
        assert_eq!(format_value(-2.5e7, Some(1), ""), "-2.5e7");
    }

    // --- undo ----------------------------------------------------------------

    impl Harness {
        /// The wheel turned over the middle of the plot, then frames for
        /// the smooth scrolling to finish.
        fn wheel(&mut self, dy: f32) {
            self.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
            self.frame(vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, dy),
                phase: egui::TouchPhase::Move,
                modifiers: self.mods,
            }]);
            for _ in 0..20 {
                self.frame(vec![]);
            }
        }

        /// `key` pressed with `mods` held.
        fn key_with(&mut self, key: egui::Key, mods: egui::Modifiers) {
            let held = std::mem::replace(&mut self.mods, mods);
            let ev = |pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: mods,
            };
            self.frame(vec![ev(true)]);
            self.frame(vec![ev(false)]);
            self.mods = held;
        }

        /// A click of mouse button `button` over the plot.
        fn click_with(&mut self, button: egui::PointerButton) {
            for pressed in [true, false] {
                self.frame(vec![egui::Event::PointerButton {
                    pos: MIDDLE,
                    button,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }]);
            }
        }

        /// A drag across the plot from `from` to `to` (x), pausing for
        /// `pause` half way.
        fn drag(&mut self, from: f32, to: f32, pause: std::time::Duration) {
            let at = |x: f32| Pos2::new(x, 300.0);
            self.frame(vec![egui::Event::PointerMoved(at(from))]);
            self.button(at(from), true);
            let mid = (from + to) / 2.0;
            for x in [from + 10.0, mid] {
                self.frame(vec![egui::Event::PointerMoved(at(x))]);
            }
            std::thread::sleep(pause);
            self.frame(vec![egui::Event::PointerMoved(at(to))]);
            self.button(at(to), false);
            self.frame(vec![]);
        }
    }

    /// A live plot of `sim://sine` in the harness, its adding undoable.
    fn sine() -> Harness {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        p.add_pvs(&names(&["sim://sine"]));
        let h = Harness::new(p);
        assert_eq!(h.plot.undo_label(), Some("add sim://sine"));
        h
    }

    #[test]
    fn a_zoom_is_undone_and_redone() {
        let mut h = sine();
        let id = h.plot.traces[0].id;
        h.wheel(200.0);
        let zoomed = h.plot.window.span;
        assert!(zoomed < 3600.0 * 0.8);
        assert_eq!(h.plot.undo.len(), (2, 0));
        assert_eq!(h.plot.undo_label(), Some("zoom"));
        assert!(h.plot.undo());
        assert_eq!(h.plot.window, Window::live(3600.0));
        assert_eq!(h.plot.redo_label(), Some("zoom"));
        assert_eq!(h.plot.undo_label(), Some("add sim://sine"));
        assert_eq!(h.plot.traces[0].id, id, "the same trace, not a new one");
        h.frame(vec![]);
        assert_eq!(h.plot.undo.len(), (1, 1), "undoing is not a step");
        assert!(h.plot.redo());
        assert_eq!(h.plot.window.span, zoomed);
        assert_eq!(h.plot.redo_label(), None);
        // Back to the start: an empty plot, nothing more to undo.
        h.plot.undo();
        h.plot.undo();
        assert!(h.plot.traces.is_empty());
        assert!(!h.plot.undo());
        assert_eq!(h.plot.undo_label(), None);
        assert!(h.plot.redo());
        assert_eq!(h.plot.traces[0].id, id, "back with its own id");
    }

    #[test]
    fn a_scroll_burst_and_a_drag_are_one_step_each() {
        let mut h = sine();
        // Two turns of the wheel close together: one step.
        h.wheel(100.0);
        h.wheel(100.0);
        assert_eq!(h.plot.undo.len(), (2, 0));
        // After a pause, another.
        h.plot.undo.age();
        h.wheel(100.0);
        assert_eq!(h.plot.undo.len(), (3, 0));
        // A drag that rests a while half way is still one step.
        h.plot.undo.age();
        h.drag(500.0, 700.0, std::time::Duration::from_millis(700));
        assert!(!h.plot.window.live);
        assert_eq!(h.plot.undo.len(), (4, 0));
        assert_eq!(h.plot.undo_label(), Some("pan"));
        // Undone, it returns to Live: where the step started.
        h.plot.undo();
        assert!(h.plot.window.live);
    }

    #[test]
    fn live_motion_and_settling_are_not_steps() {
        let mut h = sine();
        h.until(|p| p.traces[0].axis.is_some());
        for _ in 0..10 {
            h.frame(vec![]);
        }
        assert_eq!(h.plot.undo.len(), (1, 0), "only adding it");
    }

    #[test]
    fn a_reset_is_one_step_and_undone() {
        let mut h = sine();
        h.wheel(200.0);
        let zoomed = h.plot.window.span;
        // Values zoomed too, shift+wheel over the data.
        h.until(|p| p.traces[0].axis.is_some());
        h.frame(vec![]);
        h.plot.undo.age();
        h.mods = egui::Modifiers::SHIFT;
        h.wheel(100.0);
        h.mods = egui::Modifiers::NONE;
        assert!(!h.plot.autoscaling());
        assert_eq!(h.plot.undo_label(), Some("zoom values"));
        let range = (h.plot.panes[0].axes[0].lo, h.plot.panes[0].axes[0].hi);
        // A reset right after (no pause): a step of its own.
        h.key(egui::Key::Home);
        assert_eq!(h.plot.undo_label(), Some("reset"));
        assert!(h.plot.autoscaling() && h.plot.window.span == 3600.0);
        h.plot.undo();
        assert_eq!(h.plot.window.span, zoomed);
        assert!(!h.plot.panes[0].axes[0].autoscale);
        assert_eq!(
            (h.plot.panes[0].axes[0].lo, h.plot.panes[0].axes[0].hi),
            range
        );
        h.plot.undo();
        assert!(h.plot.autoscaling(), "the values' zoom undone");
        assert_eq!(h.plot.window.span, zoomed);
    }

    #[test]
    fn a_new_change_clears_redo() {
        let mut h = sine();
        h.wheel(200.0);
        h.plot.undo();
        assert_eq!(h.plot.redo_label(), Some("zoom"));
        h.plot.undo.age();
        h.key(egui::Key::ArrowLeft);
        assert_eq!(h.plot.undo_label(), Some("pan"));
        assert_eq!(h.plot.redo_label(), None);
        assert!(!h.plot.redo());
    }

    #[test]
    fn the_steps_are_capped() {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        for i in 0..150 {
            p.window = Window::live(60.0 + i as f64);
            p.undo.age();
            p.commit(false);
        }
        assert_eq!(p.undo.len(), (undo::MAX_STEPS, 0));
        while p.undo() {}
        assert_eq!(p.window.span, 60.0 + 49.0, "the oldest are gone");
    }

    #[test]
    fn keys_and_mouse_buttons_undo_and_redo() {
        let mut h = sine();
        h.wheel(200.0);
        let zoomed = h.plot.window.span;
        let ctrl = egui::Modifiers::COMMAND;
        h.key_with(egui::Key::Z, ctrl);
        assert_eq!(h.plot.window.span, 3600.0);
        h.key_with(egui::Key::Z, ctrl | egui::Modifiers::SHIFT);
        assert_eq!(h.plot.window.span, zoomed);
        h.key_with(egui::Key::Z, ctrl);
        h.key_with(egui::Key::Y, ctrl);
        assert_eq!(h.plot.window.span, zoomed);
        // As in a browser: back and forward.
        h.click_with(egui::PointerButton::Extra1);
        assert_eq!(h.plot.window.span, 3600.0);
        h.click_with(egui::PointerButton::Extra2);
        assert_eq!(h.plot.window.span, zoomed);
        // A host with its own undo: neither.
        h.plot.set_undo_controls(false);
        h.key_with(egui::Key::Z, ctrl);
        h.click_with(egui::PointerButton::Extra1);
        assert_eq!(h.plot.window.span, zoomed);
        assert!(h.plot.undo(), "the host may still undo");
    }

    #[test]
    fn a_removed_trace_comes_back_as_it_was_without_a_fetch() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let pvs = PvManager::new();
        let client = ArchiverClient::new(&mock.url()).unwrap();
        let mut p = Plot::new(&pvs, Some(client));
        p.add_pvs(&names(&["MOCK:SINE", "MOCK:NOISE", "MOCK:STEP"]));
        let mut h = Harness::new(p);
        h.until(|p| !p.is_loading() && p.traces.iter().all(|t| t.axis.is_some()));
        h.frame(vec![]);
        h.plot.traces[0].color = Some("#d62728".into());
        h.frame(vec![]);
        assert_eq!(h.plot.undo_label(), Some("change colours"));
        let before = h.plot.view();
        let ids: Vec<u64> = h.plot.traces.iter().map(|t| t.id).collect();
        let slots: Vec<usize> = h.plot.traces.iter().map(|t| t.slot).collect();
        let axes = axis_indices(&h.plot);
        let samples = h.plot.traces[1].history.as_ref().unwrap().samples.len();
        h.plot.fetcher.sent.lock().clear();

        // ✕ on the first and the middle chip.
        for i in [1, 0] {
            let id = h.plot.traces[i].id;
            let chip = h.plot.chips_shown.iter().find(|c| c.0 == id).unwrap().1;
            let x = Pos2::new(chip.right() - 12.0, chip.center().y);
            h.frame(vec![egui::Event::PointerMoved(x)]);
            h.button(x, true);
            h.button(x, false);
            h.frame(vec![]);
        }
        assert_eq!(h.plot.names(), ["MOCK:STEP"]);
        assert_eq!(h.plot.undo_label(), Some("remove MOCK:SINE"));
        assert_eq!(h.plot.panes[0].axes.len(), 1);
        assert_eq!(h.plot.undo.parked(), 2);

        h.plot.undo();
        h.plot.undo();
        for _ in 0..10 {
            h.frame(vec![]);
        }
        assert_eq!(h.plot.view(), before, "colour, axes and order");
        let t: &Vec<Trace> = &h.plot.traces;
        assert_eq!(t.iter().map(|t| t.id).collect::<Vec<_>>(), ids);
        assert_eq!(t.iter().map(|t| t.slot).collect::<Vec<_>>(), slots);
        assert_eq!(axis_indices(&h.plot), axes);
        let hist = h.plot.traces[1].history.as_ref().expect("its history kept");
        assert_eq!(hist.samples.len(), samples);
        assert!(!h.plot.is_loading());
        assert!(
            h.plot.fetcher.sent.lock().is_empty(),
            "nothing fetched again"
        );
        // Redone: gone again, and its trace stays parked for an undo.
        h.plot.redo();
        assert_eq!(h.plot.names().len(), 2);
        assert_eq!(h.plot.undo.parked(), 1);
    }

    #[test]
    fn axis_moves_and_renames_are_undone() {
        let (mut p, data) = plot_of(&[
            ("SR-DI-DCCT-01:SIGNAL", "mA", &[299.5, 300.0]),
            ("SR-DI-DCCT-01:LIFETIME", "h", &[5.2, 11.9]),
            ("SR01C-DI-EBPM-01:SA:X", "mm", &[0.0, 0.04]),
            ("SR01C-DI-EBPM-01:SA:Y", "mm", &[0.0, 0.6]),
        ]);
        p.settle_axes(&data, 0.0);
        p.commit(false);
        // Added one by one: a step each.
        assert_eq!(p.undo.len(), (4, 0));
        assert_eq!(p.undo_label(), Some("add SR01C-DI-EBPM-01:SA:Y"));
        let settled = axis_indices(&p);
        assert_eq!(settled, [Some(0), Some(1), Some(2), Some(2)]);

        // SA:Y to an axis of its own, then onto the lifetime's.
        let y = p.traces[3].id;
        p.move_by_hand(y, None);
        p.commit(false);
        assert_eq!(
            p.undo_label(),
            Some("move SR01C-DI-EBPM-01:SA:Y to Position Y (mm)")
        );
        // Named as drawn before the drop.
        p.panes[0].axis_names = p.name_axes(0);
        let lifetime = p.traces[1].axis;
        p.move_by_hand(y, lifetime);
        p.commit(false);
        assert_eq!(
            p.undo_label(),
            Some("move SR01C-DI-EBPM-01:SA:Y to Time (h)")
        );
        p.undo();
        assert_eq!(p.panes[0].axes.len(), 4);
        assert_eq!(
            p.redo_label(),
            Some("move SR01C-DI-EBPM-01:SA:Y to Time (h)")
        );
        p.undo();
        assert_eq!(axis_indices(&p), settled);
        assert_eq!(p.panes[0].axes.len(), 3);

        // Renamed, then back.
        p.panes[0].axes[1].name = Some("Lifetime".into());
        p.commit(false);
        assert_eq!(p.undo_label(), Some("rename axis to “Lifetime”"));
        p.undo();
        assert_eq!(p.panes[0].axes[1].name, None);
        p.redo();
        assert_eq!(p.panes[0].axes[1].name.as_deref(), Some("Lifetime"));

        // Hidden, and its axis' log scale.
        p.traces[0].visible = false;
        p.commit(false);
        assert_eq!(p.undo_label(), Some("hide SR-DI-DCCT-01:SIGNAL"));
        p.panes[0].axes[2].log = true;
        p.commit(false);
        assert_eq!(p.undo_label(), Some("log scale"));
        p.undo();
        p.undo();
        assert!(p.traces[0].visible && !p.panes[0].axes[2].log);
    }

    #[test]
    fn a_trace_that_settles_after_a_step_stays_settled_through_its_undo() {
        let (mut p, data) = plot_of(&[("A", "mA", &[300.0]), ("B", "h", &[10.0])]);
        p.commit(false);
        // Zoomed before they settle.
        p.window = Window::live(600.0);
        p.commit(false);
        p.settle_axes(&data, 0.0);
        p.commit(false);
        assert_eq!(
            p.undo.len(),
            (3, 0),
            "adding A, B, zooming: settling is no step"
        );
        let settled = axis_indices(&p);
        assert_eq!(settled, [Some(0), Some(1)]);
        p.undo();
        assert_eq!(p.window.span, 3600.0);
        assert_eq!(axis_indices(&p), settled, "not settling again");
        // Back to before they were added, and again: still on their axes.
        p.undo();
        p.undo();
        assert!(p.traces.is_empty() && p.panes[0].axes.is_empty());
        p.redo();
        p.redo();
        assert_eq!(axis_indices(&p), settled);
        assert!(p.traces.iter().all(|t| t.stats.is_some()));

        // Settled in the same frame as a zoom: the same.
        let (mut p, data) = plot_of(&[("A", "mA", &[300.0]), ("B", "h", &[10.0])]);
        p.commit(false);
        p.window = Window::live(600.0);
        p.settle_axes(&data, 0.0);
        p.commit(false);
        assert_eq!(p.undo_label(), Some("zoom"));
        p.undo();
        assert_eq!(axis_indices(&p), settled);
    }

    #[test]
    fn opening_a_view_is_undone() {
        let mut h = sine();
        h.until(|p| p.traces[0].axis.is_some());
        let id = h.plot.traces[0].id;
        let before = h.plot.view();
        let other = View {
            pvs: vec![TraceView {
                name: "sim://ramp".into(),
                color: None,
                visible: true,
                axis: None,
            }],
            range: view::Range::Relative { secs: 8.0 * 3600.0 },
            ..View::default()
        };
        h.plot.set_view(&other);
        h.frame(vec![]);
        assert_eq!(h.plot.undo_label(), Some("open view"));
        h.plot.undo();
        assert_eq!(h.plot.view(), before);
        assert_eq!(h.plot.traces[0].id, id);
        h.plot.set_view(&View::default());
        h.frame(vec![]);
        assert_eq!(h.plot.undo_label(), Some("clear the plot"));
    }

    #[test]
    fn a_narrow_bar_has_undo_in_its_menu() {
        let mut h = sine();
        h.width = 500.0;
        h.frame(vec![]);
        // ≡ at the right of the bar.
        let menu = Pos2::new(500.0 - 16.0, 14.0);
        h.frame(vec![egui::Event::PointerMoved(menu)]);
        h.button(menu, true);
        h.button(menu, false);
        h.frame(vec![]);
        let popup = h.ctx.memory(|m| {
            let layer = m.areas().top_layer_id(egui::Order::Foreground)?;
            m.area_rect(layer.id)
        });
        let popup = popup.expect("the menu opens");
        // Its first item: "Undo add sim://sine".
        let first = Pos2::new(popup.center().x, popup.top() + 16.0);
        h.frame(vec![egui::Event::PointerMoved(first)]);
        h.button(first, true);
        h.button(first, false);
        assert!(h.plot.traces.is_empty(), "undone from the menu");
        assert_eq!(h.plot.redo_label(), Some("add sim://sine"));
    }

    #[test]
    fn a_live_only_trace_comes_back_with_its_data() {
        let mut h = sine();
        let start = std::time::Instant::now();
        while start.elapsed().as_millis() < 1500 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            h.frame(vec![]);
        }
        let t = super::now();
        let before = h.plot.traces[0].points(t - 3600.0, t, t).len();
        assert!(before > 2, "{before}");
        h.plot.remove_pv("sim://sine");
        h.frame(vec![]);
        h.plot.undo();
        h.frame(vec![]);
        let t = super::now();
        let after = h.plot.traces[0].points(t - 3600.0, t, t);
        assert!(after.len() >= before, "{} < {before}", after.len());
        // Where it was off the plot, a gap.
        assert!(after.iter().any(|p| p.v.is_nan()));
    }

    #[test]
    fn an_undo_in_a_scrolls_tail_is_whole_and_keeps_redo() {
        let mut h = sine();
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.frame(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, 200.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        }]);
        h.frame(vec![]);
        // Ctrl+Z while egui still eases the scroll in.
        h.key_with(egui::Key::Z, egui::Modifiers::COMMAND);
        for _ in 0..30 {
            h.frame(vec![]);
        }
        assert_eq!(h.plot.window.span, 3600.0);
        assert_eq!(h.plot.undo.len(), (1, 1));
        assert_eq!(h.plot.redo_label(), Some("zoom"));
        // The wheel turned again zooms as ever.
        h.wheel(200.0);
        assert!(h.plot.window.span < 3600.0 * 0.8);
    }

    #[test]
    fn a_drag_right_after_a_scroll_is_one_step_however_slow() {
        let mut h = sine();
        h.wheel(100.0);
        // Within the half second: the drag joins the scroll's step, and
        // resting half way does not split it.
        h.drag(500.0, 700.0, std::time::Duration::from_millis(700));
        assert_eq!(h.plot.undo.len(), (2, 0));
        h.plot.undo();
        assert_eq!(h.plot.window, Window::live(3600.0));
    }

    #[test]
    fn a_held_key_is_one_step() {
        let mut h = sine();
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        let ev = |pressed, repeat| egui::Event::Key {
            key: egui::Key::ArrowLeft,
            physical_key: None,
            pressed,
            repeat,
            modifiers: egui::Modifiers::NONE,
        };
        h.frame(vec![ev(true, false)]);
        // The OS waits longer than the gap before it repeats.
        std::thread::sleep(std::time::Duration::from_millis(700));
        h.frame(vec![ev(true, true)]);
        h.frame(vec![ev(true, true)]);
        h.frame(vec![ev(false, false)]);
        assert_eq!(h.plot.undo.len(), (2, 0));
        assert_eq!(h.plot.undo_label(), Some("pan"));
    }

    #[test]
    fn a_step_learns_of_an_axis_as_it_first_was() {
        let (mut p, mut data) = plot_of(&[("B", "mA", &[]), ("A", "mA", &[300.0, 301.0])]);
        p.commit(false);
        // A settles first (B has no data yet): an axis of its own.
        p.settle_axes(&data, 0.0);
        p.commit(false);
        assert_eq!(axis_indices(&p), [None, Some(0)]);
        p.panes[0].axes[0].name = Some("Beam".into());
        p.commit(false);
        // B's data comes, and it joins A's axis.
        data[0] = vec![Pt::raw(0.0, 299.0), Pt::raw(1.0, 300.0)];
        p.traces[0].history = hist_of("mA", &[299.0, 300.0]);
        p.settle_axes(&data, 1.0);
        p.commit(false);
        assert_eq!(axis_indices(&p), [Some(0), Some(0)]);
        // Before A was added, B is on that axis as it was then: unnamed.
        p.undo();
        assert_eq!(p.undo_label(), Some("add A"));
        p.undo();
        assert_eq!(p.names(), ["B"]);
        assert_eq!(axis_indices(&p), [Some(0)]);
        assert_eq!(p.panes[0].axes[0].name, None, "not the later rename");
    }

    #[test]
    fn undo_can_be_turned_off() {
        let mut h = sine();
        h.plot.set_undo_enabled(false);
        assert_eq!(h.plot.undo.len(), (0, 0));
        h.wheel(200.0);
        h.plot.remove_pv("sim://sine");
        h.frame(vec![]);
        assert_eq!(h.plot.undo.len(), (0, 0));
        assert_eq!(h.plot.undo.parked(), 0, "nothing kept for it");
        assert!(!h.plot.undo());
        assert_eq!(h.plot.undo_label(), None);
        // On again: from the state then.
        h.plot.set_undo_enabled(true);
        h.plot.add_pvs(&names(&["sim://ramp"]));
        h.frame(vec![]);
        assert_eq!(h.plot.undo_label(), Some("add sim://ramp"));
        h.plot.undo();
        assert!(h.plot.traces.is_empty());
    }

    #[test]
    fn undoing_an_opened_view_brings_back_the_name_before() {
        let mut h = sine();
        h.plot.set_view_name(Some("mine"));
        h.frame(vec![]);
        let other = View {
            pvs: vec![TraceView {
                name: "sim://ramp".into(),
                color: None,
                visible: true,
                axis: None,
            }],
            ..View::default()
        };
        h.plot.set_view(&other);
        h.plot.set_view_name(Some("other"));
        h.frame(vec![]);
        assert_eq!(h.plot.undo_label(), Some("open view"));
        h.plot.undo();
        assert_eq!(h.plot.view_name(), Some("mine"));
        h.plot.redo();
        assert_eq!(h.plot.view_name(), Some("other"));
    }

    #[test]
    fn a_paste_is_one_undo_step_and_into_the_search_box_none() {
        let mut h = sine();
        h.plot.paste_pvs("A:B, C:D\nE:F sim://sine");
        h.frame(vec![]);
        assert_eq!(h.plot.undo.len(), (2, 0));
        assert_eq!(h.plot.undo_label(), Some("add 3 PVs"));
        h.plot.undo();
        assert_eq!(h.plot.names(), ["sim://sine"]);
        h.plot.redo();
        // Ctrl+V over the plot: one more.
        h.frame(vec![egui::Event::PointerMoved(MIDDLE)]);
        h.frame(vec![egui::Event::Paste("G:H I:J".into())]);
        h.frame(vec![]);
        assert_eq!(h.plot.undo.len(), (3, 0));
        assert_eq!(h.plot.undo_label(), Some("add 2 PVs"));
        // Into the search box, by a middle-click or Ctrl+V there: text,
        // not a step.
        h.plot.paste_into_search("K:L");
        h.frame(vec![]);
        h.key(egui::Key::Slash);
        h.frame(vec![egui::Event::Paste("M:N".into())]);
        h.frame(vec![]);
        assert_eq!(h.plot.undo.len(), (3, 0));
        assert_eq!(h.plot.names().len(), 6);
    }
}
