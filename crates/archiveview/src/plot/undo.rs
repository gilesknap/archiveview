//! Undo and redo of what the plot shows: the time range, the panes and
//! their value axes, and the traces on them.
//!
//! Snapshot-based: each step keeps the state from before it (what
//! [`Plot::view`] saves, plus what views don't: hidden flags, palette
//! slots, pane and axis ids and decisions, and the stats traces settled
//! with). A
//! change is noticed by comparing the state with the last one seen, at
//! the start and end of each frame and when a host calls in, so every
//! path that changes the plot is recorded, whoever calls it. Settling (a
//! trace taking its axis) and Live's own motion are not changes.
//!
//! Restoring diffs rather than rebuilds: a trace in both states keeps its
//! data and live connection, so undoing a zoom fetches nothing new. A
//! trace taken off the plot is parked (its history and live samples kept,
//! its channel closed), so undoing its removal brings it back with its
//! colour, axis, place and data.

use std::collections::VecDeque;

use super::axes::Stats;
use super::pane::Pane;
use super::series::{Parked, Trace, plain_name};
use super::window::Window;
use super::{Plot, ValueAxis, now};

/// The most steps kept.
pub const MAX_STEPS: usize = 100;

/// Changes of the range closer together than this (seconds) are one
/// step: a burst of scrolling or keys.
pub const GAP: f64 = 0.5;

/// Parked traces keep at most this many samples (archived and live)
/// between them; past it the oldest forget theirs, their history fetched
/// again if they come back.
const PARKED_SAMPLES: usize = 1_000_000;

/// What the plot shows, as a step keeps it.
#[derive(Clone, Debug)]
pub struct Snapshot {
    window: Window,
    home_span: f64,
    default_log: bool,
    traces: Vec<TraceState>,
    panes: Vec<PaneState>,
    /// The host's name for the view shown ([`Plot::set_view_name`]).
    view_name: Option<String>,
}

impl Snapshot {
    fn trace(&self, id: u64) -> Option<&TraceState> {
        self.traces.iter().find(|t| t.id == id)
    }

    /// Every value axis, pane by pane.
    fn axes(&self) -> impl Iterator<Item = &ValueAxis> {
        self.panes.iter().flat_map(|p| &p.axes)
    }

    fn axis(&self, id: u32) -> Option<&ValueAxis> {
        self.axes().find(|a| a.id == id)
    }

    fn pane(&self, id: u32) -> Option<&PaneState> {
        self.panes.iter().find(|p| p.id == id)
    }
}

/// A pane, as a step keeps it.
#[derive(Clone, Debug)]
struct PaneState {
    id: u32,
    weight: f32,
    axes: Vec<ValueAxis>,
}

/// A trace, as a step keeps it.
#[derive(Clone, Debug)]
struct TraceState {
    id: u64,
    name: String,
    color: Option<String>,
    slot: usize,
    visible: bool,
    axis: Option<u32>,
    pane: Option<u32>,
    stats: Option<Stats>,
}

impl TraceState {
    fn label(&self) -> String {
        plain_name(&pv::PvName::parse(&self.name))
    }
}

/// A step: the state on one side of it, and what it did ("zoom").
struct Step {
    state: Snapshot,
    label: String,
}

/// What kind of change was seen.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Change {
    /// The time range or a value axis' range: one gesture or burst of
    /// these is one step.
    Navigate,
    /// The panes' heights: one drag of a divider is one step.
    Resize,
    /// Anything else: always a step of its own.
    Other,
}

/// The step at the top of the undo list may take in more of the same
/// gesture.
struct Open {
    /// When it last changed (seconds since 1970).
    at: f64,
    /// The pointer press it began in, if it began with a button down.
    press: Option<u64>,
    /// What changes it takes in.
    kind: Change,
}

