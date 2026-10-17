//! Painting the plot area: grid, envelopes, lines, axes and the now marker.
//! Adapted from bobview's `trend::draw_plot` and `plot.rs` helpers
//! (`autoscale`, `decimate`, `line`, `vertical_text`), restyled: no frame,
//! subtle grid, muted labels.

use std::f32::consts::FRAC_PI_2;

use egui::epaint::{self, Mesh};
use egui::{Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, Vec2};

use super::series::Pt;
use super::timeaxis::{time_label, time_ticks};
use crate::axis::{self, Scale};

/// Where things go in a plot rect, for painting and for hit testing.
pub struct Frame {
    /// The plot rect.
    pub outer: Rect,
    /// Where data is drawn.
    pub data: Rect,
    pub xs: Scale,
    /// The first value axis' scale: the grid follows it.
    pub ys: Scale,
    /// The value axes, in order: alternately left and right of the data,
    /// each pair further out than the one before.
    axes: Vec<Axis>,
    font: FontId,
}

/// A value axis beside the data.
struct Axis {
    scale: Scale,
    labels: Vec<(f64, String)>,
    title: String,
    color: Option<Color32>,
    tint_labels: bool,
    dots: Vec<Color32>,
    highlight: bool,
    /// The strip it takes beside the data: labels and title.
    rect: Rect,
    /// Where its title goes, at the outer edge of `rect`.
    title_rect: Rect,
    label_w: f32,
    /// Right of the data (labels left-aligned, title outside them).
    right: bool,
}

/// A value axis to lay out.
pub struct AxisSpec {
    pub lo: f64,
    pub hi: f64,
    pub log: bool,
    /// Its name, up the side.
    pub title: String,
    /// The colour of its name (`None`: muted text): its trace's when it
    /// has one.
    pub color: Option<Color32>,
    /// Its labels in `color` too (with several axes, to tell them apart).
    pub tint_labels: bool,
    /// With several traces, a small dot in each one's colour by the name.
    pub dots: Vec<Color32>,
    /// Its traces hold whole numbers (enum states): ticks at whole
    /// numbers only.
    pub whole: bool,
    /// Picked out (clicked): its strip is tinted.
    pub highlight: bool,
}

impl Default for AxisSpec {
    fn default() -> Self {
        Self {
            lo: 0.0,
            hi: 1.0,
            log: false,
            title: String::new(),
            color: None,
            tint_labels: false,
            dots: Vec::new(),
            whole: false,
            highlight: false,
        }
    }
}

impl Frame {
    /// The number of value axes.
    pub fn axis_count(&self) -> usize {
        self.axes.len()
    }

    /// The strip value axis `i` takes beside the data (labels and name),
    /// for dragging and scrolling its values.
    pub fn axis_rect(&self, i: usize) -> Option<Rect> {
        self.axes.get(i).map(|a| a.rect)
    }

    /// Where value axis `i`'s name is (the outer part of its strip), for
    /// renaming it.
    pub fn title_rect(&self, i: usize) -> Option<Rect> {
        self.axes.get(i).map(|a| a.title_rect)
    }

    /// Pick out value axis `i` (a chip dragged over it).
    pub fn set_highlight(&mut self, i: usize) {
        if let Some(a) = self.axes.get_mut(i) {
            a.highlight = true;
        }
    }

    /// The value axis whose strip holds `p`.
    pub fn axis_at(&self, p: Pos2) -> Option<usize> {
        self.axes.iter().position(|a| a.rect.contains(p))
    }

    /// The scale of value axis `axis` (the first's for any past the last).
    pub fn scale(&self, axis: usize) -> &Scale {
        self.axes.get(axis).map_or(&self.ys, |a| &a.scale)
    }

    /// The time at screen `x`.
    pub fn time_at(&self, x: f32) -> f64 {
        let f = ((x - self.xs.from) / (self.xs.to - self.xs.from)) as f64;
        self.xs.lo + f * (self.xs.hi - self.xs.lo)
    }

    /// The value at screen `y` on value axis `axis`.
    pub fn value_on(&self, axis: usize, y: f32) -> f64 {
        let s = self.scale(axis);
        let f = ((y - s.from) / (s.to - s.from)) as f64;
        if s.log && s.lo > 0.0 {
            let (l, h) = (s.lo.log10(), s.hi.log10());
            10f64.powf(l + f * (h - l))
        } else {
            s.lo + f * (s.hi - s.lo)
        }
    }
}

