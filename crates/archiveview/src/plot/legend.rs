//! The legend: a wrapped row of chips under the plot, one per trace: a
//! colour dot, the name, the latest value. Click to hide or show, ✕ to
//! remove, hover to pick the trace out.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use super::series::Pt;
use super::{Plot, format_value, precision, units};

const HEIGHT: f32 = 26.0;
const GAP: f32 = 6.0;
const PAD: f32 = 10.0;
const DOT: f32 = 8.0;
const CLOSE: f32 = 14.0;

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
    note: String,
}

impl Chip {
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

pub fn layout(plot: &Plot, ui: &egui::Ui, width: f32) -> Chips {
    let painter = ui.painter();
    let font = font();
    let mut chips = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    for t in &plot.traces {
        let state = t.pv.state();
        let prec = precision(t, &state);
        let egu = units(t);
        let archived_last = t.history.as_ref().and_then(|h| {
            h.samples
                .iter()
                .rev()
                .find(|s| s.val.is_finite())
                .map(|s| s.val)
        });
        let (value, current) = if state.connected && state.value.as_f64().is_some() {
            (state.format(prec.map(|p| p as i32), true), true)
        } else if let Some(v) = archived_last {
            (format_value(v, prec, &egu), false)
        } else {
            ("—".to_string(), false)
        };
        let mut note = vec![t.name.clone()];
        if state.connected {
            note.push("Live: connected".into());
        } else if let Some(e) = &state.error {
            note.push(format!("Live: {e}"));
        } else {
            note.push("Live: not connected".into());
        }
        if plot.archiver.is_some() {
            note.push(if t.archive_name().is_none() {
                "Archive: none for this kind of PV".to_string()
            } else if t.fetch.not_archived {
                "Archive: not archived (live only)".to_string()
            } else if let Some(e) = &t.fetch.error {
                format!("Archive: {e}")
            } else if let Some(h) = &t.history {
                if h.samples.is_empty() {
                    "Archive: no data in range".to_string()
                } else {
                    let what = match h.kind {
                        archiver::ValueKind::Scalar if h.binned => "bins (mean, min…max)",
                        archiver::ValueKind::Scalar => "samples",
                        archiver::ValueKind::Waveform => "samples (element 0 plotted)",
                        archiver::ValueKind::Text => "text samples (not plotted)",
                        archiver::ValueKind::Other => "samples (not plottable)",
                    };
                    format!("Archive: {} {what}", h.samples.len())
                }
            } else {
                "Archive: fetching…".to_string()
            });
        }
        note.push("Click to hide or show; ✕ to remove".into());
        let mut name = t.label();
        if name.chars().count() > 48 {
            name = format!("{}…", name.chars().take(47).collect::<String>());
        }
        let w = PAD
            + DOT
            + 6.0
            + super::render::text_size(painter, &name, &font).x
            + 8.0
            + super::render::text_size(painter, &value, &font).x
            + 6.0
            + CLOSE
            + PAD / 2.0;
        let w = w.min(width);
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
            note: note.join("\n"),
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
    let mut highlight = None;
    let mut toggle = None;
    let mut remove = None;
    for c in &chips.chips {
        let r = ui
            .interact(
                c.rect.translate(chips.origin),
                plot.id.with(("chip", c.trace)),
                Sense::click(),
            )
            .on_hover_text(&c.note);
        let x = ui.interact(
            c.close_rect(chips.origin),
            plot.id.with(("chip-x", c.trace)),
            Sense::click(),
        );
        if r.hovered() || x.hovered() {
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
        plot.traces.retain(|t| t.id != id);
        highlight = None;
    }
    // Only a visible trace is picked out.
    plot.highlight = highlight.filter(|id| plot.traces.iter().any(|t| t.id == *id && t.visible));
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
        if t.fetch.in_flight {
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
        painter.text(
            Pos2::new(name.right() + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            &c.value,
            font.clone(),
            value_color,
        );
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
