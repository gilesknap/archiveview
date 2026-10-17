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
//! - `bar`: the slim top bar (search, range pill, live pill, buttons);
//! - `legend`: the chips under the plot;
//! - `hover`: the crosshair readout, empty state and status line.

mod bar;
mod fetch;
mod hover;
mod legend;
mod render;
mod series;
mod timeaxis;
mod window;

use std::sync::atomic::{AtomicU64, Ordering};

use archiver::{ArchiverClient, Operator, ValueKind};
use egui::{Color32, Pos2, Rect, Sense, Vec2};
use pv::PvManager;

use crate::view::{self, TraceView, View, YAxis};
use fetch::Fetcher;
pub use series::now;
use series::{History, Pt, Trace};
use window::Window;

pub use timeaxis::parse_span;

/// Wait this long after the last pan or zoom before fetching (seconds).
const DEBOUNCE: f64 = 0.3;

/// While following now, refresh the archived data this often (seconds).
const LIVE_REFRESH: f64 = 30.0;

/// After an error, ask again no sooner than this (seconds), doubling with
/// each failure in a row.
const RETRY: f64 = 10.0;

/// The Live pill pulses this long after following now starts (seconds).
const PULSE_SECS: f64 = 3.0;

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
    y: YState,
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
    /// Keyboard shortcuts apply: the plot was clicked last.
    active: bool,
    search: bar::SearchState,
    custom: bar::CustomState,
}

/// The value axis.
#[derive(Clone, Copy, Debug)]
struct YState {
    autoscale: bool,
    /// The fixed range when not autoscaling.
    lo: f64,
    hi: f64,
    log: bool,
    /// The range drawn, easing toward the target.
    shown: Option<(f64, f64)>,
}

