//! What floats over the plot: the crosshair readout, the empty state and
//! the status line.

use egui::epaint::Shadow;
use egui::{Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, Vec2};

use super::render::Frame;
use super::series::{Pt, at_time, nearest};
use super::timeaxis::readout_time;
use super::{Plot, format_value, precision, units};

/// Snap the hairline to a sample of the first visible trace this close
/// (points).
const SNAP_PX: f32 = 8.0;

/// Which pane a crosshair is drawn in, and what it has there.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    /// The pane (index).
    pub pane: usize,
    /// The pointer is in it: its card follows the pointer and has the
    /// time.
    pub pointer: bool,
    /// It has the time labels, where the time tag goes.
    pub time_axis: bool,
}

/// Where the crosshair goes, from pane `k` (with frame `f`): the pointer,
/// snapped to a nearby sample of the pane's first visible trace, and the
/// time there. `None` when the pointer is not over the pane's data.
pub fn hairline(
    plot: &Plot,
    ui: &egui::Ui,
    k: usize,
    f: &Frame,
    data: &[Vec<Pt>],
) -> Option<(Pos2, f64)> {
    let mut p = ui.input(|i| i.pointer.hover_pos())?;
    if !f.data.contains(p) || !ui.rect_contains_pointer(f.outer) {
        return None;
    }
    let mut t = f.time_at(p.x);
    let first = (plot.traces.iter().zip(data))
        .find(|(trace, pts)| trace.visible && !pts.is_empty() && plot.home(trace) == k);
    if let Some(i) = first.and_then(|(_, pts)| nearest(pts, t).map(|i| pts[i].t))
        && (f.xs.pos(i) - p.x).abs() <= SNAP_PX
    {
        t = i;
        p.x = f.xs.pos(i);
    }
    Some((p, t))
}

