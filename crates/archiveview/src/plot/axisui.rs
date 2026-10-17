//! Working with the value axes directly: click an axis to pick out its
//! traces, drag or scroll on it to zoom it (in `Plot::interact`),
//! double-click its name to rename it, right-click it for its scale; and
//! moving a trace between axes and panes from its chip (the colour dot's
//! menu, or dragging the chip onto an axis or into a pane).

use egui::{Color32, FontId, Key, Pos2, Sense, Stroke, Vec2};

use super::render::Frame;
use super::{MoveTo, Plot, Trace};

/// What can be done on an axis, as its tooltip says.
const HINT: &str = "Drag or shift+scroll: zoom this axis · click: pick out its traces\n\
                    Double-click its name: rename · right-click: scale";

/// What hovering an axis' name shows: its whole name (it may be cut off),
/// its units and its traces, then what can be done on it.
struct AxisCard {
    name: String,
    units: String,
    traces: Vec<(Color32, String, bool)>,
}

impl AxisCard {
    fn show(&self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new(&self.name).strong());
        if !self.units.is_empty() {
            ui.label(format!("Units: {}", self.units));
        }
        for (color, name, visible) in &self.traces {
            ui.horizontal(|ui| {
                let (r, painter) = ui.allocate_painter(Vec2::splat(10.0), Sense::hover());
                if *visible {
                    painter.circle_filled(r.rect.center(), 4.0, *color);
                } else {
                    painter.circle_stroke(r.rect.center(), 3.5, Stroke::new(1.5, *color));
                }
                let text = egui::RichText::new(name);
                ui.label(if *visible { text } else { text.weak() });
            });
        }
        ui.add_space(2.0);
        ui.label(egui::RichText::new(HINT).small().weak());
    }
}

/// An axis name being edited.
pub struct Renaming {
    pub axis: u32,
    pub text: String,
    /// The text box has had the keyboard (asked for once).
    focused: bool,
}

impl Plot {
    /// Pane `k`'s axes' own clicks, and a chip dropped on one. Runs once
    /// the frame (`f`) is laid out, so after the plot's own interaction:
    /// the axes take clicks there, and leave drags to the plot.
    pub(super) fn axis_interact(&mut self, k: usize, ui: &egui::Ui, f: &mut Frame) {
        let pointer = ui.input(|i| i.pointer.latest_pos());
        if let Some(trace) = self.chip_drag {
            let inside = pointer.is_some_and(|p| f.outer.contains(p));
            let over = pointer.and_then(|p| f.axis_at(p));
            if let Some(n) = over {
                f.set_highlight(n);
            }
            // Dropped: here if over this pane (the last pane lets go of
            // a chip dropped on none).
            let last = k + 1 == self.panes.len();
            if !ui.input(|i| i.pointer.any_down()) && (inside || last) {
                self.chip_drag = None;
                let here = self.panes[k].id;
                let home = (self.traces.iter())
                    .find(|t| t.id == trace)
                    .map(|t| self.panes[self.home(t)].id);
                let to = match over {
                    Some(n) => self.panes[k].drawn.get(n).map(|id| MoveTo::Axis(*id)),
                    // On its own pane but no axis: an axis of its own;
                    // another pane: into it.
                    None if inside && home == Some(here) => Some(MoveTo::NewAxis(here)),
                    None if inside => Some(MoveTo::Pane(here)),
                    None => None,
                };
                if let Some(to) = to {
                    self.moves.push((trace, Some(to)));
                }
            }
        }
        for n in 0..f.axis_count() {
            let (Some(id), Some(rect)) = (self.panes[k].drawn.get(n).copied(), f.axis_rect(n))
            else {
                continue;
            };
            let r = ui.interact(rect, self.id.with(("axis", id)), Sense::click());
            let on_name = f
                .title_rect(n)
                .zip(pointer)
                .is_some_and(|(t, p)| t.expand2(Vec2::new(4.0, 0.0)).contains(p));
            // No tooltip over the name editor or a menu.
            let quiet = self.renaming.is_some() || egui::Popup::is_any_open(ui.ctx());
            let r = if quiet {
                r
            } else if on_name {
                let card = self.axis_card(id, ui.visuals().dark_mode);
                r.on_hover_ui_at_pointer(|ui| card.show(ui))
            } else {
                r.on_hover_text_at_pointer(HINT)
            };
            let pane = &mut self.panes[k];
            if r.double_clicked() && on_name {
                // Its first click picked the axis out (or not): undone.
                if let Some(before) = pane.highlight_before.take() {
                    pane.axis_highlight = before;
                }
                self.start_rename(id);
            } else if r.clicked() {
                pane.highlight_before = Some(pane.axis_highlight);
                pane.axis_highlight = (pane.axis_highlight != Some(id)).then_some(id);
            }
            egui::Popup::context_menu(&r).show(|ui| self.axis_scale_menu(ui, id));
        }
    }

