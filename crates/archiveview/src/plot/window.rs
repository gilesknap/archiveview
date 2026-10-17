//! The time window and value range maths behind panning and zooming. Pure
//! functions of the current state, so they are unit tested.

/// The shortest and longest spans the time axis zooms to.
pub const MIN_SPAN: f64 = 1.0;
pub const MAX_SPAN: f64 = 10.0 * 365.0 * 86_400.0;

/// The preset spans of the range pill.
pub const PRESETS: &[(f64, &str)] = &[
    (3600.0, "1h"),
    (8.0 * 3600.0, "8h"),
    (86_400.0, "1d"),
    (7.0 * 86_400.0, "7d"),
];

/// Panning to within this fraction of the span of now follows now again.
const SNAP_LIVE: f64 = 0.02;

/// The time shown: the `span` seconds up to `end`, or up to now while
/// `live`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub span: f64,
    /// The right edge when not live (seconds since 1970).
    pub end: f64,
    pub live: bool,
}

impl Window {
    /// The last `span` seconds, following now.
    pub fn live(span: f64) -> Self {
        Self {
            span: clamp_span(span),
            end: 0.0,
            live: true,
        }
    }

    /// A fixed window from `start` to `end`.
    pub fn fixed(start: f64, end: f64) -> Self {
        let (a, b) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        Self {
            span: clamp_span(b - a),
            end: b,
            live: false,
        }
    }

    pub fn end_at(&self, now: f64) -> f64 {
        if self.live { now } else { self.end }
    }

    /// `(from, to)` at time `now`.
    pub fn range(&self, now: f64) -> (f64, f64) {
        let end = self.end_at(now);
        (end - self.span, end)
    }

    /// Scale the span by `factor` (below 1 zooms in) keeping the time at
    /// `anchor` (a fraction across the window, 0 = left) where it is.
    /// While live, the right edge stays at now whatever the anchor.
    pub fn zoom(&mut self, factor: f64, anchor: f64, now: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let span = clamp_span(self.span * factor);
        if self.live {
            self.span = span;
            return;
        }
        let (from, _) = self.range(now);
        let at = from + anchor.clamp(0.0, 1.0) * self.span;
        let new_from = at - anchor.clamp(0.0, 1.0) * span;
        self.span = span;
        self.end = new_from + span;
        self.settle(now);
    }

    /// Move the window `dt` seconds later (earlier when negative). Leaves
    /// live mode going back; reaching now (within 2% of the span) follows
    /// it again.
    pub fn pan(&mut self, dt: f64, now: f64) {
        if !dt.is_finite() || dt == 0.0 {
            return;
        }
        if self.live {
            if dt > 0.0 {
                return;
            }
            self.live = false;
            self.end = now + dt;
        } else {
            self.end += dt;
        }
        self.settle(now);
    }

    /// Show `from..to` exactly (a box zoom, or a custom range), not live.
    pub fn set(&mut self, from: f64, to: f64) {
        *self = Self::fixed(from, to);
    }

    /// Follow now if the right edge reached it; never look into the future.
    fn settle(&mut self, now: f64) {
        if !self.live && self.end >= now - SNAP_LIVE * self.span {
            self.live = true;
        }
    }

    /// The preset whose span this is, if any.
    pub fn preset(&self) -> Option<usize> {
        PRESETS
            .iter()
            .position(|(s, _)| (s - self.span).abs() < 0.5)
    }
}

fn clamp_span(span: f64) -> f64 {
    if span.is_finite() {
        span.clamp(MIN_SPAN, MAX_SPAN)
    } else {
        3600.0
    }
}

/// `lo..hi` scaled by `factor` about `anchor` (a value), on a log scale
/// in log space.
pub fn zoom_values(lo: f64, hi: f64, factor: f64, anchor: f64, log: bool) -> (f64, f64) {
    if log && lo > 0.0 && hi > 0.0 && anchor > 0.0 {
        let (l, h, a) = (lo.log10(), hi.log10(), anchor.log10());
        let (l, h) = (a - (a - l) * factor, a + (h - a) * factor);
        return (10f64.powf(l), 10f64.powf(h));
    }
    let (l, h) = (
        anchor - (anchor - lo) * factor,
        anchor + (hi - anchor) * factor,
    );
    if (h - l).abs() < 1e-12 * l.abs().max(h.abs()).max(1e-300) {
        return (lo, hi);
    }
    (l, h)
}

