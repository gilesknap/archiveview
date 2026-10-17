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
//! - `axes`: which value axis a trace joins, and the axes' names.
//! - `status`: whether the archiver can be reached.

mod axes;
mod axisui;
mod bar;
mod fetch;
mod hover;
mod legend;
mod render;
mod series;
mod status;
mod timeaxis;
mod window;

use std::sync::atomic::{AtomicU64, Ordering};

use archiver::{ArchiverClient, Operator, ValueKind};
use egui::{Color32, Pos2, Rect, Sense, Vec2};
use pv::PvManager;

use crate::axis;
use crate::history::SearchHistory;
use crate::view::{self, TraceView, View, YAxis};
use axes::{Choice, Members, Stats};
use fetch::Fetcher;
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
    /// The value axes, in the order they are laid out (those with no
    /// visible trace aside).
    axes: Vec<ValueAxis>,
    next_axis: u32,
    /// New axes start on a log scale: the view (from before there were
    /// several axes) said so for its one.
    default_log: bool,
    /// The axes drawn this frame, as ids in `axes`.
    drawn: Vec<u32>,
    /// Each axis' name, by id, as of this frame.
    axis_names: Vec<(u32, String)>,
    /// Each trace's value axis this frame (an index into `drawn`), in
    /// trace order; `None` for one not drawn (hidden, or not settled).
    axis_of: Vec<Option<usize>>,
    /// The axis clicked: its traces are picked out, the others dimmed.
    axis_highlight: Option<u32>,
    /// `axis_highlight` before the last click on an axis, to undo when
    /// that click was the first of a double-click (renaming).
    highlight_before: Option<Option<u32>>,
    /// The axis whose name is being edited, and the text so far.
    renaming: Option<axisui::Renaming>,
    /// A legend chip being dragged (its trace), to drop on an axis.
    chip_drag: Option<u64>,
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
}

