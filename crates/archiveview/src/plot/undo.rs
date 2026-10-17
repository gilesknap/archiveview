//! Undo and redo of what the plot shows: the time range, the value axes,
//! and the traces on them.
//!
//! Snapshot-based: each step keeps the state from before it (what
//! [`Plot::view`] saves, plus what views don't: hidden flags, palette
//! slots, axis ids and decisions, and the stats traces settled with). A
//! change is noticed by comparing the state with the last one seen, at
//! the start and end of each frame and when a host calls in, so every
//! path that changes the plot is recorded, whoever calls it. Settling (a
//! trace taking its axis) and Live's own motion are not changes.
//!
//! Restoring diffs rather than rebuilds: a trace in both states keeps its
//! data and live connection, so undoing a zoom fetches nothing new. A
//! trace taken off the plot is parked (its history kept, its channel
//! closed), so undoing its removal brings it back with its colour, axis,
//! place and history.

use std::collections::VecDeque;

use super::axes::Stats;
use super::series::{Parked, Trace, plain_name};
use super::window::Window;
use super::{Plot, ValueAxis, now};

/// The most steps kept.
pub const MAX_STEPS: usize = 100;

/// Changes of the range closer together than this (seconds) are one
/// step: a burst of scrolling or keys.
pub const GAP: f64 = 0.5;

/// Parked traces keep at most this many archived samples between them;
/// past it the oldest forget theirs, to be fetched again if they come
/// back.
const PARKED_SAMPLES: usize = 1_000_000;

/// What the plot shows, as a step keeps it.
#[derive(Clone, Debug)]
pub struct Snapshot {
    window: Window,
    home_span: f64,
    default_log: bool,
    traces: Vec<TraceState>,
    axes: Vec<ValueAxis>,
}

impl Snapshot {
    fn trace(&self, id: u64) -> Option<&TraceState> {
        self.traces.iter().find(|t| t.id == id)
    }