/// `lo..hi` moved by `frac` of its height (positive moves the values up
/// the axis, showing lower ones).
pub fn pan_values(lo: f64, hi: f64, frac: f64, log: bool) -> (f64, f64) {
    if log && lo > 0.0 && hi > 0.0 {
        let d = (hi.log10() - lo.log10()) * frac;
        return (10f64.powf(lo.log10() - d), 10f64.powf(hi.log10() - d));
    }
    let d = (hi - lo) * frac;
    (lo - d, hi - d)
}

/// Ease `shown` toward `target` over about 150 ms, `dt` seconds after the
/// last frame. Returns true once there.
pub fn ease(shown: &mut f64, target: f64, dt: f64, scale: f64) -> bool {
    if !shown.is_finite() || !target.is_finite() {
        *shown = target;
        return true;
    }
    let k = 1.0 - (-dt.max(0.0) * 22.0).exp();
    *shown += (target - *shown) * k;
    if (target - *shown).abs() <= scale.abs() * 1e-3 {
        *shown = target;
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_000_000.0;

    #[test]
    fn live_windows_end_now() {
        let w = Window::live(3600.0);
        assert_eq!(w.range(NOW), (NOW - 3600.0, NOW));
        assert_eq!(w.range(NOW + 5.0), (NOW - 3595.0, NOW + 5.0));
        assert_eq!(PRESETS[w.preset().unwrap()].1, "1h");
        assert_eq!(Window::live(1e30).span, MAX_SPAN);
        assert_eq!(Window::live(f64::NAN).span, 3600.0);
    }

    #[test]
    fn zoom_keeps_the_anchor_still() {
        let mut w = Window::fixed(NOW - 2000.0, NOW - 1000.0);
        // The time a quarter of the way across stays there.
        let at = NOW - 2000.0 + 250.0;
        w.zoom(0.5, 0.25, NOW);
        let (from, to) = w.range(NOW);
        assert_eq!(to - from, 500.0);
        assert!((from + 0.25 * 500.0 - at).abs() < 1e-6);
        assert!(!w.live);
        // Live zooms about now.
        let mut l = Window::live(3600.0);
        l.zoom(2.0, 0.1, NOW);
        assert!(l.live);
        assert_eq!(l.range(NOW), (NOW - 7200.0, NOW));
        // Never below a second.
        l.zoom(1e-9, 0.5, NOW);
        assert_eq!(l.span, MIN_SPAN);
    }

    #[test]
    fn zooming_out_to_now_follows_it() {
        let mut w = Window::fixed(NOW - 1000.0, NOW - 100.0);
        w.zoom(4.0, 0.5, NOW);
        assert!(w.live, "the right edge passed now");
    }

    #[test]
    fn panning_leaves_and_rejoins_live() {
        let mut w = Window::live(1000.0);
        w.pan(50.0, NOW);
        assert!(w.live, "no panning into the future");
        w.pan(-300.0, NOW);
        assert!(!w.live);
        assert_eq!(w.range(NOW), (NOW - 1300.0, NOW - 300.0));
        w.pan(200.0, NOW);
        assert!(!w.live);
        // Within 2% of the span of now: live again.
        w.pan(85.0, NOW);
        assert!(w.live);
    }

    #[test]
    fn set_makes_a_fixed_window() {
        let mut w = Window::live(60.0);
        w.set(NOW - 10.0, NOW - 50.0);
        assert!(!w.live);
        assert_eq!(w.range(NOW), (NOW - 50.0, NOW - 10.0));
        assert_eq!(w.preset(), None);
    }

    #[test]
    fn values_zoom_and_pan() {
        assert_eq!(zoom_values(0.0, 10.0, 0.5, 5.0, false), (2.5, 7.5));
        assert_eq!(zoom_values(0.0, 10.0, 2.0, 0.0, false), (0.0, 20.0));
        let (l, h) = zoom_values(1.0, 1000.0, 0.5, 10.0, true);
        assert!((l - 10f64.powf(0.5)).abs() < 1e-9 && (h - 10f64.powf(2.0)).abs() < 1e-9);
        assert_eq!(pan_values(0.0, 10.0, 0.1, false), (-1.0, 9.0));
        let (l, h) = pan_values(1.0, 100.0, 0.5, true);
        assert!((l - 0.1).abs() < 1e-12 && (h - 10.0).abs() < 1e-9);
    }

    #[test]
    fn easing_arrives() {
        let mut x = 0.0;
        let mut frames = 0;
        while !ease(&mut x, 100.0, 1.0 / 60.0, 100.0) {
            frames += 1;
            assert!(frames < 60, "within a second");
        }
        assert_eq!(x, 100.0);
        assert!(frames > 3, "but not at once");
    }
}