/// In the pane at `place` (with frame `f`): a vertical hairline at
/// `hair` (from [`hairline`]), a dot on each of its traces where its line
/// is, a time tag on the time axis, and a floating card of the values the
/// lines show there ("—" for a trace with none).
pub fn crosshair(
    plot: &Plot,
    ui: &egui::Ui,
    f: &Frame,
    data: &[Vec<Pt>],
    dark: bool,
    (p, t): (Pos2, f64),
    place: Place,
) {
    let painter = ui.painter_at(f.outer);
    let v = ui.visuals();
    painter.line_segment(
        [
            Pos2::new(p.x, f.data.top()),
            Pos2::new(p.x, f.data.bottom()),
        ],
        Stroke::new(1.0, v.text_color().gamma_multiply(0.35)),
    );
    if place.time_axis {
        time_tag(&painter, f, p.x, t, v);
    }
    let pane = &plot.panes[place.pane];

    struct Row {
        color: Color32,
        name: String,
        value: String,
        range: Option<String>,
    }
    let mut rows = Vec::new();
    for (n, (trace, pts)) in plot.traces.iter().zip(data).enumerate() {
        // Arrays are not plotted: their chip has the value. Each pane
        // lists its own traces (those not settled, the first).
        if !trace.visible || trace.is_array() || plot.home(trace) != place.pane {
            continue;
        }
        let color = plot.color(trace, dark);
        let Some(i) = at_time(pts, t) else {
            rows.push(Row {
                color,
                name: trace.label(),
                value: "—".into(),
                range: None,
            });
            continue;
        };
        let pt = pts[i];
        // A held raw value is where the hairline crosses its step; a bin
        // mean at the bin's centre.
        let x = if pt.step { p.x } else { f.xs.pos(pt.t) };
        // A trace not yet on an axis is not drawn: no dot.
        let axis = pane.axis_of.get(n).copied().flatten();
        let at = Pos2::new(x, axis.map_or(f32::NAN, |k| f.scale(k).pos(pt.v)));
        if axis.is_some() && pt.v.is_finite() && f.data.expand(4.0).contains(at) {
            painter.circle(at, 4.0, color, Stroke::new(1.5, v.extreme_bg_color));
        }
        let state = trace.pv.state();
        let prec = precision(trace, &state);
        let egu = units(trace);
        rows.push(Row {
            color,
            name: trace.label(),
            // An enum's state label, as its chip shows, when the index has one.
            value: match state.labels.get(pt.v as usize) {
                Some(label) if !pt.is_bin() && pt.v >= 0.0 && pt.v.fract() == 0.0 => label.clone(),
                _ => format_value(pt.v, prec, &egu),
            },
            range: pt.is_bin().then(|| {
                format!(
                    "{} … {}",
                    format_value(pt.lo, prec, ""),
                    format_value(pt.hi, prec, "")
                )
            }),
        });
    }
    if rows.is_empty() {
        return;
    }

    // The card: the time, then a row per trace.
    let font = FontId::proportional(12.5);
    let small = FontId::proportional(11.0);
    // The time once, in the pointer's pane.
    let header = place.pointer.then(|| {
        painter.layout_no_wrap(
            readout_time(t, f.xs.hi - f.xs.lo),
            small.clone(),
            v.weak_text_color(),
        )
    });
    let header_h = header.as_ref().map_or(0.0, |h| h.size().y + 6.0);
    let header_w = header.as_ref().map_or(0.0, |h| h.size().x);
    let galleys: Vec<_> = rows
        .iter()
        .map(|r| {
            let name = painter.layout_no_wrap(r.name.clone(), font.clone(), v.text_color());
            let value =
                painter.layout_no_wrap(r.value.clone(), font.clone(), v.strong_text_color());
            let range = r
                .range
                .as_ref()
                .map(|s| painter.layout_no_wrap(s.clone(), small.clone(), v.weak_text_color()));
            (name, value, range)
        })
        .collect();
    let name_w = galleys.iter().map(|g| g.0.size().x).fold(0.0, f32::max);
    let value_w = galleys
        .iter()
        .map(|g| g.1.size().x.max(g.2.as_ref().map_or(0.0, |r| r.size().x)))
        .fold(0.0, f32::max);
    let row_h = 18.0;
    let pad = 10.0;
    let width = (pad + 14.0 + name_w + 16.0 + value_w + pad).max(header_w + 2.0 * pad);
    let height = pad
        + header_h
        + galleys
            .iter()
            .map(|g| row_h + if g.2.is_some() { 13.0 } else { 0.0 })
            .sum::<f32>()
        + pad
        - 4.0;
    let mut x = p.x + 16.0;
    if x + width > f.outer.right() - 4.0 {
        x = p.x - 16.0 - width;
    }
    // Above the time axis strip, so it never covers the crosshair's own
    // time tag there; by the pointer, or in the middle of another pane.
    let mid = if place.pointer {
        p.y
    } else {
        f.data.center().y
    };
    let y = (mid - height / 2.0).clamp(
        f.outer.top() + 4.0,
        (f.data.bottom() - height - 4.0).max(f.outer.top() + 4.0),
    );
    let card = Rect::from_min_size(
        Pos2::new(x.max(f.outer.left() + 4.0), y),
        Vec2::new(width, height),
    );
    let shadow = Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 90 } else { 40 }),
    };
    let top = ui.painter().clone().with_layer_id(egui::LayerId::new(
        egui::Order::Tooltip,
        plot.id.with(("crosshair", place.pane)),
    ));
    top.add(shadow.as_shape(card, 8.0));
    top.rect(
        card,
        8.0,
        v.window_fill,
        v.window_stroke,
        egui::StrokeKind::Inside,
    );
    let mut cy = card.top() + pad;
    if let Some(header) = header {
        top.galley(
            Pos2::new(card.left() + pad, cy),
            header,
            v.weak_text_color(),
        );
    }
    cy += header_h;
    for (row, (name, value, range)) in rows.iter().zip(galleys) {
        let mid = cy + row_h / 2.0 - 2.0;
        top.circle_filled(Pos2::new(card.left() + pad + 4.0, mid), 4.0, row.color);
        top.galley(
            Pos2::new(card.left() + pad + 14.0, mid - name.size().y / 2.0),
            name,
            v.text_color(),
        );
        let vx = card.right() - pad - value.size().x;
        top.galley(
            Pos2::new(vx, mid - value.size().y / 2.0),
            value,
            v.strong_text_color(),
        );
        cy += row_h;
        if let Some(range) = range {
            let rx = card.right() - pad - range.size().x;
            top.galley(Pos2::new(rx, cy - 4.0), range, v.weak_text_color());
            cy += 13.0;
        }
    }
}

/// The time tag's text at `t`.
fn tag_text(f: &Frame, t: f64) -> String {
    super::timeaxis::tag_time(t, f.xs.hi - f.xs.lo)
}

fn tag_font() -> FontId {
    FontId::proportional(11.5)
}

/// Where the time tag for a hairline at `x` (time `t`) goes: the tick
/// labels it covers are left out.
pub fn time_tag_rect(painter: &egui::Painter, f: &Frame, x: f32, t: f64) -> Rect {
    let size =
        super::render::text_size(painter, &tag_text(f, t), &tag_font()) + Vec2::new(12.0, 4.0);
    let left = (x - size.x / 2.0).clamp(f.outer.left() + 2.0, f.outer.right() - size.x - 2.0);
    Rect::from_min_size(Pos2::new(left, f.data.bottom() + 4.0), size)
}

