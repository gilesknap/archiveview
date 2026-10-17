//! Toolbar icons, drawn as strokes so they look the same on every platform
//! (the fonts' symbols vary, and some are emoji). Adapted from bobview's
//! `icons.rs`, with the icons a plot needs.

use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Icon {
    /// Reset the view.
    Home,
    /// Fit the values (autoscale): arrows out to the top and bottom.
    Fit,
    Menu,
    Search,
    /// A recent search.
    Clock,
    /// Already on the plot.
    Check,
    /// Forget, clear.
    Cross,
}

impl Icon {
    /// The strokes, as polylines in a 16 × 16 box.
    fn lines(self) -> Vec<Vec<[f32; 2]>> {
        match self {
            Icon::Home => vec![
                vec![[2.5, 7.5], [8.0, 3.0], [13.5, 7.5]],
                vec![
                    [4.0, 6.5],
                    [4.0, 13.0],
                    [7.0, 13.0],
                    [7.0, 9.5],
                    [9.0, 9.5],
                    [9.0, 13.0],
                    [12.0, 13.0],
                    [12.0, 6.5],
                ],
            ],
            Icon::Fit => vec![
                vec![[8.0, 1.5], [8.0, 14.5]],
                vec![[5.5, 4.0], [8.0, 1.5], [10.5, 4.0]],
                vec![[5.5, 12.0], [8.0, 14.5], [10.5, 12.0]],
                vec![[2.0, 8.0], [5.0, 8.0]],
                vec![[11.0, 8.0], [14.0, 8.0]],
            ],
            Icon::Menu => vec![
                vec![[3.0, 4.5], [13.0, 4.5]],
                vec![[3.0, 8.0], [13.0, 8.0]],
                vec![[3.0, 11.5], [13.0, 11.5]],
            ],
            Icon::Search => {
                let ring = (0..=16)
                    .map(|i| {
                        let a = (i as f32 * 22.5).to_radians();
                        [6.8 + 4.3 * a.cos(), 6.8 + 4.3 * a.sin()]
                    })
                    .collect();
                vec![ring, vec![[10.0, 10.0], [14.0, 14.0]]]
            }
            Icon::Clock => {
                let ring = (0..=24)
                    .map(|i| {
                        let a = (i as f32 * 15.0).to_radians();
                        [8.0 + 6.0 * a.cos(), 8.0 + 6.0 * a.sin()]
                    })
                    .collect();
                vec![ring, vec![[8.0, 4.5], [8.0, 8.0], [10.5, 9.5]]]
            }
            Icon::Check => vec![vec![[3.0, 8.5], [6.5, 12.0], [13.0, 4.5]]],
            Icon::Cross => vec![
                vec![[4.5, 4.5], [11.5, 11.5]],
                vec![[11.5, 4.5], [4.5, 11.5]],
            ],
        }
    }

    /// Draw the icon `size` points square, centred in `rect`.
    pub fn paint(self, painter: &egui::Painter, rect: Rect, size: f32, color: Color32) {
        let scale = size / 16.0;
        let origin = rect.center() - Vec2::splat(size / 2.0);
        let stroke = Stroke::new((1.5 * scale).max(1.0), color);
        for line in self.lines() {
            let points = line
                .iter()
                .map(|[x, y]| origin + Vec2::new(x * scale, y * scale))
                .collect::<Vec<Pos2>>();
            painter.add(Shape::line(points, stroke));
        }
    }
}

/// The height of a toolbar button: a line of button text and its padding.
pub fn height(ui: &egui::Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y
}

/// A borderless round-cornered button showing `icon`, as tall as a text
/// button: highlighted when `selected`.
pub fn button(ui: &mut egui::Ui, icon: Icon, selected: bool) -> egui::Response {
    let height = height(ui);
    let size = Vec2::new(height * 1.1, height);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&resp, selected);
        let fill = if selected {
            ui.visuals().selection.bg_fill
        } else if resp.hovered() {
            visuals.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        };
        ui.painter()
            .rect(rect, 8.0, fill, Stroke::NONE, StrokeKind::Inside);
        let color = if selected {
            ui.visuals().selection.stroke.color
        } else {
            visuals.fg_stroke.color
        };
        icon.paint(ui.painter(), rect, height * 0.55, color);
    }
    resp
}