/// Where the axes (strip, name) and chips (whole, dot) were last drawn.
#[cfg(test)]
type Seen = (Vec<(Rect, Rect)>, Vec<(Rect, Rect)>);

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
        Self {
            pvs: pvs.clone(),
            archiver,
            id,
            traces: Vec::new(),
            next_trace: 0,
            window: Window::live(3600.0),
            home_span: 3600.0,
            shown: None,
            axes: Vec::new(),
            next_axis: 0,
            default_log: false,
            drawn: Vec::new(),
            axis_names: Vec::new(),
            axis_of: Vec::new(),
            axis_highlight: None,
            highlight_before: None,
            renaming: None,
            chip_drag: None,
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
        }
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
    pub fn add_pvs(&mut self, names: &[String]) {
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

    /// Remove a trace by the name it was added with.
    pub fn remove_pv(&mut self, name: &str) {
        let key = pv::PvName::parse(name).key();
        self.traces
            .retain(|t| t.name != name && t.pv.name().key() != key);
        self.prune_axes();
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

    /// The current view (traces, range, value axes, archiver), to save.
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
        let kept: Vec<&ValueAxis> = (self.axes.iter())
            .filter(|a| self.has_traces(a) || a.reserved.is_some())
            .collect();
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
            archiver: self.archiver.as_ref().map(|a| a.base().to_string()),
        }
    }

    /// Show `view`, replacing the traces, range and value axes. Traces
    /// the view puts on an axis stay there; others join one when their
    /// data arrives. With archiver switching on
    /// ([`Plot::set_archiver_switching`]), a view that names an archiver
    /// switches to it; otherwise, or for a view without one, the archiver
    /// stays as it is.
    pub fn set_view(&mut self, view: &View) {
        if let Some(url) = view.archiver.as_ref().filter(|_| self.switch_archivers) {
            match ArchiverClient::new(url) {
                Ok(a) if self.archiver.as_ref() != Some(&a) => self.set_archiver(Some(a)),
                Ok(_) => {}
                Err(e) => log::warn!("view archiver {url}: {e}"),
            }
        }
        self.traces.clear();
        self.axes = view
            .value_axes()
            .map(|y| {
                let mut a = ValueAxis::from_view(self.next_axis, y);
                self.next_axis += 1;
                a.fit_log_range();
                a
            })
            .collect();
        self.axis_highlight = None;
        self.renaming = None;
        // A file from before there were several axes: its one axis is for
        // its first visible PV, and its log scale for every axis.
        let old = view.axes.is_empty() && view.pvs.iter().all(|t| t.axis.is_none());
        self.default_log = old && view.y.log;
        for tv in &view.pvs {
            let axis = tv.axis.and_then(|i| self.axes.get_mut(i)).map(|a| {
                a.used = true;
                a.id
            });
            if let Some(t) = self.add(&tv.name) {
                t.color = tv.color.clone();
                t.visible = tv.visible;
                t.axis = axis;
            }
        }
        if old
            && let Some(first) = self.traces.iter().find(|t| t.visible)
            && let Some(y) = self.axes.first_mut()
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
        let whole = ui.available_rect_before_wrap();
        self.keyboard(ui, whole, now);

        bar::top_bar(self, ui, menu, now);
        ui.add_space(6.0);

        // The legend's height first, so the plot fills the rest; never
        // smaller than MIN_SIZE (an auto-sized window offers little).
        let width = ui.available_width().max(MIN_SIZE.x);
        let mut chips = legend::layout(self, ui, width);
        let gap = if chips.height > 0.0 { 6.0 } else { 0.0 };
        let plot_h = (ui.available_height() - chips.height - gap).max(MIN_SIZE.y);
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(width, plot_h), Sense::click_and_drag());
        ui.add_space(gap);
        legend::interact(self, ui, &mut chips);
        #[cfg(test)]
        {
            self.chips_shown = chips.shown();
        }

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
        let axes = self.value_axes(&data, from, to, dt, dark);

        let font = egui::FontId::proportional(11.5);
        let title_font = egui::FontId::proportional(13.5);
        let mut f = render::frame(ui.painter(), rect, (from, to), &axes, &font, &title_font);
        self.interact(ui, &resp, &f, now);
        self.axis_interact(ui, &mut f);
        #[cfg(test)]
        {
            self.seen.0 = (0..f.axis_count())
                .filter_map(|k| f.axis_rect(k).zip(f.title_rect(k)))
                .collect();
        }
        let ppp = ui.ctx().pixels_per_point();
        self.schedule_fetches(now, f.data.width() * ppp);
        self.schedule_probe(now);

        // Paint. Bands are lighter the more traces have one; a picked-out
        // trace keeps its band, the others lose theirs.
        let binned: Vec<bool> = data.iter().map(|pts| pts.iter().any(Pt::is_bin)).collect();
        let bands = (self.traces.iter().zip(&binned))
            .filter(|(t, b)| t.visible && t.axis.is_some() && **b)
            .count();
        let lines: Vec<render::Line> = self
            .paint_order()
            .into_iter()
            .filter_map(|i| self.axis_of.get(i).copied().flatten().map(|axis| (i, axis)))
            .map(|(i, axis)| {
                let t = &self.traces[i];
                let color = self.color(t, dark);
                let (color, width, band) = match (self.highlight, self.axis_highlight) {
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
            .collect();
        let painter = ui.painter_at(rect);
        // The crosshair, first so its time tag can hide the label under it.
        let hair = (!self.traces.is_empty()
            && !matches!(self.drag, Some(Drag::Pan | Drag::Axis(_))))
        .then(|| hover::hairline(self, ui, &f, &data))
        .flatten();
        if self.traces.is_empty() {
            painter.rect_filled(rect, 10.0, ui.visuals().extreme_bg_color);
        } else {
            let cover = hair.map(|(p, t)| hover::time_tag_rect(&painter, &f, p.x, t));
            render::paint(&painter, &f, &lines, ui.visuals(), cover);
            render::now_marker(&painter, &f, now, ui.visuals(), &font);
        }
        if let Some(Drag::Box(origin)) = self.drag
            && let Some(p) = ui.input(|i| i.pointer.latest_pos())
        {
            let r = Rect::from_two_pos(origin, p).intersect(f.data);
            let accent = ui.visuals().selection.bg_fill;
            painter.rect(
                r,
                2.0,
                accent.gamma_multiply(0.15),
                egui::Stroke::new(1.0, accent),
                egui::StrokeKind::Inside,
            );
        }
        if self.traces.is_empty() {
            hover::empty_state(&painter, &f, ui.visuals());
        } else if let Some(hair) = hair {
            hover::crosshair(self, ui, &f, &data, dark, hair);
        }
        hover::status(self, ui, &f);
        legend::paint(self, ui, &chips, &data, dark);
        self.rename_box(ui, &f);
        self.chip_ghost(ui, dark);

        // Keep moving: easing, gliding, and time passing while live.
        let target = self.window.range(now);
        let settled = self.shown.is_some_and(|(a, b)| {
            let tol = self.window.span * 1e-3;
            (a - target.0).abs() <= tol && (b - target.1).abs() <= tol
        });
        if self.glide != 0.0 || !settled || self.pending {
            ctx.request_repaint();
        } else if self.window.live {
            let per_pixel = self.window.span / f.data.width().max(1.0) as f64;
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                per_pixel.clamp(1.0 / 15.0, 1.0),
            ));
            if self.pulse {
                ctx.request_repaint_after(std::time::Duration::from_millis(66));
            }
        }
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
        t.color
            .as_deref()
            .and_then(crate::style::parse_hex)
            .map(|c| crate::style::adapt(c, dark))
            .unwrap_or_else(|| crate::style::palette(t.slot, dark))
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

    /// The stats of each axis with traces, by id, from its traces' stats
    /// as last seen (hidden ones too), and how many axes with traces have
    /// none yet (from a view, never shown).
    fn axis_stats(&self) -> (Vec<(u32, Stats)>, usize) {
        let mut known = Vec::new();
        let mut unknown = 0;
        for a in &self.axes {
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
                    for a in &mut self.axes {
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
            let (known, unknown) = self.axis_stats();
            let id = match axes::choose(&stats, &known, unknown) {
                Choice::Join(id) => id,
                Choice::New => self.new_axis(&stats, self.traces[i].id),
            };
            self.traces[i].stats = Some(stats);
            self.put_on(i, id);
        }
        self.prune_axes();
    }

    /// An axis for trace `trace`, with `stats`, to settle on: one from a
    /// view that no trace is on yet (kept for this trace, or for any),
    /// keeping its settings; else a new one, on a log scale for values
    /// over decades (or when the view said log). Its id.
    fn new_axis(&mut self, stats: &Stats, trace: u64) -> u32 {
        if let Some(a) = self
            .axes
            .iter()
            .find(|a| !a.used && a.reserved == Some(trace))
        {
            return a.id;
        }
        let mut a = ValueAxis::new(self.next_axis);
        self.next_axis += 1;
        a.log = (self.default_log && stats.positive) || stats.wants_log();
        let id = a.id;
        self.axes.push(a);
        id
    }

    /// Put trace `i` on axis `id`.
    fn put_on(&mut self, i: usize, id: u32) {
        self.traces[i].axis = Some(id);
        if let Some(a) = self.axes.iter_mut().find(|a| a.id == id) {
            a.used = true;
            a.reserved = None;
        }
    }

    /// Move the trace with id `trace` to axis `to`, or to a new axis of
    /// its own with `None`, by hand. Alone on its axis already, it stays
    /// there (keeping the axis' range, scale and name).
    fn move_trace(&mut self, trace: u64, to: Option<u32>) {
        let Some(i) = self.traces.iter().position(|t| t.id == trace) else {
            return;
        };
        let on = self.traces[i].axis;
        let id = match to {
            Some(id) if self.axes.iter().any(|a| a.id == id) => id,
            Some(_) => return,
            None if on.is_some() && self.traces.iter().filter(|t| t.axis == on).count() == 1 => {
                return;
            }
            None => {
                // Fresh, not one waiting for a view's trace.
                let a = ValueAxis::new(self.next_axis);
                self.next_axis += 1;
                let id = a.id;
                self.axes.push(a);
                id
            }
        };
        self.put_on(i, id);
        self.prune_axes();
    }

    /// Drop the axes with no trace: those whose last trace has left, and
    /// one from a view kept for a trace that is gone, is an array, or
    /// got no data. Keep the others in the legend's order (of each one's
    /// first trace), so the first chip's axis is always the first.
    fn prune_axes(&mut self) {
        let traces = &self.traces;
        self.axes.retain(|a| {
            traces.iter().any(|t| t.axis == Some(a.id))
                || (!a.used
                    && a.reserved
                        .is_some_and(|r| traces.iter().any(|t| t.id == r && !t.is_array())))
        });
        self.axes.sort_by_key(|a| {
            traces
                .iter()
                .position(|t| t.axis == Some(a.id))
                .unwrap_or(usize::MAX)
        });
        if let Some(h) = self.axis_highlight
            && !self.axes.iter().any(|a| a.id == h)
        {
            self.axis_highlight = None;
        }
    }

    /// The axes' names, by id, for those with traces: as the user named
    /// them, else built from their traces' units and names.
    fn name_axes(&self) -> Vec<(u32, String)> {
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
        let with: Vec<&ValueAxis> = self
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

    /// The value axes to draw this frame: each axis with a visible trace,
    /// in order. A trace that has not settled is not drawn yet. Sets
    /// `drawn`, `axis_of` and `axis_names`.
    fn value_axes(
        &mut self,
        data: &[Vec<Pt>],
        from: f64,
        to: f64,
        dt: f64,
        dark: bool,
    ) -> Vec<render::AxisSpec> {
        self.axis_names = self.name_axes();
        let drawn: Vec<u32> = self
            .axes
            .iter()
            .filter(|a| {
                self.traces
                    .iter()
                    .any(|t| t.visible && !t.is_array() && t.axis == Some(a.id))
            })
            .map(|a| a.id)
            .collect();
        if let Some(h) = self.axis_highlight
            && !drawn.contains(&h)
        {
            self.axis_highlight = None;
        }
        self.axis_of = self
            .traces
            .iter()
            .map(|t| {
                t.axis
                    .filter(|_| t.visible && !t.is_array())
                    .and_then(|id| drawn.iter().position(|d| *d == id))
            })
            .collect();
        let several = drawn.len() > 1;
        let mut specs = Vec::new();
        for (k, id) in drawn.iter().enumerate() {
            let members: Vec<usize> = (0..self.traces.len())
                .filter(|&i| self.axis_of[i] == Some(k))
                .collect();
            let whole =
                !members.is_empty() && members.iter().all(|&i| whole_valued(&self.traces[i]));
            let pts = members.iter().map(|&i| &data[i]);
            let a = self.axes.iter_mut().find(|a| a.id == *id).expect("drawn");
            let (lo, hi) = shown_values(a, pts, (from, to), whole, dt);
            let log = a.log;
            let title = self
                .axis_names
                .iter()
                .find(|(n, _)| n == id)
                .map(|(_, name)| name.clone())
                .unwrap_or_default();
            let colors: Vec<Color32> = members
                .iter()
                .map(|&i| self.color(&self.traces[i], dark))
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
                highlight: Some(*id) == self.axis_highlight,
            });
        }
        self.drawn = drawn;
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

    /// Pan, zoom, box zoom and reset on the plot.
    fn interact(&mut self, ui: &egui::Ui, resp: &egui::Response, f: &render::Frame, now: f64) {
        let data = f.data;
        let span = self.window.span;
        let per_px = span / data.width().max(1.0) as f64;
        if resp.clicked() || resp.drag_started() {
            self.active = true;
        } else if ui.input(|i| i.pointer.any_pressed()) && !resp.contains_pointer() {
            self.active = false;
        }

        if resp.drag_started() {
            let origin = ui
                .input(|i| i.pointer.press_origin())
                .unwrap_or(data.center());
            let shift = ui.input(|i| i.modifiers.shift);
            self.glide = 0.0;
            let axis = f.axis_at(origin).and_then(|k| self.drawn.get(k).copied());
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
                        for a in self.drawn_axes_mut().filter(|a| !a.autoscale) {
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
                    if let Some(k) = self.drawn.iter().position(|d| *d == id) {
                        self.zoom_axis(k, factor, None, f);
                    }
                    if let Some(a) = self.axes.iter_mut().find(|a| a.id == id) {
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
                        if r.width() > 6.0 {
                            self.window.set(f.time_at(r.left()), f.time_at(r.right()));
                        }
                        if r.height() > 6.0 {
                            for (k, id) in self.drawn.clone().into_iter().enumerate() {
                                let range = (f.value_on(k, r.bottom()), f.value_on(k, r.top()));
                                if let Some(a) = self.axes.iter_mut().find(|a| a.id == id) {
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
            let (scroll, zoom, mods, pointer) = ui.input(|i| {
                (
                    i.smooth_scroll_delta,
                    i.zoom_delta(),
                    i.modifiers,
                    i.pointer.hover_pos(),
                )
            });
            let anchor = pointer.map_or(1.0, |p| ((p.x - data.left()) / data.width()) as f64);
            if mods.shift && (scroll.x != 0.0 || scroll.y != 0.0) {
                // Shift+wheel arrives as horizontal scrolling.
                let amount = (scroll.x + scroll.y) as f64;
                self.zoom_values((-amount / 300.0).exp(), pointer, f);
            } else if zoom != 1.0 {
                if mods.ctrl || mods.command {
                    self.zoom_values(1.0 / zoom as f64, pointer, f);
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
            if scroll != Vec2::ZERO {
                // Not for a scroll area around the plot too.
                ui.ctx().input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
            }
        }
        if resp.double_clicked() {
            self.reset();
        }
    }

    /// Zoom values by `factor` about the pointer at `at`: the axis under
    /// it alone, or over the data every axis drawn, each about its own
    /// value there (the middle without a pointer). Leaves autoscale.
    fn zoom_values(&mut self, factor: f64, at: Option<Pos2>, f: &render::Frame) {
        match at.and_then(|p| f.axis_at(p)) {
            Some(k) => self.zoom_axis(k, factor, at, f),
            None => {
                for k in 0..self.drawn.len() {
                    self.zoom_axis(k, factor, at, f);
                }
            }
        }
    }

    /// Zoom drawn axis `k` by `factor` about its value at `at`'s height.
    fn zoom_axis(&mut self, k: usize, factor: f64, at: Option<Pos2>, f: &render::Frame) {
        let s = f.scale(k);
        let (shown, value) = ((s.lo, s.hi), at.map(|p| f.value_on(k, p.y)));
        if let Some(a) = self.drawn_axis_mut(k) {
            a.zoom(factor, value, shown);
        }
    }

    /// The value axis drawn `k`th this frame.
    fn drawn_axis_mut(&mut self, k: usize) -> Option<&mut ValueAxis> {
        let id = self.drawn.get(k).copied()?;
        self.axes.iter_mut().find(|a| a.id == id)
    }

    /// The value axes drawn this frame.
    fn drawn_axes_mut(&mut self) -> impl Iterator<Item = &mut ValueAxis> {
        let drawn = &self.drawn;
        self.axes.iter_mut().filter(|a| drawn.contains(&a.id))
    }

    /// Every value axis back to autoscaling.
    fn autoscale(&mut self) {
        for a in &mut self.axes {
            a.autoscale = true;
        }
    }

    /// Whether value axis `a` has traces on it.
    fn has_traces(&self, a: &ValueAxis) -> bool {
        self.traces.iter().any(|t| t.axis == Some(a.id))
    }

    /// Every value axis with traces autoscales.
    fn autoscaling(&self) -> bool {
        self.axes
            .iter()
            .filter(|a| self.has_traces(a))
            .all(|a| a.autoscale)
    }

    /// Every value axis with traces is on a log scale (and there is one).
    fn all_log(&self) -> bool {
        let mut on = self.axes.iter().filter(|a| self.has_traces(a)).peekable();
        on.peek().is_some() && on.all(|a| a.log)
    }

    /// Every value axis on a log scale, or none; new axes too.
    fn set_log(&mut self, log: bool) {
        self.default_log = log;
        for a in &mut self.axes {
            a.log = log;
            a.fit_log_range();
        }
    }

    /// Back to the home span, following now, values fitted.
    fn reset(&mut self) {
        self.window = Window::live(self.home_span);
        self.autoscale();
        self.glide = 0.0;
    }

    /// Toggle following now: pause at the range shown, or jump to now.
    fn toggle_live(&mut self, now: f64) {
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
        self.window = w;
        self.home_span = w.span;
        self.glide = 0.0;
    }

    /// The keyboard shortcuts, when the pointer is over the plot or it was
    /// clicked last, and no text field has the keyboard.
    fn keyboard(&mut self, ui: &egui::Ui, whole: Rect, now: f64) {
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        if typing || !(self.active || ui.rect_contains_pointer(whole)) {
            return;
        }
        use egui::Key;
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

/// Put archiver `url` first in `recent`, at most [`MAX_RECENT`] long.
fn remember(recent: &mut Vec<String>, url: &str) {
    recent.retain(|u| u != url);
    recent.insert(0, url.to_string());
    recent.truncate(MAX_RECENT);
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
/// A tooltip's list of what can be done, one gesture a line, as
/// "action — effect" in two columns (a proportional font does not keep
/// indents made of spaces).
fn gestures(ui: &mut egui::Ui, id: egui::Id, rows: &[(&str, &str)]) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([6.0, 2.0])
        .show(ui, |ui| {
            for (action, effect) in rows {
                ui.add(egui::Label::new(*action).extend());
                ui.add(
                    egui::Label::new(egui::RichText::new(format!("— {effect}")).weak()).extend(),
                );
                ui.end_row();
            }
        });
}

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
    }

    impl Harness {
        fn new(plot: Plot) -> Self {
            let mut h = Self {
                ctx: egui::Context::default(),
                plot,
                t: 0.0,
                mods: egui::Modifiers::NONE,
            };
            h.frame(vec![]);
            h
        }

        fn frame(&mut self, mut events: Vec<egui::Event>) {
            events.insert(0, egui::Event::ModifiersChanged(self.mods));
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 600.0))),
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
            .map(|t| t.axis.and_then(|id| p.axes.iter().position(|a| a.id == id)))
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
        let names: Vec<String> = p.name_axes().into_iter().map(|(_, n)| n).collect();
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
        assert!(p.axes[3].log && !p.axes[0].log);
        // Decisions stick, whatever the data does next.
        data[1] = data[0].clone();
        p.settle_axes(&data, 3.0);
        assert_eq!(
            axis_indices(&p),
            [Some(0), Some(1), Some(2), Some(2), Some(3)]
        );
        // Moved by hand to its own axis, then back: the empty axis goes.
        let y = p.traces[3].id;
        p.move_trace(y, None);
        assert_eq!(p.axes.len(), 5);
        let names: Vec<String> = p.name_axes().into_iter().map(|(_, n)| n).collect();
        assert_eq!(names[2], "Position X (mm)");
        assert_eq!(names[3], "Position Y (mm)", "in the legend's order");
        let x_axis = p.traces[2].axis;
        p.move_trace(y, x_axis);
        assert_eq!(p.axes.len(), 4);
        // Removing a trace removes its axis with it.
        p.remove_pv("SR-DI-DCCT-01:LIFETIME");
        assert_eq!(p.axes.len(), 3);
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
            p.axes.iter().find(|a| a.id == id).unwrap()
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
            let names: Vec<String> = p.name_axes().into_iter().map(|(_, n)| n).collect();
            (axis_indices(&p), names)
        };
        let first = settle(&[0, 1, 2, 3, 4, 5]);
        assert_eq!(
            first.0,
            [Some(0), Some(1), Some(2), Some(2), Some(3), Some(4)]
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
        assert!(p.axes[0].log);
        p.axes[0].name = Some("Vac".into());
        let id = p.traces[0].id;
        p.move_trace(id, None);
        assert_eq!(p.axes.len(), 1);
        assert!(p.axes[0].log);
        assert_eq!(p.axes[0].name.as_deref(), Some("Vac"));
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
        assert_eq!(p.axes.len(), 4);
        // Hide SA:X: SA:Y still joins it, and the four axes are still
        // four, so E (mm, but far bigger) joins it too.
        p.traces[3].visible = false;
        data[3] = Vec::new();
        data[4] = later[0].clone();
        data[5] = later[1].clone();
        p.settle_axes(&data, 1.0);
        assert_eq!(p.axes.len(), 4);
        assert_eq!(p.traces[4].axis, p.traces[3].axis);
        assert_eq!(p.traces[5].axis, p.traces[3].axis);
    }

    #[test]
    fn switching_archivers_keeps_the_axes() {
        let (mut p, data) = plot_of(&[("A", "mA", &[300.0]), ("B", "h", &[10.0])]);
        p.settle_axes(&data, 0.0);
        let before = axis_indices(&p);
        p.set_archiver(Some(ArchiverClient::new("http://127.0.0.1:9").unwrap()));
        p.settle_axes(&[Vec::new(), Vec::new()], 1.0);
        assert_eq!(axis_indices(&p), before);
        assert_eq!(p.axes.len(), 2);
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
        assert_eq!(p.axes.len(), 1, "no axis left for SIG");
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
        assert!(p.axes.iter().find(|a| a.id == id).unwrap().autoscale);

        // A reserved PV that never gets data frees its axis after the wait.
        let (mut p, data) = plot_of(&[("DEAD", "mA", &[]), ("LT", "h", &[10.0])]);
        let hist: Vec<Option<History>> = p.traces.iter().map(|t| t.history.clone()).collect();
        p.set_view(&v);
        for (t, h) in p.traces.iter_mut().zip(hist) {
            t.history = h;
        }
        p.settle_axes(&data, 0.0);
        p.settle_axes(&data, SETTLE_WAIT);
        assert_eq!(p.axes.len(), 1);
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
        assert!(p.axes.iter().all(|a| !a.log));
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
        assert_eq!(h.plot.drawn.len(), 4);
        let names: Vec<String> = h.plot.axis_names.iter().map(|(_, n)| n.clone()).collect();
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
        let auto: Vec<bool> = h.plot.axes.iter().map(|a| a.autoscale).collect();
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
        assert!(h.plot.axes.iter().all(|a| !a.autoscale));
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
        assert_eq!(h.plot.axis_highlight, None, "renaming picks nothing out");
        // The name so far is selected, to type over.
        h.frame(vec![]);
        h.frame(vec![egui::Event::Text("Beam".into())]);
        h.key(egui::Key::Enter);
        assert!(h.plot.renaming.is_none());
        assert_eq!(h.plot.axes[0].name.as_deref(), Some("Beam"));
        assert_eq!(h.plot.view().y.name.as_deref(), Some("Beam"));

        // Click the second axis: its trace is picked out; again, not.
        let second = h.plot.seen.0[1].0.center();
        click(&mut h, second);
        assert_eq!(h.plot.axis_highlight, Some(h.plot.axes[1].id));
        h.frame(vec![]);
        click(&mut h, second);
        assert_eq!(h.plot.axis_highlight, None);
        // Picked out, then its trace hidden: the axis goes, and so does
        // the dimming.
        click(&mut h, second);
        assert!(h.plot.axis_highlight.is_some());
        h.plot.traces[1].visible = false;
        h.frame(vec![]);
        assert_eq!(h.plot.axis_highlight, None);
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
        assert_eq!(h.plot.axes.len(), 1);
        assert_eq!(h.plot.traces[1].axis, h.plot.traces[0].axis);

        // Its dot's menu, "New axis": back on an axis of its own.
        let dot = h.plot.seen.1[1].1.center();
        click(&mut h, dot);
        h.frame(vec![]);
        let open = egui::Popup::is_any_open(&h.ctx);
        assert!(open, "the dot opens its menu");
        h.plot.move_trace(h.plot.traces[1].id, None);
        assert_eq!(h.plot.axes.len(), 2);
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
}