const PAD: f32 = 8.0;
const TICK_GAP: f32 = 6.0;

/// Lay out the plot in `rect` for times `from..to` and the value `axes`
/// (at least one is drawn: a blank 0..1 without any).
///
/// The axes alternate sides, nearest the data first: the first left, the
/// second right, the third left of the first, the fourth right of the
/// second. Each side then holds at most two of the usual four, every
/// axis' labels sit as close to the data as they can, and the data stays
/// centred; the common pair reads as the familiar two-sided chart.
pub fn frame(
    painter: &egui::Painter,
    rect: Rect,
    (from, to): (f64, f64),
    axes: &[AxisSpec],
    font: &FontId,
) -> Frame {
    let label_h = text_size(painter, "0", font).y;
    let bottom = rect.bottom() - PAD - label_h - TICK_GAP;
    let top = rect.top() + PAD + label_h / 2.0;
    let blank = [AxisSpec::default()];
    let specs = if axes.is_empty() { &blank[..] } else { axes };
    let mut list: Vec<Axis> = specs
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let scale = Scale {
                lo: a.lo,
                hi: a.hi,
                log: a.log,
                from: bottom,
                to: top,
            };
            let (ticks, step) = if a.whole {
                scale.whole_ticks(label_h * 3.0)
            } else {
                scale.ticks(label_h * 3.0)
            };
            let labels: Vec<(f64, String)> =
                ticks.iter().map(|v| (*v, axis::label(*v, step))).collect();
            let label_w = labels
                .iter()
                .map(|(_, l)| text_size(painter, l, font).x)
                .fold(text_size(painter, "0000", font).x, f32::max);
            Axis {
                scale,
                labels,
                title: a.title.clone(),
                color: a.color,
                tint_labels: a.tint_labels,
                dots: a.dots.clone(),
                highlight: a.highlight,
                rect: Rect::NOTHING,
                title_rect: Rect::NOTHING,
                label_w,
                right: i % 2 == 1,
            }
        })
        .collect();
    // Each strip: a gap, the name (when it has one), a gap, the labels and
    // the tick gap, mirrored on the right.
    let width = |a: &Axis| {
        let title = if a.title.is_empty() {
            0.0
        } else {
            label_h + PAD / 2.0
        };
        PAD / 2.0 + title + a.label_w + TICK_GAP
    };
    let sum = |right: bool| -> f32 { list.iter().filter(|a| a.right == right).map(width).sum() };
    let left = rect.left() + PAD / 2.0 + sum(false);
    let right = if list.iter().any(|a| a.right) {
        rect.right() - PAD / 2.0 - sum(true)
    } else {
        rect.right() - PAD * 2.0
    };
    let data = Rect::from_min_max(
        Pos2::new(left, top),
        Pos2::new(right.max(left + 1.0), bottom),
    );
    let (mut l, mut r) = (data.left(), data.right());
    for a in &mut list {
        let w = width(a);
        let title = if a.title.is_empty() { 0.0 } else { label_h };
        (a.rect, a.title_rect) = if a.right {
            r += w;
            let t = r - PAD / 2.0;
            (
                Rect::from_x_y_ranges(r - w..=r, data.y_range()),
                Rect::from_x_y_ranges(t - title..=t, data.y_range()),
            )
        } else {
            l -= w;
            let t = l + PAD / 2.0;
            (
                Rect::from_x_y_ranges(l..=l + w, data.y_range()),
                Rect::from_x_y_ranges(t..=t + title, data.y_range()),
            )
        };
    }
    let xs = Scale {
        lo: from,
        hi: to,
        log: false,
        from: data.left(),
        to: data.right(),
    };
    Frame {
        outer: rect,
        data,
        xs,
        ys: list[0].scale,
        axes: list,
        font: font.clone(),
    }
}

/// A trace to draw.
pub struct Line<'a> {
    pub pts: &'a [Pt],
    pub color: Color32,
    pub width: f32,
    /// Its value axis (an index into the frame's axes).
    pub axis: usize,
}