/// The plot's undo and redo lists.
#[derive(Default)]
pub struct Undo {
    done: VecDeque<Step>,
    undone: Vec<Step>,
    /// The state as last seen.
    last: Option<Snapshot>,
    /// What the next step is called, said by the code making it ("reset");
    /// else it is described from what changed.
    hint: Option<String>,
    open: Option<Open>,
    /// Traces taken off the plot that a step may bring back, oldest
    /// first.
    parked: Vec<Parked>,
    /// Each axis as it was when it first appeared, and its pane, for
    /// steps that learn of a trace settling on it later.
    born: Vec<(u32, ValueAxis)>,
    /// Counts pointer and key presses, to keep a drag or a held key one
    /// step however slow.
    pub press: u64,
    /// Hide the buttons and ignore the keys (a host with its own undo).
    pub hidden: bool,
    /// Record nothing ([`Plot::set_undo_enabled`]).
    off: bool,
    /// Ignore egui's smoothed scrolling until the wheel turns again: the
    /// tail of a scroll undone must not zoom on.
    pub hold_scroll: bool,
}

#[cfg(test)]
impl Undo {
    /// The steps to undo and to redo.
    pub fn len(&self) -> (usize, usize) {
        (self.done.len(), self.undone.len())
    }

    /// Let more than [`GAP`] pass since the last change.
    pub fn age(&mut self) {
        if let Some(o) = &mut self.open {
            o.at -= 2.0 * GAP;
        }
    }

    /// How many traces are parked.
    pub fn parked(&self) -> usize {
        self.parked.len()
    }
}

impl Plot {
    /// Show the ↶ ↷ buttons and take Ctrl+Z, Ctrl+Shift+Z, Ctrl+Y and the
    /// mouse's back and forward buttons (on by default). A host with its
    /// own undo turns them off; the plot still records its steps, for
    /// [`Plot::undo`] and [`Plot::redo`].
    pub fn set_undo_controls(&mut self, on: bool) {
        self.undo.hidden = !on;
    }

    /// Record undo steps (on by default). Off, the lists are cleared and
    /// nothing is recorded, nor kept for it (removed traces are dropped at
    /// once), and the controls do nothing: for a host with an undo of its
    /// own that has no use for the plot's. On again, recording starts
    /// from the state then.
    pub fn set_undo_enabled(&mut self, on: bool) {
        let u = &mut self.undo;
        if on == !u.off {
            return;
        }
        u.off = !on;
        u.done.clear();
        u.undone.clear();
        u.parked.clear();
        u.born.clear();
        u.open = None;
        u.hint = None;
        u.last = None;
        if on {
            self.undo.last = Some(self.capture());
        }
    }

    /// The host's name for the view shown (the standalone app's saved
    /// view), kept with each undo step: undoing the opening of a view
    /// brings back the name of the one before. Setting it is no step.
    pub fn set_view_name(&mut self, name: Option<&str>) {
        // Not compared for changes: the next step's state before it takes
        // the name in, and a view opened just before keeps the old one.
        self.view_name = name.map(str::to_string);
    }

    /// The name given with [`Plot::set_view_name`], as undo has left it.
    pub fn view_name(&self) -> Option<&str> {
        self.view_name.as_deref()
    }

    /// Undo the last step (a zoom, a reset, a trace removed, ...): true if
    /// there was one.
    pub fn undo(&mut self) -> bool {
        self.commit(false);
        let Some(step) = self.undo.done.pop_back() else {
            return false;
        };
        let now = self.undo.last.take().expect("seen");
        self.undo.undone.push(Step {
            state: now,
            label: step.label,
        });
        self.restore(&step.state);
        true
    }

    /// Redo the last step undone: true if there was one.
    pub fn redo(&mut self) -> bool {
        self.commit(false);
        let Some(step) = self.undo.undone.pop() else {
            return false;
        };
        let now = self.undo.last.take().expect("seen");
        self.undo.done.push_back(Step {
            state: now,
            label: step.label,
        });
        self.restore(&step.state);
        true
    }