    /// What the tooltip on axis `id`'s name shows.
    fn axis_card(&self, id: u32, dark: bool) -> AxisCard {
        let on: Vec<&Trace> = self.traces.iter().filter(|t| t.axis == Some(id)).collect();
        let mut units: Vec<String> = Vec::new();
        for u in on.iter().map(|t| super::units(t)) {
            if !u.is_empty() && !units.contains(&u) {
                units.push(u);
            }
        }
        AxisCard {
            name: self.axis_name(id).unwrap_or_default(),
            units: units.join(" · "),
            traces: on
                .iter()
                .map(|t| (self.color(t, dark), t.label(), t.visible))
                .collect(),
        }
    }

    /// An axis' own menu: its log scale, autoscale and name.
    fn axis_scale_menu(&mut self, ui: &mut egui::Ui, id: u32) {
        let Some(a) = self.axis_mut(id) else {
            return;
        };
        if ui.checkbox(&mut a.log, "Log scale").changed() {
            a.fit_log_range();
        }
        if ui
            .add_enabled(!a.autoscale, egui::Button::new("Autoscale"))
            .clicked()
        {
            a.autoscale = true;
            ui.close();
        }
        if ui.button("Rename…").clicked() {
            self.start_rename(id);
            ui.close();
        }
    }

    fn start_rename(&mut self, id: u32) {
        let text = self.axis_name(id).unwrap_or_default();
        self.renaming = Some(Renaming {
            axis: id,
            text,
            focused: false,
        });
    }