/// Paint the background, grid, labels and `lines`, leaving out any time
/// label under `cover` (the crosshair's time tag).
pub fn paint(
    painter: &egui::Painter,
    f: &Frame,
    lines: &[Line],
    visuals: &egui::Visuals,
    cover: Option<Rect>,
) {
    let painter = &painter.with_clip_rect(painter.clip_rect().intersect(f.outer));
    painter.rect_filled(f.outer, 10.0, visuals.extreme_bg_color);
    let text = visuals.text_color();
    let weak = visuals.weak_text_color();
    let grid = Stroke::new(1.0, text.gamma_multiply(0.07));
    let data = f.data;
    if data.width() < 4.0 || data.height() < 4.0 {
        return;
    }

    // Time ticks, labels measured so none overlap.
    let sample_w = text_size(painter, "00:00", &f.font).x;
    let max_ticks = (data.width() / (sample_w + 48.0)).floor().max(2.0) as usize;
    let (t_ticks, t_step) = time_ticks(f.xs.lo, f.xs.hi, max_ticks, &chrono::Local);
    for t in &t_ticks {
        let x = f.xs.pos(*t).round() + 0.5;
        painter.line_segment(
            [Pos2::new(x, data.top()), Pos2::new(x, data.bottom())],
            grid,
        );
    }
    for (v, _) in &f.axes[0].labels {
        let y = f.ys.pos(*v).round() + 0.5;
        painter.line_segment(
            [Pos2::new(data.left(), y), Pos2::new(data.right(), y)],
            grid,
        );
    }
    // A baseline under the data.
    painter.line_segment(
        [data.left_bottom(), data.right_bottom()],
        Stroke::new(1.0, text.gamma_multiply(0.18)),
    );

    let clipped = painter.with_clip_rect(painter.clip_rect().intersect(data.expand(1.0)));
    for line in lines {
        draw_line(&clipped, f, line);
    }

    // Time labels below, the first with its date.
    let mut right_of = f32::NEG_INFINITY;
    for (i, t) in t_ticks.iter().enumerate() {
        let x = f.xs.pos(*t);
        let label = time_label(*t, t_step, i == 0);
        let galley = painter.layout_no_wrap(label, f.font.clone(), weak);
        let w = galley.size().x;
        let left = (x - w / 2.0).clamp(f.outer.left() + 2.0, f.outer.right() - w - 2.0);
        if left < right_of + 10.0 {
            continue;
        }
        right_of = left + w;
        let label = Rect::from_min_size(Pos2::new(left, data.bottom() + TICK_GAP), galley.size());
        if cover.is_some_and(|c| c.expand2(Vec2::new(4.0, 0.0)).intersects(label)) {
            continue;
        }
        painter.galley(Pos2::new(left, data.bottom() + TICK_GAP), galley, weak);
    }
    for a in &f.axes {
        paint_axis(painter, f, a, visuals);
    }
}

/// A value axis: its labels beside the data and its name up the outer
/// side, in its trace's colour when it has one.
fn paint_axis(painter: &egui::Painter, f: &Frame, a: &Axis, visuals: &egui::Visuals) {
    let weak = visuals.weak_text_color();
    let color = a.color.filter(|_| a.tint_labels).unwrap_or(weak);
    if a.highlight {
        let accent = visuals.selection.bg_fill;
        painter.rect_filled(
            a.rect.expand2(Vec2::new(0.0, 4.0)),
            4.0,
            accent.gamma_multiply(0.18),
        );
    }
    for (v, label) in &a.labels {
        let y = a.scale.pos(*v);
        // The top one on the right would sit under the "now" tag.
        if a.right && y < f.data.top() + 4.0 {
            continue;
        }
        let (x, align) = if a.right {
            (a.rect.left() + TICK_GAP, Align2::LEFT_CENTER)
        } else {
            (a.rect.right() - TICK_GAP, Align2::RIGHT_CENTER)
        };
        painter.text(Pos2::new(x, y), align, label, f.font.clone(), color);
    }
    if a.title.is_empty() {
        return;
    }
    let size = text_size(painter, &a.title, &f.font);
    let x = a.title_rect.center().x;
    let mid = f.data.center().y;
    let title_color = match a.color {
        Some(c) => c,
        None if a.highlight => visuals.strong_text_color(),
        None => weak,
    };
    vertical_text(
        painter,
        Pos2::new(x, mid),
        &a.title,
        f.font.clone(),
        title_color,
    );
    // Its traces' colours, above the name (which reads upwards).
    for (k, c) in a.dots.iter().enumerate() {
        let y = mid - size.x / 2.0 - 8.0 - k as f32 * 9.0;
        if y > f.data.top() + 4.0 {
            painter.circle_filled(Pos2::new(x, y), 3.0, *c);
        }
    }
}