    /// What [`Plot::undo`] would undo ("zoom", "remove SR-DI-DCCT-01:SIGNAL"),
    /// as of the last frame or call; `None` with nothing to undo.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.done.back().map(|s| s.label.as_str())
    }

    /// What [`Plot::redo`] would redo, as [`Plot::undo_label`].
    pub fn redo_label(&self) -> Option<&str> {
        self.undo.undone.last().map(|s| s.label.as_str())
    }

    /// The state now, as a step keeps it.
    pub(super) fn capture(&self) -> Snapshot {
        Snapshot {
            window: self.window,
            home_span: self.home_span,
            default_log: self.default_log,
            traces: self
                .traces
                .iter()
                .map(|t| TraceState {
                    id: t.id,
                    name: t.name.clone(),
                    color: t.color.clone(),
                    slot: t.slot,
                    visible: t.visible,
                    axis: t.axis,
                    pane: t.pane,
                    stats: t.stats.clone(),
                })
                .collect(),
            panes: (self.panes.iter())
                .map(|p| PaneState {
                    id: p.id,
                    weight: p.weight,
                    axes: p.axes.clone(),
                })
                .collect(),
            view_name: self.view_name.clone(),
        }
    }

    /// The next change is a step of its own called `label` ("reset"):
    /// what changed before it is recorded first. Within a change already
    /// named (opening a view sets its range), the first name stands.
    pub(super) fn name_step(&mut self, label: impl Into<String>) {
        if self.undo.hint.is_none() && !self.undo.off {
            self.commit(false);
            self.undo.hint = Some(label.into());
        }
    }

    /// Record what changed since the state was last seen, if anything:
    /// a new step, or more of the open one (a gesture, while a button or
    /// key is `down` in the press it began with, or a burst). Settling is
    /// not a change, but the steps learn of it.
    pub(super) fn commit(&mut self, down: bool) {
        if self.undo.off {
            return;
        }
        let current = self.capture();
        let Some(last) = self.undo.last.take() else {
            self.undo.last = Some(current);
            return;
        };
        let hint = self.undo.hint.take();
        let settled = settled(&last, &current);
        for p in &current.panes {
            for a in &p.axes {
                if last.axis(a.id).is_none() && !self.undo.born.iter().any(|b| b.1.id == a.id) {
                    let first = ValueAxis {
                        shown: None,
                        ..a.clone()
                    };
                    self.undo.born.push((p.id, first));
                }
            }
        }
        if let Some(change) = changed(&last, &current) {
            let at = now();
            let press = self.undo.press;
            let held = |o: &Open| down && o.press == Some(press);
            let more = hint.is_none()
                && !self.undo.done.is_empty()
                && (self.undo.open.as_ref()).is_some_and(|o| {
                    o.kind == change
                        && match change {
                            Change::Navigate => at - o.at <= GAP || held(o),
                            // A divider dragged: only while it is held.
                            Change::Resize => held(o),
                            Change::Other => false,
                        }
                });
            if more {
                let label = {
                    let top = self.undo.done.back().expect("open");
                    self.describe(&top.state, &current)
                };
                self.undo.done.back_mut().expect("open").label = label;
                if let Some(o) = &mut self.undo.open {
                    o.at = at;
                    // A drag begun in a scroll's step: one step to its end.
                    if down {
                        o.press = Some(press);
                    }
                }
            } else {
                // A step named by its code (a reset, a preset) takes in
                // nothing more: only a gesture's does.
                let gesture = change != Change::Other && hint.is_none();
                let label = hint.unwrap_or_else(|| self.describe(&last, &current));
                self.undo.open = gesture.then(|| Open {
                    at,
                    press: down.then_some(press),
                    kind: change,
                });
                self.undo.done.push_back(Step { state: last, label });
                while self.undo.done.len() > MAX_STEPS {
                    self.undo.done.pop_front();
                }
                self.undo.undone.clear();
            }
            self.forget_parked();
        }
        // After the step is pushed: it may be from before the settling.
        self.learn_settling(&settled, &current);
        self.undo.last = Some(current);
    }

    /// Show `s`, keeping what is in both: traces keep their data and
    /// live connections; those only in `s` come back from where they
    /// were parked, and those not in it are parked.
    fn restore(&mut self, s: &Snapshot) {
        let mut old = std::mem::take(&mut self.traces);
        for ts in &s.traces {
            let mut t = if let Some(i) = old.iter().position(|t| t.id == ts.id) {
                old.remove(i)
            } else if let Some(i) = self.undo.parked.iter().position(|p| p.id == ts.id) {
                let p = self.undo.parked.remove(i);
                let pv = self.pvs.subscribe(&p.name);
                Trace::revive(p, pv)
            } else {
                // Not kept (it never should be lost): as new, with an id
                // of its own, so no answer for the old one is taken.
                debug_assert!(false, "trace {} neither shown nor parked", ts.id);
                let id = self.next_trace;
                self.next_trace += 1;
                Trace::new(id, &ts.name, self.pvs.subscribe(&ts.name))
            };
            t.color = ts.color.clone();
            t.slot = ts.slot;
            t.visible = ts.visible;
            t.axis = ts.axis;
            t.pane = ts.pane;
            t.stats = ts.stats.clone();
            self.traces.push(t);
        }
        for t in old {
            self.park(t);
        }
        // Panes and axes still there carry on as drawn: the axes from
        // their ranges, a pane with its axis picked out.
        let old = std::mem::take(&mut self.panes);
        let shown: Vec<(u32, Option<(f64, f64)>)> = (old.iter().flat_map(|p| &p.axes))
            .map(|a| (a.id, a.shown))
            .collect();
        self.panes = (s.panes.iter())
            .map(|ps| {
                let axes = (ps.axes.iter().cloned())
                    .map(|mut a| {
                        a.shown = shown.iter().find(|(id, _)| *id == a.id).and_then(|s| s.1);
                        a
                    })
                    .collect();
                let mut pane = Pane::with_axes(ps.id, ps.weight, axes);
                if let Some(was) = old.iter().find(|p| p.id == ps.id) {
                    pane.axis_highlight = was.axis_highlight;
                }
                pane
            })
            .collect();
        if self.panes.is_empty() {
            self.panes = old;
            self.panes.truncate(1);
            self.panes[0].axes.clear();
        }
        self.prune_axes();
        self.window = s.window;
        self.home_span = s.home_span;
        self.default_log = s.default_log;
        self.view_name = s.view_name.clone();
        self.glide = 0.0;
        self.undo.hold_scroll = true;
        self.changed_at = now();
        self.undo.open = None;
        self.undo.hint = None;
        self.undo.last = Some(self.capture());
        self.forget_parked();
    }

    /// Take trace `t` off the plot, keeping it for a step to bring back.
    pub(super) fn park(&mut self, t: Trace) {
        if self.highlight == Some(t.id) {
            self.highlight = None;
        }
        if self.undo.off {
            return;
        }
        let parked = &mut self.undo.parked;
        parked.push(t.park());
        let mut total: usize = parked.iter().map(Parked::samples).sum();
        for p in parked.iter_mut() {
            if total <= PARKED_SAMPLES {
                break;
            }
            total -= p.samples();
            p.forget_data();
        }
    }

    /// Forget every parked trace's history: from another archiver now.
    pub(super) fn forget_parked_histories(&mut self) {
        for p in &mut self.undo.parked {
            p.forget_history();
        }
    }

    /// Drop the parked traces no step can bring back, and the first
    /// look of axes neither shown nor in a step.
    fn forget_parked(&mut self) {
        let Undo {
            done,
            undone,
            parked,
            born,
            ..
        } = &mut self.undo;
        let steps = || done.iter().chain(undone.iter());
        parked.retain(|p| steps().any(|s| s.state.trace(p.id).is_some()));
        let panes = &self.panes;
        born.retain(|(_, b)| {
            panes.iter().any(|p| p.axis(b.id).is_some())
                || steps().any(|s| s.state.axis(b.id).is_some())
        });
    }

    /// The traces `ids` settled, as they are in `after` (the state now):
    /// every step learns of it, so undoing never unsettles a trace, nor
    /// brings one back to settle again. A step without the axis it
    /// settled on gets the axis as it first appeared (not renamed or
    /// rescaled since).
    fn learn_settling(&mut self, ids: &[u64], after: &Snapshot) {
        if ids.is_empty() {
            return;
        }
        let settled: Vec<&TraceState> = (after.traces.iter())
            .filter(|t| ids.contains(&t.id))
            .collect();
        let Undo {
            done, undone, born, ..
        } = &mut self.undo;
        for step in done.iter_mut().chain(undone.iter_mut()) {
            let s = &mut step.state;
            for t in &settled {
                let Some(st) = s.traces.iter_mut().find(|x| x.id == t.id) else {
                    continue;
                };
                if st.stats.is_none() {
                    st.stats = t.stats.clone();
                }
                let (None, Some(id)) = (st.axis, t.axis) else {
                    continue;
                };
                st.axis = Some(id);
                st.pane = t.pane;
                // An axis kept for it (from a view) is not kept any more.
                for p in &mut s.panes {
                    p.axes
                        .retain(|a| a.id == id || a.used || a.reserved != Some(t.id));
                }
                match s
                    .panes
                    .iter_mut()
                    .flat_map(|p| &mut p.axes)
                    .find(|a| a.id == id)
                {
                    Some(a) => {
                        a.used = true;
                        a.reserved = None;
                    }
                    None => {
                        let now = (after.panes.iter())
                            .find_map(|p| p.axes.iter().find(|a| a.id == id).map(|a| (p.id, a)));
                        let first = (born.iter())
                            .find(|(_, a)| a.id == id)
                            .map(|(p, a)| (*p, a))
                            .or(now);
                        if let Some((pane, a)) = first {
                            // Its pane too, where the step has none (it
                            // opened as the trace settled).
                            let k = match s.panes.iter().position(|p| p.id == pane) {
                                Some(k) => k,
                                None => {
                                    let weight = after.pane(pane).map_or(1.0, |p| p.weight);
                                    s.panes.push(PaneState {
                                        id: pane,
                                        weight,
                                        axes: Vec::new(),
                                    });
                                    s.panes.len() - 1
                                }
                            };
                            s.panes[k].axes.push(ValueAxis {
                                shown: None,
                                used: true,
                                reserved: None,
                                ..a.clone()
                            });
                        }
                    }
                }
            }
        }
    }

    /// What changed from `before` to `after`, in a few words: "zoom",
    /// "remove SR-DI-DCCT-01:SIGNAL", "move SR-DI-DCCT-01:LIFETIME to
    /// Time (h)". `after` is the state now (the axes are named from it).
    fn describe(&self, before: &Snapshot, after: &Snapshot) -> String {
        let added: Vec<&TraceState> = (after.traces.iter())
            .filter(|t| before.trace(t.id).is_none())
            .collect();
        let removed: Vec<&TraceState> = (before.traces.iter())
            .filter(|t| after.trace(t.id).is_none())
            .collect();
        match (added.as_slice(), removed.as_slice()) {
            ([], []) => {}
            ([one], []) => return format!("add {}", one.label()),
            (many, []) => return format!("add {} PVs", many.len()),
            ([], [one]) => return format!("remove {}", one.label()),
            ([], many) => return format!("remove {} PVs", many.len()),
            _ => return "change the PVs".into(),
        }
        let pairs: Vec<(&TraceState, &TraceState)> = (after.traces.iter())
            .filter_map(|t| before.trace(t.id).map(|b| (b, t)))
            .collect();
        let moved: Vec<&(&TraceState, &TraceState)> = (pairs.iter())
            .filter(|(b, a)| {
                (b.axis.is_some() && a.axis.is_some() && b.axis != a.axis)
                    // Not settled yet: put in a pane.
                    || (a.axis.is_none() && a.pane.is_some() && b.pane != a.pane)
            })
            .collect();
        match moved.as_slice() {
            [] => {}
            [(b, t)] if b.pane != t.pane && t.pane.is_some_and(|p| before.pane(p).is_none()) => {
                return format!("move {} to a new pane", t.label());
            }
            [(_, t)] if t.axis.is_none() => {
                let n = (t.pane).and_then(|p| after.panes.iter().position(|x| x.id == p));
                return format!("move {} to pane {}", t.label(), n.map_or(1, |n| n + 1));
            }
            [(_, t)] => {
                // The axis as it was named when the trace was dropped on
                // it (as last drawn), else as it is named now (a new one).
                return match t.axis.and_then(|id| self.axis_name(id)) {
                    Some(name) => format!("move {} to {name}", t.label()),
                    None => format!("move {}", t.label()),
                };
            }
            _ => return "move PVs".into(),
        }
        let flipped: Vec<&TraceState> = (pairs.iter())
            .filter(|(b, a)| b.visible != a.visible)
            .map(|(_, a)| *a)
            .collect();
        match flipped.as_slice() {
            [] => {}
            [one] if one.visible => return format!("show {}", one.label()),
            [one] => return format!("hide {}", one.label()),
            many => return format!("show or hide {} PVs", many.len()),
        }
        if pairs.iter().any(|(b, a)| b.color != a.color) {
            return "change colours".into();
        }
        let axes: Vec<(&ValueAxis, &ValueAxis)> = (after.axes())
            .filter_map(|a| before.axis(a.id).map(|b| (b, a)))
            .collect();
        if let Some((_, a)) = axes.iter().find(|(b, a)| b.name != a.name) {
            return match &a.name {
                Some(name) => format!("rename axis to “{name}”"),
                None => "rename axis".into(),
            };
        }
        let log = |on: bool| if on { "log scale" } else { "linear scale" };
        if let Some((_, a)) = axes.iter().find(|(b, a)| b.log != a.log) {
            return log(a.log).into();
        }
        if before.default_log != after.default_log {
            return log(after.default_log).into();
        }
        if resized(before, after) {
            return "resize panes".into();
        }
        if before.window == after.window {
            let fixed = |a: &ValueAxis| (!a.autoscale).then_some((a.lo, a.hi));
            let changed: Vec<&(&ValueAxis, &ValueAxis)> = (axes.iter())
                .filter(|(b, a)| fixed(b) != fixed(a))
                .collect();
            if !changed.is_empty() && changed.iter().all(|(_, a)| a.autoscale) {
                return "autoscale".into();
            }
            let zoomed = changed.iter().any(|(b, a)| {
                b.autoscale != a.autoscale
                    || ((b.hi - b.lo) - (a.hi - a.lo)).abs() > 1e-9 * (b.hi - b.lo).abs()
            });
            if !changed.is_empty() {
                return if zoomed { "zoom values" } else { "pan values" }.into();
            }
        }
        let (b, a) = (before.window, after.window);
        if b.span != a.span {
            "zoom".into()
        } else if b != a {
            "pan".into()
        } else {
            "change".into()
        }
    }
}