/// A small rounded tag on the time axis under the hairline, in place of
/// the tick labels there, with the time to the second (or millisecond
/// when zoomed in).
fn time_tag(painter: &egui::Painter, f: &Frame, x: f32, t: f64, v: &egui::Visuals) {
    let galley = painter.layout_no_wrap(tag_text(f, t), tag_font(), v.strong_text_color());
    let tag = time_tag_rect(painter, f, x, t);
    let size = tag.size();
    painter.rect(
        tag,
        size.y / 2.0,
        v.widgets.inactive.bg_fill,
        Stroke::new(1.0, v.text_color().gamma_multiply(0.25)),
        egui::StrokeKind::Inside,
    );
    painter.galley(tag.min + Vec2::new(6.0, 2.0), galley, v.strong_text_color());
}

/// With no traces: a quiet prompt in the middle of the plot, pointing at
/// the search box.
pub fn empty_state(painter: &egui::Painter, f: &Frame, v: &egui::Visuals) {
    let c = f.data.center();
    let title = painter.text(
        c - Vec2::new(0.0, 10.0),
        Align2::CENTER_CENTER,
        "Type a PV name to start",
        FontId::proportional(22.0),
        v.weak_text_color(),
    );
    painter.text(
        c + Vec2::new(0.0, 20.0),
        Align2::CENTER_CENTER,
        "/ to search  ·  drag to pan  ·  scroll to zoom  ·  double-click to reset",
        FontId::proportional(13.0),
        v.weak_text_color().gamma_multiply(0.8),
    );
    // A curved arrow from the prompt up toward the search box (top left).
    let from = Pos2::new(title.left() - 12.0, title.center().y);
    let to = Pos2::new(f.outer.left() + 60.0, f.outer.top() + 12.0);
    if to.x < from.x - 40.0 && to.y < from.y - 40.0 {
        let ctrl = Pos2::new(to.x + 10.0, from.y);
        let pts: Vec<Pos2> = (0..=24)
            .map(|i| {
                let s = i as f32 / 24.0;
                let a = from.lerp(ctrl, s);
                let b = ctrl.lerp(to, s);
                a.lerp(b, s)
            })
            .collect();
        let stroke = Stroke::new(1.5, v.weak_text_color().gamma_multiply(0.6));
        painter.add(Shape::dashed_line(&pts, stroke, 5.0, 4.0));
        let dir = (to - pts[pts.len() - 3]).normalized();
        let side = Vec2::new(-dir.y, dir.x);
        painter.line_segment([to, to - dir * 8.0 + side * 5.0], stroke);
        painter.line_segment([to, to - dir * 8.0 - side * 5.0], stroke);
    }
}

/// One quiet line top left of the data: fetching, an archiver error, or
/// that there is no archiver.
pub fn status(plot: &Plot, ui: &egui::Ui, f: &Frame) {
    if plot.traces.is_empty() {
        return;
    }
    let v = ui.visuals();
    let (text, color, busy) = if plot.fetcher.busy() {
        ("fetching archive…".to_string(), v.weak_text_color(), true)
    } else if let Some((name, e)) = plot
        .traces
        .iter()
        .find_map(|t| t.fetch.error.as_ref().map(|e| (t.label(), e)))
    {
        (format!("archiver: {name}: {e}"), v.warn_fg_color, false)
    } else if let Some(name) = raw_capped(plot) {
        (
            format!(
                "{}: raw history only for the last {}",
                name.label(),
                super::timeaxis::format_span(super::fetch::RAW_MAX_SPAN)
            ),
            v.weak_text_color(),
            false,
        )
    } else if plot.archiver.is_none() {
        (
            "live only: no archiver".to_string(),
            v.weak_text_color(),
            false,
        )
    } else {
        return;
    };
    let painter = ui.painter_at(f.data);
    let mut pos = f.data.left_top() + Vec2::new(10.0, 6.0);
    if busy {
        // A small turning arc.
        let a0 = (ui.input(|i| i.time) * 6.0) as f32;
        let c = pos + Vec2::new(5.0, 7.0);
        let pts: Vec<Pos2> = (0..=10)
            .map(|i| {
                let a = a0 + i as f32 * 0.45;
                c + Vec2::new(a.cos(), a.sin()) * 5.0
            })
            .collect();
        painter.add(Shape::line(pts, Stroke::new(1.5, color)));
        pos.x += 16.0;
    }
    let max = (f.data.width() - 40.0).max(40.0);
    let font = FontId::proportional(12.0);
    let shown = super::legend::fit(&painter, &text, &font, max);
    painter.text(pos, Align2::LEFT_TOP, shown, font, color);
}

/// The plotted trace whose history is cut to the last
/// [`super::fetch::RAW_MAX_SPAN`] in the window shown, if any: one
/// fetched raw (a string or enum). Arrays are not plotted, so they don't
/// count.
pub(super) fn raw_capped(plot: &Plot) -> Option<&super::series::Trace> {
    if plot.archiver.is_none() || plot.window.span <= super::fetch::RAW_MAX_SPAN {
        return None;
    }
    plot.traces.iter().find(|t| {
        t.visible && t.fetch.raw_only == Some(true) && t.archive_name().is_some() && !t.is_array()
    })
}
