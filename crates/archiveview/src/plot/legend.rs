//! The legend: a wrapped row of chips under the plot, one per trace: a
//! colour dot, the name, the latest value. Click to hide or show, its
//! cross to remove, hover to pick the trace out.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use super::series::{FetchState, History, Pt};
use super::{Plot, format_value, precision, units};

const HEIGHT: f32 = 26.0;
const GAP: f32 = 6.0;
const PAD: f32 = 10.0;
const DOT: f32 = 8.0;
const CLOSE: f32 = 14.0;
/// A chip's value is cut to this many characters (long strings), and to
/// what fits the window.
const VALUE_CHARS: usize = 32;
/// An array's chip shows its length and this many elements.
const ARRAY_SHOWN: usize = 3;

/// Laid-out chips, relative to the legend's top left until placed.
pub struct Chips {
    pub height: f32,
    origin: Vec2,
    chips: Vec<Chip>,
}

struct Chip {
    trace: u64,
    rect: Rect,
    name: String,
    value: String,
    /// Live and connected: the value is current.
    current: bool,
    severity: Option<Color32>,
    /// Quiet words after the value: "live only", "no data".
    mark: Option<String>,
    /// Its tooltip: the PV's name, then its live and archive status.
    title: String,
    status: Vec<String>,
    array: bool,
    /// The value as shown, for tests.
    #[cfg(test)]
    text: String,
}

impl Chips {
    /// Each chip's trace, rect on screen and value as shown.
    #[cfg(test)]
    pub fn shown(&self) -> Vec<(u64, Rect, String)> {
        (self.chips.iter())
            .map(|c| (c.trace, c.rect.translate(self.origin), c.text.clone()))
            .collect()
    }
}

impl Chip {
    /// Where its colour dot is, with room to click it.
    fn dot_rect(&self, origin: Vec2) -> Rect {
        let r = self.rect.translate(origin);
        Rect::from_center_size(
            Pos2::new(r.left() + PAD + DOT / 2.0, r.center().y),
            Vec2::new(DOT + 10.0, HEIGHT),
        )
    }

    /// What hovering it shows: the PV's name and status, then what can be
    /// done to it.
    fn tooltip(&self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new(&self.title).strong());
        for line in &self.status {
            ui.label(line);
        }
        ui.add_space(4.0);
        let mut rows = vec![("Click", "hide or show"), ("Click its ×", "remove")];
        if !self.array {
            rows.extend([
                ("Click its dot", "choose its value axis"),
                ("Drag onto an axis", "move it to that axis"),
                ("Drag onto the plot", "give it an axis of its own"),
            ]);
        }
        super::gestures(ui, egui::Id::new(("chip-gestures", self.trace)), &rows);
    }

    fn close_rect(&self, origin: Vec2) -> Rect {
        let r = self.rect.translate(origin);
        Rect::from_center_size(
            Pos2::new(r.right() - PAD / 2.0 - CLOSE / 2.0, r.center().y),
            Vec2::splat(CLOSE + 4.0),
        )
    }
}

fn font() -> FontId {
    FontId::proportional(13.0)
}

fn mark_font() -> FontId {
    FontId::proportional(11.0)
}