/// The traces that settled (took an axis, or their stats) from `before`
/// to `after`.
fn settled(before: &Snapshot, after: &Snapshot) -> Vec<u64> {
    (after.traces.iter())
        .filter(|t| {
            before.trace(t.id).is_some_and(|b| {
                (b.axis.is_none() && t.axis.is_some()) || (b.stats.is_none() && t.stats.is_some())
            })
        })
        .map(|t| t.id)
        .collect()
}

/// Whether, and how, the plot changed from `before` to `after`: settling
/// (a trace taking its first axis, axes coming and going with it) and
/// the ranges drawn easing are not changes.
fn changed(before: &Snapshot, after: &Snapshot) -> Option<Change> {
    let traces = before.traces.len() != after.traces.len()
        || before.traces.iter().zip(&after.traces).any(|(b, a)| {
            b.id != a.id
                || b.color != a.color
                || b.slot != a.slot
                || b.visible != a.visible
                || (b.axis.is_some() && b.axis != a.axis)
                // A pane given by hand, not taken as it settles.
                || (b.pane != a.pane && (b.pane.is_some() || a.axis.is_none()))
        });
    if traces || before.default_log != after.default_log {
        return Some(Change::Other);
    }
    let mut navigate = before.window != after.window || before.home_span != after.home_span;
    for a in after.axes() {
        let Some(b) = before.axis(a.id) else {
            continue;
        };
        if b.name != a.name || b.log != a.log {
            return Some(Change::Other);
        }
        navigate |= b.autoscale != a.autoscale || (!a.autoscale && (b.lo, b.hi) != (a.lo, a.hi));
    }
    if navigate {
        Some(Change::Navigate)
    } else {
        resized(before, after).then_some(Change::Resize)
    }
}

/// Whether a pane in both `before` and `after` changed its height.
/// Panes coming and going with their traces are not resizing.
fn resized(before: &Snapshot, after: &Snapshot) -> bool {
    (after.panes.iter()).any(|a| before.pane(a.id).is_some_and(|b| b.weight != a.weight))
}