/// A thin dashed line at `now` with a small tag, when it is in view. In
/// neutral grey: red is a trace colour (and an alarm), and the Live pill's
/// dot alone says the plot follows now.
pub fn now_marker(painter: &egui::Painter, f: &Frame, now: f64, v: &egui::Visuals, font: &FontId) {
    if now < f.xs.lo || now > f.xs.hi + (f.xs.hi - f.xs.lo) * 1e-3 {
        return;
    }
    let color = v.weak_text_color();
    let x = f.xs.pos(now).min(f.data.right());
    let stroke = Stroke::new(1.0, color.gamma_multiply(0.7));
    painter.extend(Shape::dashed_line(
        &[Pos2::new(x, f.data.top()), Pos2::new(x, f.data.bottom())],
        stroke,
        4.0,
        4.0,
    ));
    let galley = painter.layout_no_wrap("now".into(), font.clone(), color);
    let size = galley.size() + Vec2::new(10.0, 2.0);
    let tag = Rect::from_min_size(
        Pos2::new(
            (x - size.x / 2.0).min(f.outer.right() - size.x - 2.0),
            f.outer.top() + 2.0,
        ),
        size,
    );
    painter.rect(
        tag,
        size.y / 2.0,
        v.extreme_bg_color,
        Stroke::new(1.0, color.gamma_multiply(0.5)),
        egui::StrokeKind::Inside,
    );
    painter.galley(tag.min + Vec2::new(5.0, 1.0), galley, color);
}

fn draw_line(painter: &egui::Painter, f: &Frame, line: &Line) {
    let ys = f.scale(line.axis);
    let ok = |v: f64| v.is_finite() && (!ys.log || v > 0.0);
    // Far-off points are pulled in so the maths stays finite.
    let bounds = f.data.expand(f.data.width().max(f.data.height()) * 4.0);
    let x = |t: f64| f.xs.pos(t).clamp(bounds.left(), bounds.right());
    let y = |v: f64| ys.pos(v).clamp(bounds.top(), bounds.bottom());

    // Runs between gaps, each raw value held to the gap that ends it.
    let pts = hold_to_gaps(line.pts, ok);
    for run in pts.split(|p| !ok(p.v)).filter(|r| !r.is_empty()) {
        // The min..max envelope of bins, as quads between neighbours.
        if run.iter().any(Pt::is_bin) {
            let fill = line.color.gamma_multiply(0.22);
            let mut mesh = Mesh::default();
            // One (x, lo, hi) per pixel column: quads narrower than a pixel
            // would shimmer into moiré.
            let cols: Vec<(f32, f32, f32)> = run
                .iter()
                .map(|p| {
                    let (lo, hi) = if ok(p.lo) && ok(p.hi) {
                        (p.lo, p.hi)
                    } else {
                        (p.v, p.v)
                    };
                    (x(p.t), y(lo), y(hi))
                })
                .collect::<Vec<_>>()
                .chunk_by(|a, b| a.0.floor() == b.0.floor())
                .map(|c| {
                    let x = c.iter().map(|p| p.0).sum::<f32>() / c.len() as f32;
                    let lo = c.iter().map(|p| p.1).fold(f32::MIN, f32::max);
                    let hi = c.iter().map(|p| p.2).fold(f32::MAX, f32::min);
                    (x, lo, hi)
                })
                .collect();
            for w in cols.windows(2) {
                let ((x0, lo0, hi0), cur) = (w[0], w[1]);
                if cur.0 > x0 {
                    let i = mesh.vertices.len() as u32;
                    mesh.colored_vertex(Pos2::new(x0, lo0), fill);
                    mesh.colored_vertex(Pos2::new(x0, hi0), fill);
                    mesh.colored_vertex(Pos2::new(cur.0, cur.2), fill);
                    mesh.colored_vertex(Pos2::new(cur.0, cur.1), fill);
                    mesh.add_triangle(i, i + 1, i + 2);
                    mesh.add_triangle(i, i + 2, i + 3);
                }
            }
            if !mesh.is_empty() {
                painter.add(Shape::mesh(mesh));
            }
        }
        // The value line: steps for raw samples, slopes between bin means.
        // Each point is marked whether it is a bin mean, which may be
        // averaged with its pixel column's others; raw samples and step
        // corners never are, so live values after binned history keep
        // their steps.
        let mut pts: Vec<(Pos2, bool)> = Vec::with_capacity(run.len() * 2);
        let mut step = false;
        for p in run {
            let pos = Pos2::new(x(p.t), y(p.v));
            if step && let Some((last, _)) = pts.last().copied() {
                pts.push((Pos2::new(pos.x, last.y), false));
            }
            pts.push((pos, p.is_bin()));
            step = p.step;
        }
        // Bin means packed tighter than a pixel apart zigzag into a solid
        // block (a signal faster than the bins aliases); one mean per pixel
        // column draws the trend, and the band above shows the extremes.
        let pts = if run.iter().any(Pt::is_bin) {
            column_means(&pts)
        } else {
            decimate(pts.into_iter().map(|(p, _)| p).collect())
        };
        stroke_line(painter, pts, Stroke::new(line.width, line.color));
    }
}