pub fn layout(plot: &Plot, ui: &egui::Ui, width: f32) -> Chips {
    let painter = ui.painter();
    let font = font();
    let mut chips = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    for t in &plot.traces {
        let state = t.pv.state();
        let prec = precision(t, &state);
        let egu = units(t);
        let array = t.is_array();
        let archived_last = t.history.as_ref().filter(|_| !array).and_then(|h| {
            h.samples
                .iter()
                .rev()
                .find(|s| s.val.is_finite())
                .map(|s| s.val)
        });
        let (value, current) = if state.connected && array {
            // An array archived as a waveform may have one element live.
            let text = array_text(&state.value, prec)
                .unwrap_or_else(|| state.format(prec.map(|p| p as i32), true));
            (text, true)
        } else if state.connected && matches!(state.value, pv::Value::Str(_)) {
            // Text: shown (cut short below), not plotted.
            (state.format(None, false), true)
        } else if state.connected
            && let Some(v) = state.value.as_f64()
        {
            // Large numbers as the axis has them, not in a dozen digits.
            if v.abs() >= 1e6 && state.labels.is_empty() {
                (format_value(v, prec, &egu), true)
            } else {
                (state.format(prec.map(|p| p as i32), true), true)
            }
        } else if let Some(v) = archived_last {
            (format_value(v, prec, &egu), false)
        } else {
            ("—".to_string(), false)
        };
        let mut status = Vec::new();
        if state.connected {
            status.push("Live: connected".into());
        } else if let Some(e) = &state.error {
            status.push(format!("Live: {e}"));
        } else {
            status.push("Live: not connected".into());
        }
        if array {
            status.push("An array: not plotted".into());
        }
        if plot.archiver.is_some() {
            status.push(if t.archive_name().is_none() {
                "Archive: none for this kind of PV".to_string()
            } else if t.fetch.not_archived {
                "Archive: not archived (live only)".to_string()
            } else if let Some(e) = &t.fetch.error {
                format!("Archive: {e}")
            } else if let Some(h) = &t.history {
                if h.kind == archiver::ValueKind::Waveform {
                    "Archive: archived (an array: history not fetched)".to_string()
                } else if h.samples.is_empty() {
                    "Archive: no data in range".to_string()
                } else {
                    let what = match h.kind {
                        archiver::ValueKind::Scalar if h.binned => "bins (mean, min…max)",
                        archiver::ValueKind::Scalar => "samples",
                        archiver::ValueKind::Waveform => "array samples",
                        archiver::ValueKind::Text => "text samples (not plotted)",
                        archiver::ValueKind::Other => "samples (not plottable)",
                    };
                    format!("Archive: {} {what}", h.samples.len())
                }
            } else {
                "Archive: fetching…".to_string()
            });
        }
        let mark = marks(
            plot.archiver.is_some(),
            &t.fetch,
            t.history.as_ref(),
            array,
            value == "—",
        );
        let mark = (!mark.is_empty()).then(|| mark.join(" · "));
        let name = ellipsize(&t.label(), 48);
        // Everything but the value, which gets what is left of the width.
        let rest = PAD
            + DOT
            + 6.0
            + super::render::text_size(painter, &name, &font).x
            + 8.0
            + mark.as_ref().map_or(0.0, |m| {
                6.0 + super::render::text_size(painter, m, &mark_font()).x
            })
            + 6.0
            + CLOSE
            + PAD / 2.0;
        let value = fit(
            painter,
            &ellipsize(&value, VALUE_CHARS),
            &font,
            width - rest,
        );
        let w = (rest + super::render::text_size(painter, &value, &font).x).min(width);
        #[cfg(test)]
        let text = value.clone();
        if x > 0.0 && x + w > width {
            x = 0.0;
            y += HEIGHT + GAP;
        }
        chips.push(Chip {
            trace: t.id,
            rect: Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, HEIGHT)),
            name,
            value,
            current,
            severity: if current {
                crate::style::severity(state.severity)
            } else {
                None
            },
            mark,
            title: t.name.clone(),
            status,
            array,
            #[cfg(test)]
            text,
        });
        x += w + GAP;
    }
    Chips {
        height: if chips.is_empty() { 0.0 } else { y + HEIGHT },
        origin: Vec2::ZERO,
        chips,
    }
}