impl Default for YState {
    fn default() -> Self {
        Self {
            autoscale: true,
            lo: 0.0,
            hi: 1.0,
            log: false,
            shown: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    /// Moving through time.
    Pan,
    /// Zooming values on the value axis.
    Axis,
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
        Self {
            pvs: pvs.clone(),
            archiver,
            id,
            traces: Vec::new(),
            next_trace: 0,
            window: Window::live(3600.0),
            home_span: 3600.0,
            shown: None,
            y: YState::default(),
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
            active: false,
            search: bar::SearchState::default(),
            custom: bar::CustomState::default(),
        }
    }

    /// Change (or remove) the archiver; refetches every trace.
    pub fn set_archiver(&mut self, archiver: Option<ArchiverClient>) {
        self.archiver = archiver;
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

    /// The current view (traces, range, y axis), to save.
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
        View {
            version: view::VERSION,
            pvs: self
                .traces
                .iter()
                .map(|t| TraceView {
                    name: t.name.clone(),
                    color: t.color.clone(),
                    visible: t.visible,
                })
                .collect(),
            range,
            y: YAxis {
                autoscale: self.y.autoscale,
                min: (!self.y.autoscale).then_some(self.y.lo),
                max: (!self.y.autoscale).then_some(self.y.hi),
                log: self.y.log,
            },
        }
    }

    /// Show `view`, replacing the traces and range.
    pub fn set_view(&mut self, view: &View) {
        self.traces.clear();
        for tv in &view.pvs {
            if let Some(t) = self.add(&tv.name) {
                t.color = tv.color.clone();
                t.visible = tv.visible;
            }
        }
        self.set_range(&view.range);
        self.y = YState {
            autoscale: view.y.autoscale || view.y.min.is_none() || view.y.max.is_none(),
            lo: view.y.min.unwrap_or(0.0),
            hi: view.y.max.unwrap_or(1.0),
            log: view.y.log,
            shown: None,
        };
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

    /// True while any archive fetch or search is in flight, or one is due
    /// (snapshots wait for this).
    pub fn is_loading(&self) -> bool {
        self.fetcher.pending()
            || self.pending
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
        let (lo, hi) = self.shown_values(&data, from, to, dt);

        let font = egui::FontId::proportional(11.5);
        let units = self.common_units();
        let f = render::frame(
            ui.painter(),
            rect,
            (from, to),
            (lo, hi),
            self.y.log,
            &units,
            &font,
        );
        self.interact(ui, &resp, &f, now);
        let ppp = ui.ctx().pixels_per_point();
        self.schedule_fetches(now, f.data.width() * ppp);

        // Paint.
        let lines: Vec<render::Line> = self
            .traces
            .iter()
            .zip(&data)
            .filter(|(t, _)| t.visible)
            .map(|(t, pts)| {
                let color = self.color(t, dark);
                let (color, width) = match self.highlight {
                    Some(h) if h == t.id => (color, 2.5),
                    Some(_) => (color.gamma_multiply(0.25), 1.5),
                    None => (color, 1.5),
                };
                render::Line { pts, color, width }
            })
            .collect();
        let painter = ui.painter_at(rect);
        if self.traces.is_empty() {
            painter.rect_filled(rect, 10.0, ui.visuals().extreme_bg_color);
        } else {
            render::paint(&painter, &f, &lines, ui.visuals());
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
        } else if !matches!(self.drag, Some(Drag::Pan | Drag::Axis)) {
            hover::crosshair(self, ui, &f, &data, dark);
        }
        hover::status(self, ui, &f);
        legend::paint(self, ui, &chips, &data, dark);

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

    /// The units of the visible traces when they all have the same.
    fn common_units(&self) -> String {
        let units: Vec<String> = self
            .traces
            .iter()
            .filter(|t| t.visible)
            .map(units)
            .collect();
        match units.split_first() {
            Some((first, rest)) if rest.iter().all(|u| u == first) => first.clone(),
            _ => String::new(),
        }
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

    /// The value range to draw this frame: fitted to the visible data
    /// (envelopes included) or fixed, easing toward it.
    fn shown_values(&mut self, data: &[Vec<Pt>], from: f64, to: f64, dt: f64) -> (f64, f64) {
        let log = self.y.log;
        let target = if self.y.autoscale {
            let values = data.iter().flat_map(|pts| {
                let start = pts.partition_point(|p| p.t < from).saturating_sub(1);
                pts[start..]
                    .iter()
                    .take_while(|p| p.t <= to)
                    .flat_map(|p| [p.lo, p.hi, p.v])
            });
            render::autoscale(log, values).or(self.y.shown)
        } else {
            Some((self.y.lo, self.y.hi))
        };
        let (lo, hi) = target.unwrap_or(if log { (1.0, 10.0) } else { (0.0, 1.0) });
        let (mut a, mut b) = self.y.shown.unwrap_or((lo, hi));
        let scale = hi - lo;
        window::ease(&mut a, lo, dt, scale);
        window::ease(&mut b, hi, dt, scale);
        if a.partial_cmp(&b) != Some(std::cmp::Ordering::Less) {
            (a, b) = (lo, hi);
        }
        self.y.shown = Some((a, b));
        (a, b)
    }

    /// Take in finished fetches and searches.
    fn take_results(&mut self, now: f64) {
        for done in self.fetcher.take() {
            let Some(t) = self.traces.iter_mut().find(|t| t.id == done.trace) else {
                continue;
            };
            if done.generation != t.fetch.generation {
                continue;
            }
            t.fetch.in_flight = false;
            if done.result.is_ok() {
                t.fetch.failures = 0;
            }
            // The window really fetched: a raw-only trace's is shorter.
            if let Some(r) = &mut t.fetch.requested {
                r.from = r.from.max(done.from);
            }
            if let Some(info) = &done.info {
                t.fetch.raw_only = Some(!info.binnable());
            }
            match done.result {
                Ok(series) => {
                    let series = series.unwrap_or_default();
                    let scalar = series.kind == ValueKind::Scalar;
                    if series.samples.is_empty() {
                        // Nothing to tell the type by.
                    } else if !scalar && done.op != Operator::Raw {
                        // A waveform or text: ask again, raw.
                        t.fetch.raw_only = Some(true);
                        t.fetch.requested = None;
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
                    t.history = Some(History {
                        to: done.to,
                        binned: series.binned,
                        kind: series.kind,
                        meta,
                        samples: series.samples,
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
                    t.fetch.error = Some(e.to_string());
                    t.fetch.failures += 1;
                }
            }
        }
        bar::take_found(self, now);
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
            if t.fetch.not_archived {
                continue;
            }
            let raw = t.fetch.raw_only.unwrap_or(false);
            let from = fetch::needed_from(from, to, raw);
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
                        !t.fetch.in_flight && now - r.at > wait
                    } else {
                        !covered || (live && !t.fetch.in_flight && now - r.at > LIVE_REFRESH)
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
            t.fetch.requested = Some(series::Requested {
                from: a,
                to: b,
                op,
                at: now,
            });
            let job = fetch::Job {
                trace: t.id,
                generation: t.fetch.generation,
                pv: name,
                from: a,
                to: b,
                op,
                raw_from,
                probe: t.fetch.raw_only.is_none(),
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
            self.drag = Some(
                if shift || resp.dragged_by(egui::PointerButton::Secondary) {
                    Drag::Box(origin)
                } else if f.value_axis().contains(origin) {
                    Drag::Axis
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
                    if !self.y.autoscale && d.y != 0.0 {
                        let (lo, hi) = self.y.shown.unwrap_or((self.y.lo, self.y.hi));
                        let frac = -(d.y / data.height().max(1.0)) as f64;
                        (self.y.lo, self.y.hi) = window::pan_values(lo, hi, frac, self.y.log);
                        self.y.shown = Some((self.y.lo, self.y.hi));
                    }
                    self.snap = true;
                    self.changed_at = now;
                }
                Some(Drag::Axis) => {
                    let factor = (d.y as f64 * 0.01).exp();
                    self.zoom_values(factor, None, f);
                    self.y.shown = Some((self.y.lo, self.y.hi));
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
                            self.y.autoscale = false;
                            (self.y.lo, self.y.hi) = (f.value_at(r.bottom()), f.value_at(r.top()));
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
            let at_value = pointer.map(|p| f.value_at(p.y));
            if mods.shift && (scroll.x != 0.0 || scroll.y != 0.0) {
                // Shift+wheel arrives as horizontal scrolling.
                let amount = (scroll.x + scroll.y) as f64;
                self.zoom_values((-amount / 300.0).exp(), at_value, f);
            } else if zoom != 1.0 {
                if mods.ctrl || mods.command {
                    self.zoom_values(1.0 / zoom as f64, at_value, f);
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

    /// Zoom values by `factor` about `at` (the middle when `None`), leaving
    /// autoscale.
    fn zoom_values(&mut self, factor: f64, at: Option<f64>, f: &render::Frame) {
        let (lo, hi) = self.y.shown.unwrap_or((f.ys.lo, f.ys.hi));
        let mid = if self.y.log && lo > 0.0 {
            (lo * hi).sqrt()
        } else {
            (lo + hi) / 2.0
        };
        let (l, h) = window::zoom_values(lo, hi, factor, at.unwrap_or(mid), self.y.log);
        if l.is_finite() && h.is_finite() && l < h {
            self.y.autoscale = false;
            (self.y.lo, self.y.hi) = (l, h);
        }
    }

    /// Back to the home span, following now, values fitted.
    fn reset(&mut self) {
        self.window = Window::live(self.home_span);
        self.y.autoscale = true;
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

    /// Show a preset span, following now.
    fn preset(&mut self, span: f64) {
        self.window = Window::live(span);
        self.home_span = span;
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
            self.y.autoscale = true;
        }
        if pressed(Key::Home) {
            self.reset();
        }
    }
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

/// A value for the readout and chips: at `prec` decimals, else compactly.
fn format_value(v: f64, prec: Option<usize>, units: &str) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let text = match prec {
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
    fn views_round_trip() {
        let pvs = PvManager::new();
        let mut p = Plot::new(&pvs, None);
        let v = View {
            pvs: vec![
                TraceView {
                    name: "SR-DI-DCCT-01:SIGNAL".into(),
                    color: None,
                    visible: true,
                },
                TraceView {
                    name: "bl01t:SUM".into(),
                    color: Some("#d62728".into()),
                    visible: false,
                },
            ],
            range: view::Range::Relative { secs: 8.0 * 3600.0 },
            y: YAxis {
                autoscale: false,
                min: Some(0.0),
                max: Some(300.0),
                log: false,
            },
            ..View::default()
        };
        p.set_view(&v);
        assert_eq!(p.view(), v);
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
    }

    impl Harness {
        fn new(plot: Plot) -> Self {
            let mut h = Self {
                ctx: egui::Context::default(),
                plot,
                t: 0.0,
            };
            h.frame(vec![]);
            h
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
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
        assert!(h.plot.y.autoscale);
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
        // Its history arrives, binned.
        h.until(|p| !p.is_loading());
        let t = &h.plot.traces[0];
        let hist = t.history.as_ref().expect("history");
        assert!(!hist.samples.is_empty());
        assert_eq!(hist.meta.egu.as_deref(), Some("mA"));
        assert_eq!(t.fetch.raw_only, Some(false));
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
        assert!(t[2].history.is_none(), "no archive for sim://");
        // Enums are fetched raw: whole state indices, not bin means.
        assert_eq!(t[3].fetch.raw_only, Some(true));
        let enum_hist = t[3].history.as_ref().expect("enum history");
        assert!(!enum_hist.binned);
        assert!(enum_hist.samples.iter().all(|s| s.val.fract() == 0.0));
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
    }
}