/// `pts` with a corner added before each gap that follows a held (raw)
/// value, so its step runs flat to where the gap starts: splitting at the
/// gap would otherwise end the line at the value's own time. `ok` tells
/// a value from a gap.
fn hold_to_gaps(pts: &[Pt], ok: impl Fn(f64) -> bool) -> std::borrow::Cow<'_, [Pt]> {
    let corner = |w: &[Pt]| ok(w[0].v) && w[0].step && !ok(w[1].v) && w[1].t > w[0].t;
    if !pts.windows(2).any(corner) {
        return pts.into();
    }
    let mut out = Vec::with_capacity(pts.len() + 8);
    for (i, p) in pts.iter().enumerate() {
        if i > 0 && corner(&pts[i - 1..=i]) {
            out.push(Pt::raw(p.t, pts[i - 1].v));
        }
        out.push(*p);
    }
    out.into()
}

/// Average the bin means (`true`) in each pixel column, when there is more
/// than one; other points are kept as they are.
fn column_means(run: &[(Pos2, bool)]) -> Vec<Pos2> {
    run.chunk_by(|a, b| a.1 && b.1 && a.0.x.floor() == b.0.x.floor())
        .map(|c| {
            let n = c.len() as f32;
            let sum = c.iter().fold(egui::Vec2::ZERO, |s, p| s + p.0.to_vec2());
            (sum / n).to_pos2()
        })
        .collect()
}

/// The range that shows `data`: whole decades on a log scale, else rounded
/// out to whole ticks. `None` when there is nothing to show (bobview's
/// `plot::autoscale`).
pub fn autoscale(log: bool, data: impl Iterator<Item = f64>) -> Option<(f64, f64)> {
    let ok = |v: &f64| v.is_finite() && (!log || *v > 0.0);
    let (lo, hi) = data
        .filter(ok)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            (a.min(v), b.max(v))
        });
    if lo > hi {
        return None;
    }
    Some(if log {
        let (a, b) = (
            lo.log10().floor(),
            hi.log10().ceil().max(lo.log10().floor() + 1.0),
        );
        (10f64.powf(a), 10f64.powf(b))
    } else {
        // A little air above and below (none below a floor of exactly zero,
        // or above a ceiling of zero), then out to half ticks: rounding to
        // whole ticks can waste an eighth of the height.
        let pad = (hi - lo) * 0.04;
        let a = if lo >= 0.0 {
            (lo - pad).max(0.0)
        } else {
            lo - pad
        };
        let b = if hi <= 0.0 {
            (hi + pad).min(0.0)
        } else {
            hi + pad
        };
        if b <= a {
            return Some(axis::nice_range(a, b, 8));
        }
        let half = axis::nice_step(b - a, 8) / 2.0;
        ((a / half).floor() * half, (b / half).ceil() * half)
    })
}

pub fn text_size(painter: &egui::Painter, text: &str, font: &FontId) -> Vec2 {
    painter
        .layout_no_wrap(text.to_string(), font.clone(), Color32::BLACK)
        .size()
}

/// Text turned to read upwards, centred on `center`.
fn vertical_text(painter: &egui::Painter, center: Pos2, text: &str, font: FontId, c: Color32) {
    let galley = painter.layout_no_wrap(text.to_string(), font, c);
    let size = galley.size();
    let pos = Pos2::new(center.x - size.y / 2.0, center.y + size.x / 2.0);
    painter.add(epaint::TextShape::new(pos, galley, c).with_angle(-FRAC_PI_2));
}