/// Place the chips (the next thing in `ui`) and handle their clicks.
pub fn interact(plot: &mut Plot, ui: &mut egui::Ui, chips: &mut Chips) {
    if chips.chips.is_empty() {
        plot.highlight = None;
        return;
    }
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), chips.height),
        Sense::hover(),
    );
    chips.origin = rect.min.to_vec2();
    #[cfg(test)]
    plot.seen.1.clear();
    let mut highlight = None;
    let mut toggle = None;
    let mut remove = None;
    for c in &chips.chips {
        // An array is not plotted: no value axis to choose.
        let array = c.array;
        let r = ui
            .interact(
                c.rect.translate(chips.origin),
                plot.id.with(("chip", c.trace)),
                if array {
                    Sense::click()
                } else {
                    Sense::click_and_drag()
                },
            )
            .on_hover_ui(|ui| c.tooltip(ui));
        #[cfg(test)]
        plot.seen
            .1
            .push((c.rect.translate(chips.origin), c.dot_rect(chips.origin)));
        if r.drag_started() && !array {
            plot.chip_drag = Some(c.trace);
        }
        let x = ui.interact(
            c.close_rect(chips.origin),
            plot.id.with(("chip-x", c.trace)),
            Sense::click(),
        );
        // The colour dot: a menu of the value axes.
        let dot = (!array).then(|| {
            ui.interact(
                c.dot_rect(chips.origin),
                plot.id.with(("chip-dot", c.trace)),
                Sense::click(),
            )
            .on_hover_text("Choose its value axis")
        });
        if let Some(dot) = &dot {
            egui::Popup::menu(dot).show(|ui| plot.axis_menu(ui, c.trace));
        }
        if r.hovered() || x.hovered() || dot.is_some_and(|d| d.hovered()) {
            highlight = Some(c.trace);
        }
        if x.clicked() {
            remove = Some(c.trace);
        } else if r.clicked() {
            toggle = Some(c.trace);
        }
    }
    if let Some(id) = toggle
        && let Some(t) = plot.traces.iter_mut().find(|t| t.id == id)
    {
        t.visible = !t.visible;
    }
    if let Some(id) = remove {
        plot.remove_trace(id);
        highlight = None;
    }
    // Only a drawn trace is picked out: visible, not an array, on an axis.
    plot.highlight = highlight.filter(|id| {
        (plot.traces.iter()).any(|t| t.id == *id && t.visible && !t.is_array() && t.axis.is_some())
    });
}

/// The quiet words after a chip's value, saying why it has no line or no
/// history: an array (not plotted), a PV the archiver does not have (live
/// only), or an archive with nothing drawable in range. `no_value`: the
/// chip shows "—".
fn marks(
    archiver: bool,
    fetch: &FetchState,
    history: Option<&History>,
    array: bool,
    no_value: bool,
) -> Vec<&'static str> {
    let mut marks = Vec::new();
    if array {
        marks.push("not plotted");
    }
    if archiver && fetch.not_archived {
        marks.push("live only");
    } else if array {
        // Its history is not kept: nothing more to say.
    } else if no_value && let Some(h) = history {
        if h.samples.is_empty() {
            marks.push("no data");
        } else if h.kind == archiver::ValueKind::Text {
            // Archived, but nothing a plot can draw.
            marks.push("text");
        } else if h.kind == archiver::ValueKind::Other {
            marks.push("not plotted");
        }
    }
    marks
}

/// An array's value, short: its length and first few elements, as
/// `[936] 0, 0, 0, …`. `None` for a value that is not an array.
fn array_text(v: &pv::Value, prec: Option<usize>) -> Option<String> {
    let num = |x: f64| format_value(x, prec, "");
    let (len, mut parts): (usize, Vec<String>) = match v {
        pv::Value::DoubleArray(a) => (
            a.len(),
            a.iter().take(ARRAY_SHOWN).map(|x| num(*x)).collect(),
        ),
        pv::Value::LongArray(a) => (
            a.len(),
            a.iter().take(ARRAY_SHOWN).map(|x| num(*x as f64)).collect(),
        ),
        pv::Value::StrArray(a) => (a.len(), a.iter().take(ARRAY_SHOWN).cloned().collect()),
        _ => return None,
    };
    if len > ARRAY_SHOWN {
        parts.push("…".into());
    }
    Some(format!("[{len}] {}", parts.join(", ")))
}