    fn axis(&self, id: u32) -> Option<&ValueAxis> {
        self.axes.iter().find(|a| a.id == id)
    }
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
    /// Counts pointer presses, to keep a drag one step however slow.
    pub press: u64,
    /// Hide the buttons and ignore the keys (a host with its own undo).
    pub hidden: bool,
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
                    stats: t.stats.clone(),
                })
                .collect(),
            axes: self.axes.clone(),
        }
    }

    /// The next change is a step of its own called `label` ("reset"):
    /// what changed before it is recorded first. Within a change already
    /// named (opening a view sets its range), the first name stands.
    pub(super) fn name_step(&mut self, label: impl Into<String>) {
        if self.undo.hint.is_none() {
            self.commit(false);
            self.undo.hint = Some(label.into());
        }
    }

    /// Record what changed since the state was last seen, if anything:
    /// a new step, or more of the open one (a gesture, while the pointer
    /// is `down` in the press it began with, or a burst). Settling is
    /// not a change, but the steps learn of it.
    pub(super) fn commit(&mut self, down: bool) {
        let current = self.capture();
        let Some(last) = self.undo.last.take() else {
            self.undo.last = Some(current);
            return;
        };
        let hint = self.undo.hint.take();
        let settled = settled(&last, &current);
        if let Some(change) = changed(&last, &current) {
            let at = now();
            let press = self.undo.press;
            let more = hint.is_none()
                && change == Change::Navigate
                && !self.undo.done.is_empty()
                && self
                    .undo
                    .open
                    .as_ref()
                    .is_some_and(|o| at - o.at <= GAP || (down && o.press == Some(press)));
            if more {
                let label = {
                    let top = self.undo.done.back().expect("open");
                    self.describe(&top.state, &current)
                };
                self.undo.done.back_mut().expect("open").label = label;
                if let Some(o) = &mut self.undo.open {
                    o.at = at;
                }
            } else {
                // A step named by its code (a reset, a preset) takes in
                // nothing more: only a gesture's does.
                let gesture = change == Change::Navigate && hint.is_none();
                let label = hint.unwrap_or_else(|| self.describe(&last, &current));
                self.undo.open = gesture.then(|| Open {
                    at,
                    press: down.then_some(press),
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
                // Not kept (it never should be lost): as new.
                Trace::new(ts.id, &ts.name, self.pvs.subscribe(&ts.name))
            };
            t.color = ts.color.clone();
            t.slot = ts.slot;
            t.visible = ts.visible;
            t.axis = ts.axis;
            t.stats = ts.stats.clone();
            self.traces.push(t);
        }
        for t in old {
            self.park(t);
        }
        // Axes still there carry on from the range drawn.
        let shown: Vec<(u32, Option<(f64, f64)>)> =
            self.axes.iter().map(|a| (a.id, a.shown)).collect();
        self.axes = (s.axes.iter().cloned())
            .map(|mut a| {
                a.shown = shown.iter().find(|(id, _)| *id == a.id).and_then(|s| s.1);
                a
            })
            .collect();
        self.prune_axes();
        self.window = s.window;
        self.home_span = s.home_span;
        self.default_log = s.default_log;
        self.glide = 0.0;
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
        let parked = &mut self.undo.parked;
        parked.push(t.park());
        let len = |p: &Parked| p.history.as_ref().map_or(0, |h| h.samples.len());
        let mut total: usize = parked.iter().map(len).sum();
        for p in parked.iter_mut() {
            if total <= PARKED_SAMPLES {
                break;
            }
            total -= len(p);
            p.forget_history();
        }
    }

    /// Forget every parked trace's history: from another archiver now.
    pub(super) fn forget_parked_histories(&mut self) {
        for p in &mut self.undo.parked {
            p.forget_history();
        }
    }

    /// Drop the parked traces no step can bring back.
    fn forget_parked(&mut self) {
        let Undo {
            done,
            undone,
            parked,
            ..
        } = &mut self.undo;
        parked
            .retain(|p| (done.iter().chain(undone.iter())).any(|s| s.state.trace(p.id).is_some()));
    }

    /// The traces `ids` settled, as they are in `after` (the state now):
    /// every step learns of it, so undoing never unsettles a trace, nor
    /// brings one back to settle again.
    fn learn_settling(&mut self, ids: &[u64], after: &Snapshot) {
        if ids.is_empty() {
            return;
        }
        let settled: Vec<&TraceState> = (after.traces.iter())
            .filter(|t| ids.contains(&t.id))
            .collect();
        let Undo { done, undone, .. } = &mut self.undo;
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
                // An axis kept for it (from a view) is not kept any more.
                s.axes
                    .retain(|a| a.id == id || a.used || a.reserved != Some(t.id));
                match s.axes.iter_mut().find(|a| a.id == id) {
                    Some(a) => {
                        a.used = true;
                        a.reserved = None;
                    }
                    None => {
                        if let Some(a) = after.axis(id) {
                            s.axes.push(ValueAxis {
                                shown: None,
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
            .filter(|(b, a)| b.axis.is_some() && a.axis.is_some() && b.axis != a.axis)
            .collect();
        match moved.as_slice() {
            [] => {}
            [(_, t)] => {
                // The axis as it was named when the trace was dropped on
                // it (as last drawn), else as it is named now (a new one).
                let named = |names: &[(u32, String)]| {
                    names.iter().find(|(id, _)| Some(*id) == t.axis).cloned()
                };
                let to = named(&self.axis_names).or_else(|| named(&self.name_axes()));
                return match to {
                    Some((_, name)) => format!("move {} to {name}", t.label()),
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
        let axes: Vec<(&ValueAxis, &ValueAxis)> = (after.axes.iter())
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
        });
    if traces || before.default_log != after.default_log {
        return Some(Change::Other);
    }
    let mut navigate = before.window != after.window || before.home_span != after.home_span;
    for a in &after.axes {
        let Some(b) = before.axis(a.id) else {
            continue;
        };
        if b.name != a.name || b.log != a.log {
            return Some(Change::Other);
        }
        navigate |= b.autoscale != a.autoscale || (!a.autoscale && (b.lo, b.hi) != (a.lo, a.hi));
    }
    navigate.then_some(Change::Navigate)
}