/// Thin a line with many points per pixel column to the first, lowest,
/// highest and last point of each column, which draws the same.
fn decimate(run: Vec<Pos2>) -> Vec<Pos2> {
    if run.len() < 4096 {
        return run;
    }
    let mut out = Vec::new();
    for column in run.chunk_by(|a, b| a.x.floor() == b.x.floor()) {
        let lowest = column.iter().max_by(|a, b| a.y.total_cmp(&b.y));
        let highest = column.iter().min_by(|a, b| a.y.total_cmp(&b.y));
        let (first, last) = (column.first(), column.last());
        let mut keep: Vec<Pos2> = [first, lowest, highest, last]
            .into_iter()
            .flatten()
            .copied()
            .collect();
        keep.dedup();
        out.extend(keep);
    }
    out
}

fn stroke_line(painter: &egui::Painter, mut pts: Vec<Pos2>, stroke: Stroke) {
    // egui's tessellator turns a zero-length segment into NaN vertices,
    // which can paint one triangle over the whole plot.
    pts.dedup_by(|b, a| a.distance_sq(*b) < 1e-6);
    if pts.len() < 2 {
        if let Some(p) = pts.first() {
            painter.circle_filled(*p, stroke.width, stroke.color);
        }
        return;
    }
    painter.add(Shape::line(pts, stroke));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autoscale_pads_and_rounds() {
        let (lo, hi) = autoscale(false, [1.0, 9.0, f64::NAN].into_iter()).unwrap();
        assert!(lo <= 0.68 && hi >= 9.32, "{lo}..{hi}");
        assert_eq!(
            autoscale(true, [3.0, 450.0, -1.0].into_iter()),
            Some((1.0, 1000.0))
        );
        assert_eq!(autoscale(false, [f64::NAN].into_iter()), None);
        let (lo, hi) = autoscale(false, [5.0, 5.0].into_iter()).unwrap();
        assert!(lo < 5.0 && hi > 5.0);
    }

    #[test]
    fn autoscale_wastes_little() {
        // From zero: no negative tick under it.
        let (lo, hi) = autoscale(false, [0.0, 300.0].into_iter()).unwrap();
        assert_eq!(lo, 0.0);
        assert!((300.0..=325.0).contains(&hi), "{hi}");
        let (lo, hi) = autoscale(false, [-300.0, 0.0].into_iter()).unwrap();
        assert!(lo <= -300.0 && hi == 0.0);
    }

    #[test]
    fn raw_values_hold_up_to_a_gap() {
        let gap = Pt::raw(50.0, f64::NAN);
        let pts = [
            Pt::raw(0.0, 1.0),
            Pt::raw(10.0, 2.0),
            gap,
            Pt::raw(60.0, 3.0),
        ];
        let held = hold_to_gaps(&pts, f64::is_finite);
        let runs: Vec<Vec<(f64, f64)>> = held
            .split(|p| !p.v.is_finite())
            .map(|r| r.iter().map(|p| (p.t, p.v)).collect())
            .collect();
        // Flat from 10 to 50 at 2, then the gap, then 3.
        assert_eq!(
            runs,
            [
                vec![(0.0, 1.0), (10.0, 2.0), (50.0, 2.0)],
                vec![(60.0, 3.0)]
            ]
        );
        // A bin mean (sloped) is not held; nothing to add without gaps.
        let bin = Pt {
            lo: 0.0,
            hi: 2.0,
            step: false,
            ..Pt::raw(10.0, 1.0)
        };
        assert_eq!(hold_to_gaps(&[bin, gap], f64::is_finite).len(), 2);
        assert!(matches!(
            hold_to_gaps(&pts[..2], f64::is_finite),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn column_means_keep_raw_steps() {
        let p = |x: f32, y: f32| Pos2::new(x, y);
        // Two bins in column 0 average; the step corner and the live
        // sample in column 5 stay apart.
        let run = [
            (p(0.2, 10.0), true),
            (p(0.7, 20.0), true),
            (p(5.1, 20.0), false),
            (p(5.3, 30.0), false),
        ];
        let out = column_means(&run);
        assert_eq!(out, [p(0.45, 15.0), p(5.1, 20.0), p(5.3, 30.0)]);
    }

    #[test]
    fn decimation_keeps_column_extremes() {
        let pts: Vec<Pos2> = (0..10_000)
            .map(|i| Pos2::new(i as f32 / 100.0, (i % 7) as f32))
            .collect();
        let d = decimate(pts);
        assert!(d.len() <= 4 * 100);
        assert!(d.iter().any(|p| p.y == 6.0) && d.iter().any(|p| p.y == 0.0));
    }
}