/// `text` cut to `max` characters, the last an ellipsis.
fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let keep: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", keep.trim_end())
}

/// `text` cut (with an ellipsis) to fit `width` points in `font`: the
/// most characters that fit, found by bisection. At least "…".
pub(super) fn fit(painter: &egui::Painter, text: &str, font: &FontId, width: f32) -> String {
    let fits = |s: &str| super::render::text_size(painter, s, font).x <= width;
    if fits(text) {
        return text.to_string();
    }
    // `lo` characters fit (one, the ellipsis, is the floor); `hi` don't.
    let (mut lo, mut hi) = (1, text.chars().count());
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if fits(&ellipsize(text, mid)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    ellipsize(text, lo)
}

pub fn paint(plot: &Plot, ui: &egui::Ui, chips: &Chips, _data: &[Vec<Pt>], dark: bool) {
    let painter = ui.painter();
    let v = ui.visuals();
    let font = font();
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let time = ui.input(|i| i.time);
    for c in &chips.chips {
        let Some(t) = plot.traces.iter().find(|t| t.id == c.trace) else {
            continue;
        };
        let rect = c.rect.translate(chips.origin);
        let hovered = pointer.is_some_and(|p| rect.contains(p));
        let fill = if hovered {
            v.widgets.hovered.weak_bg_fill
        } else {
            v.extreme_bg_color
        };
        painter.rect(
            rect,
            HEIGHT / 2.0,
            fill,
            v.widgets.noninteractive.bg_stroke,
            StrokeKind::Inside,
        );
        let color = plot.color(t, dark);
        let dot = Pos2::new(rect.left() + PAD + DOT / 2.0, rect.center().y);
        // Fetching, or with data but not yet on an axis (so not drawn).
        let waiting = t.visible && !t.is_array() && t.axis.is_none() && t.data_since.is_some();
        if t.fetch.in_flight || waiting {
            // A small spinner while fetching.
            let a0 = (time * 6.0) as f32;
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = a0 + i as f32 * 0.4;
                    dot + Vec2::new(a.cos(), a.sin()) * (DOT / 2.0)
                })
                .collect();
            painter.add(egui::Shape::line(pts, Stroke::new(2.0, color)));
            ui.ctx().request_repaint();
        } else if t.visible {
            painter.circle_filled(dot, DOT / 2.0, color);
        } else {
            painter.circle_stroke(dot, DOT / 2.0 - 0.75, Stroke::new(1.5, v.weak_text_color()));
        }
        let name_color = if t.visible {
            v.text_color()
        } else {
            v.weak_text_color()
        };
        let name = painter.text(
            Pos2::new(dot.x + DOT / 2.0 + 6.0, rect.center().y),
            Align2::LEFT_CENTER,
            &c.name,
            font.clone(),
            name_color,
        );
        let value_color = if !t.visible {
            v.weak_text_color()
        } else if let Some(s) = c.severity {
            s
        } else if c.current {
            v.strong_text_color()
        } else {
            v.weak_text_color()
        };
        let value = painter.text(
            Pos2::new(name.right() + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            &c.value,
            font.clone(),
            value_color,
        );
        if let Some(mark) = &c.mark {
            painter.text(
                Pos2::new(value.right() + 6.0, rect.center().y),
                Align2::LEFT_CENTER,
                mark,
                mark_font(),
                v.weak_text_color(),
            );
        }
        let close = c.close_rect(chips.origin);
        if hovered {
            let hot = pointer.is_some_and(|p| close.contains(p));
            if hot {
                painter.circle_filled(close.center(), CLOSE / 2.0, v.widgets.active.weak_bg_fill);
            }
            let s = 3.5;
            let k = Stroke::new(
                1.5,
                if hot {
                    v.strong_text_color()
                } else {
                    v.text_color()
                },
            );
            let m = close.center();
            painter.line_segment([m + Vec2::new(-s, -s), m + Vec2::new(s, s)], k);
            painter.line_segment([m + Vec2::new(s, -s), m + Vec2::new(-s, s)], k);
        } else if t.fetch.error.is_some() {
            painter.text(
                close.center(),
                Align2::CENTER_CENTER,
                "!",
                FontId::proportional(13.0),
                v.warn_fg_color,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_values_are_cut_with_an_ellipsis() {
        assert_eq!(ellipsize("298.1 mA", 32), "298.1 mA");
        assert_eq!(ellipsize("abcdefgh", 5), "abcd…");
        assert_eq!(ellipsize("abc defgh", 5), "abc…");
        assert_eq!(ellipsize("abcdefgh", 5).chars().count(), 5);
    }

    #[test]
    fn fit_keeps_the_most_that_fits() {
        let ctx = egui::Context::default();
        let text = "a long status message that goes on and on and on";
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter();
            let size = |s: &str| super::super::render::text_size(painter, s, &font()).x;
            assert_eq!(fit(painter, text, &font(), 1e4), text);
            for width in [0.0, 20.0, 60.0, 150.0] {
                let shown = fit(painter, text, &font(), width);
                let n = shown.chars().count();
                assert!(shown.ends_with('…'), "{shown}");
                assert!(n == 1 || size(&shown) <= width, "{shown} in {width}");
                // Nothing longer would fit.
                let longest = (1..text.len())
                    .map(|k| ellipsize(text, k))
                    .filter(|s| size(s) <= width)
                    .map(|s| s.chars().count())
                    .max();
                assert!(longest.is_none_or(|m| m <= n), "{shown} in {width}");
            }
        });
        out.textures_delta.clear();
    }

    #[test]
    fn arrays_show_their_length_and_first_elements() {
        let wave = pv::Value::DoubleArray(vec![0.0; 936].into());
        assert_eq!(array_text(&wave, Some(0)).unwrap(), "[936] 0, 0, 0, …");
        let short = pv::Value::LongArray(vec![1, 2].into());
        assert_eq!(array_text(&short, Some(0)).unwrap(), "[2] 1, 2");
        let one = pv::Value::DoubleArray(vec![4.0].into());
        assert_eq!(array_text(&one, Some(1)).unwrap(), "[1] 4.0");
        assert_eq!(array_text(&pv::Value::Double(1.0), None), None);
    }

    #[test]
    fn marks_say_why_a_trace_has_no_line_or_history() {
        let fetch = FetchState::default();
        let not_archived = FetchState {
            not_archived: true,
            ..FetchState::default()
        };
        let history = |kind, n| History {
            kind,
            samples: vec![archiver::Sample::raw(0.0, 1.0); n],
            ..History::default()
        };
        // An unarchived array.
        assert_eq!(
            marks(true, &not_archived, None, true, false),
            ["not plotted", "live only"]
        );
        // An unarchived scalar; no archiver at all says nothing per chip.
        assert_eq!(
            marks(true, &not_archived, None, false, false),
            ["live only"]
        );
        assert!(marks(false, &not_archived, None, false, false).is_empty());
        // Archived, but nothing in range.
        let empty = history(archiver::ValueKind::Scalar, 0);
        assert_eq!(marks(true, &fetch, Some(&empty), false, true), ["no data"]);
        let text = history(archiver::ValueKind::Text, 3);
        assert_eq!(marks(true, &fetch, Some(&text), false, true), ["text"]);
        // An archived array.
        // An archived array (its samples not kept), connected or not.
        let wave = history(archiver::ValueKind::Waveform, 0);
        for no_value in [false, true] {
            assert_eq!(
                marks(true, &fetch, Some(&wave), true, no_value),
                ["not plotted"]
            );
        }
        // An ordinary trace.
        let scalar = history(archiver::ValueKind::Scalar, 3);
        assert!(marks(true, &fetch, Some(&scalar), false, false).is_empty());
    }
}