    /// The text box for an axis name being edited, over the name: Enter
    /// (or clicking away) keeps it, Esc does not; an empty name goes back
    /// to the one the axis gives itself. `frames`: the panes'.
    pub(super) fn rename_box(&mut self, ui: &egui::Ui, frames: &[Frame]) {
        let Some(edit) = &mut self.renaming else {
            return;
        };
        let found = (self.panes.iter().zip(frames))
            .find_map(|(p, f)| p.drawn.iter().position(|d| *d == edit.axis).map(|n| (f, n)));
        let Some((f, n)) = found else {
            self.renaming = None;
            return;
        };
        let Some(title) = f.title_rect(n) else {
            return;
        };
        let width = 180.0;
        let x = if title.center().x < f.data.center().x {
            title.left()
        } else {
            title.right() - width
        };
        let pos = Pos2::new(x, f.data.center().y - 14.0);
        let mut done = None;
        egui::Area::new(self.id.with("rename"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let mut out = egui::TextEdit::singleline(&mut edit.text)
                        .desired_width(width - 20.0)
                        .hint_text("Axis name (empty: automatic)")
                        .show(ui);
                    let r = out.response;
                    if !edit.focused {
                        // All of it selected, to type over.
                        let all = egui::text::CCursorRange::select_all(&out.galley);
                        out.state.cursor.set_char_range(Some(all));
                        out.state.store(ui.ctx(), r.id);
                        r.request_focus();
                        edit.focused = true;
                    } else if r.lost_focus() {
                        done = Some(!ui.input(|i| i.key_pressed(Key::Escape)));
                    }
                });
            });
        match done {
            Some(keep) => {
                let edit = self.renaming.take().expect("renaming");
                if keep && let Some(a) = self.axis_mut(edit.axis) {
                    let name = edit.text.trim();
                    a.name = (!name.is_empty()).then(|| name.to_string());
                }
            }
            None => ui.ctx().request_repaint(),
        }
    }

    /// The menu under a chip's colour dot: the axes of its pane to put
    /// its trace on, and a new one; a new pane, and the other panes.
    /// Moved at the end of the frame.
    pub(super) fn axis_menu(&mut self, ui: &mut egui::Ui, trace: u64) {
        let Some(t) = self.traces.iter().find(|t| t.id == trace) else {
            return;
        };
        let on = t.axis;
        let alone = on.is_some() && self.traces.iter().filter(|x| x.axis == on).count() == 1;
        let lone = t.pane.is_some() && self.traces.iter().filter(|x| x.pane == t.pane).count() == 1;
        let k = self.home(t);
        let here = self.panes[k].id;
        let mut to = None;
        ui.label(egui::RichText::new("Value axis").small().weak());
        for (id, name) in &self.panes[k].axis_names {
            if ui.selectable_label(on == Some(*id), name).clicked() {
                to = Some(MoveTo::Axis(*id));
            }
        }
        if ui
            .add_enabled(!alone, egui::Button::new("New axis"))
            .clicked()
        {
            to = Some(MoveTo::NewAxis(here));
        }
        ui.separator();
        if ui
            .add_enabled(!lone, egui::Button::new("New pane"))
            .on_hover_text("A pane of its own, below the others")
            .clicked()
        {
            to = Some(MoveTo::NewPane);
        }
        if self.panes.len() > 1 {
            let panes: Vec<(u32, String)> = (0..self.panes.len())
                .map(|n| (self.panes[n].id, self.pane_label(n)))
                .collect();
            ui.menu_button("Pane", |ui| {
                for (id, label) in panes {
                    if ui.selectable_label(id == here, label).clicked() {
                        to = Some(MoveTo::Pane(id));
                    }
                }
            });
        }
        if let Some(to) = to {
            self.moves.push((trace, Some(to)));
            ui.close();
        }
    }

    /// What pane `k` is called in menus: its place and its axes' names,
    /// "2 · Pressure (mbar)".
    fn pane_label(&self, k: usize) -> String {
        let names: Vec<&str> = (self.panes[k].axis_names.iter())
            .map(|(_, n)| n.as_str())
            .collect();
        let label = format!("{} · {}", k + 1, names.join(", "));
        if label.chars().count() > 48 {
            let cut: String = label.chars().take(47).collect();
            format!("{}…", cut.trim_end())
        } else {
            label
        }
    }

    /// A chip being dragged: a small pill under the pointer saying where
    /// it can go.
    pub(super) fn chip_ghost(&self, ui: &egui::Ui, dark: bool) {
        let (Some(id), Some(p)) = (self.chip_drag, ui.input(|i| i.pointer.latest_pos())) else {
            return;
        };
        let Some(t) = self.traces.iter().find(|t| t.id == id) else {
            return;
        };
        let v = ui.visuals();
        let painter = ui.painter().clone().with_layer_id(egui::LayerId::new(
            egui::Order::Tooltip,
            self.id.with("chip-ghost"),
        ));
        let text = format!(
            "{}  →  an axis, a pane, or its pane for a new axis",
            Trace::label(t)
        );
        let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), v.text_color());
        let size = galley.size() + Vec2::new(30.0, 8.0);
        let rect = egui::Rect::from_min_size(p + Vec2::new(12.0, 8.0), size);
        painter.rect(
            rect,
            size.y / 2.0,
            v.window_fill.gamma_multiply(0.95),
            Stroke::new(1.0, v.window_stroke.color),
            egui::StrokeKind::Inside,
        );
        let dot = Pos2::new(rect.left() + 12.0, rect.center().y);
        painter.circle_filled(dot, 4.0, self.color(t, dark));
        painter.galley(
            Pos2::new(dot.x + 10.0, rect.center().y - galley.size().y / 2.0),
            galley,
            Color32::PLACEHOLDER,
        );
        ui.ctx().request_repaint();
    }
}
