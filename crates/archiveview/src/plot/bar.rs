//! The slim top bar: the PV search box with live completion from the
//! archiver, the time-range pill, the Live pill, autoscale and reset, and
//! the `≡` menu.

use archiver::Search;
use egui::{Align, Color32, Key, Layout, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use super::timeaxis::{format_local, format_span, parse_local, parse_span};
use super::window::{PRESETS, Window};
use super::{Plot, now};
use crate::icons::{self, Icon};

/// Wait this long after typing before searching (seconds).
const SEARCH_DELAY: f64 = 0.2;

/// Names a search asks for.
const SEARCH_LIMIT: i32 = 50;

/// Below this width the bar is compact: the pill is a menu and the
/// buttons move into the `≡` menu.
const COMPACT_WIDTH: f32 = 600.0;

/// The search box.
#[derive(Default)]
pub struct SearchState {
    text: String,
    regex: bool,
    /// The text as last searched for, and when it was last edited.
    searched: String,
    edited_at: f64,
    /// The latest search's number; older answers are dropped.
    asked: u64,
    waiting: bool,
    pub(super) results: Vec<String>,
    error: Option<String>,
    /// The highlighted row of the popup.
    selected: usize,
    open: bool,
    /// Give the box the keyboard on the next frame (`/`).
    pub focus: bool,
}

/// The custom range popover.
#[derive(Default)]
pub struct CustomState {
    open: bool,
    span: String,
    from: String,
    to: String,
    error: Option<String>,
}

pub fn top_bar(
    plot: &mut Plot,
    ui: &mut egui::Ui,
    menu: Option<&mut dyn FnMut(&mut egui::Ui)>,
    now: f64,
) {
    let compact = ui.available_width() < COMPACT_WIDTH;
    ui.spacing_mut().button_padding = Vec2::new(10.0, 5.0);
    ui.spacing_mut().item_spacing.x = 6.0;
    let h = icons::height(ui);
    let row = Vec2::new(ui.available_width(), h);
    ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
        let menu_button = icons::button(ui, Icon::Menu, false).on_hover_text("More");
        egui::Popup::menu(&menu_button).show(|ui| {
            if compact {
                if ui.button("Autoscale values").clicked() {
                    plot.y.autoscale = true;
                }
                if ui.button("Reset view").clicked() {
                    plot.reset();
                }
                ui.separator();
            }
            if ui.checkbox(&mut plot.y.log, "Log scale").changed() {
                plot.fit_log_range();
            }
            if let Some(menu) = menu {
                ui.separator();
                menu(ui);
            }
            ui.separator();
            ui.label(
                egui::RichText::new(
                    "drag: pan · wheel: zoom time · shift+wheel: zoom values\n\
                     shift+drag: zoom to box · double-click: reset\n\
                     / search · ←→ pan · +/- zoom · L live · A autoscale",
                )
                .small()
                .weak(),
            );
        });
        if !compact {
            if icons::button(ui, Icon::Home, false)
                .on_hover_text(
                    "Reset: the preset span up to now, values fitted (Home, double-click)",
                )
                .clicked()
            {
                plot.reset();
            }
            if icons::button(ui, Icon::Fit, plot.y.autoscale)
                .on_hover_text("Autoscale values (A)")
                .clicked()
            {
                plot.y.autoscale = true;
            }
        }
        live_pill(plot, ui, now);
        if compact {
            range_combo(plot, ui);
        } else {
            range_pill(plot, ui, now);
        }
        // The search box takes the rest, from the left.
        let rest = Vec2::new(ui.available_width(), h);
        ui.allocate_ui_with_layout(rest, Layout::left_to_right(Align::Center), |ui| {
            let w = ui.available_width().min(460.0);
            search_box(plot, ui, w, now);
        });
    });
}

/// A pill-shaped frame, as the controls in the bar are.
fn pill_frame(ui: &egui::Ui, rect: Rect) {
    let v = ui.visuals();
    ui.painter().rect(
        rect,
        rect.height() / 2.0,
        v.extreme_bg_color,
        v.widgets.noninteractive.bg_stroke,
        StrokeKind::Inside,
    );
}

// --- search -----------------------------------------------------------------

fn search_box(plot: &mut Plot, ui: &mut egui::Ui, width: f32, now: f64) {
    let h = icons::height(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
    let edit_id = plot.id.with("search");
    let focused = ui.ctx().memory(|m| m.has_focus(edit_id));
    pill_frame(ui, rect);
    if focused {
        ui.painter().rect_stroke(
            rect,
            h / 2.0,
            Stroke::new(1.5, ui.visuals().selection.bg_fill),
            StrokeKind::Inside,
        );
    }
    let icon = Rect::from_min_size(rect.min + Vec2::new(8.0, 0.0), Vec2::new(h * 0.6, h));
    Icon::Search.paint(ui.painter(), icon, h * 0.5, ui.visuals().weak_text_color());

    // Keys for the result list, taken before the text field sees them.
    let s = &mut plot.search;
    let rows = rows(s, plot.archiver.is_some());
    if focused && s.open {
        let (down, up) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
            )
        });
        if down && s.selected + 1 < rows.len() {
            s.selected += 1;
        }
        if up {
            s.selected = s.selected.saturating_sub(1);
        }
    }

    // On the right inside the box: ✕ to clear when there is text, and the
    // regex toggle while the box has the focus or the pointer (or regex is
    // on), so an idle box is just a search field.
    let regex_w = 28.0;
    let clear_w = 20.0;
    let hovered = ui.rect_contains_pointer(rect);
    let show_regex = plot.archiver.is_some() && (focused || hovered || s.regex);
    let show_clear = !s.text.is_empty();
    let right = rect.right()
        - 6.0
        - if show_regex { regex_w } else { 0.0 }
        - if show_clear { clear_w } else { 0.0 };
    let inner = Rect::from_min_max(
        Pos2::new(icon.right() + 2.0, rect.top()),
        Pos2::new(right.max(icon.right() + 40.0), rect.bottom()),
    );
    let mut edit_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let hint = if plot.archiver.is_some() {
        "Search PVs…"
    } else {
        "Add a PV…"
    };
    if !focused && s.text.is_empty() {
        // The shortcut, as a key cap.
        let cap = Rect::from_center_size(
            Pos2::new(inner.right() - 10.0, rect.center().y),
            Vec2::new(16.0, 16.0),
        );
        let v = ui.visuals();
        ui.painter().rect(
            cap,
            4.0,
            Color32::TRANSPARENT,
            Stroke::new(1.0, v.weak_text_color().gamma_multiply(0.6)),
            StrokeKind::Inside,
        );
        ui.painter().text(
            cap.center(),
            egui::Align2::CENTER_CENTER,
            "/",
            egui::FontId::proportional(11.0),
            v.weak_text_color(),
        );
    }
    let resp = edit_ui.add(
        egui::TextEdit::singleline(&mut s.text)
            .id(edit_id)
            .frame(egui::Frame::NONE)
            .hint_text(hint)
            .desired_width(inner.width()),
    );
    if std::mem::take(&mut s.focus) {
        resp.request_focus();
    }
    if resp.changed() {
        s.edited_at = now;
        s.selected = 0;
        s.open = !s.text.trim().is_empty();
    }
    if resp.gained_focus() && !s.text.trim().is_empty() {
        s.open = true;
    }

    // The regex toggle, inside the box on the right.
    let toggle = Rect::from_center_size(
        Pos2::new(rect.right() - regex_w / 2.0 - 5.0, rect.center().y),
        Vec2::new(regex_w, h - 8.0),
    );
    if show_clear {
        let x = if show_regex {
            toggle.left() - clear_w / 2.0
        } else {
            rect.right() - 5.0 - clear_w / 2.0
        };
        let button = Rect::from_center_size(Pos2::new(x, rect.center().y), Vec2::splat(18.0));
        let r = ui
            .interact(button, plot.id.with("clear"), Sense::click())
            .on_hover_text("Clear");
        let v = ui.visuals();
        if r.hovered() {
            ui.painter()
                .circle_filled(button.center(), 9.0, v.widgets.hovered.weak_bg_fill);
        }
        let k = Stroke::new(
            1.4,
            if r.hovered() {
                v.text_color()
            } else {
                v.weak_text_color()
            },
        );
        let (m, d) = (button.center(), 3.5);
        ui.painter()
            .line_segment([m + Vec2::new(-d, -d), m + Vec2::new(d, d)], k);
        ui.painter()
            .line_segment([m + Vec2::new(d, -d), m + Vec2::new(-d, d)], k);
        if r.clicked() {
            s.text.clear();
            s.searched.clear();
            s.results.clear();
            s.error = None;
            s.waiting = false;
            s.open = false;
            s.focus = true;
            ui.ctx().request_repaint();
        }
    }
    if show_regex {
        let r = ui
            .interact(toggle, plot.id.with("regex"), Sense::click())
            .on_hover_text("Match a regular expression (else * and ? wildcards)");
        if r.clicked() {
            s.regex = !s.regex;
            s.searched.clear();
            s.edited_at = now;
            s.focus = true;
            ui.ctx().request_repaint();
        }
        let v = ui.visuals();
        let (fill, fg) = if s.regex {
            (v.selection.bg_fill, v.selection.stroke.color)
        } else if r.hovered() {
            (v.widgets.hovered.weak_bg_fill, v.text_color())
        } else {
            (Color32::TRANSPARENT, v.weak_text_color())
        };
        ui.painter()
            .rect_filled(toggle, toggle.height() / 2.0, fill);
        ui.painter().text(
            toggle.center(),
            egui::Align2::CENTER_CENTER,
            ".*",
            egui::FontId::monospace(12.0),
            fg,
        );
    }

    // Search once typing pauses.
    let text = s.text.trim().to_string();
    if let Some(client) = &plot.archiver
        && !text.is_empty()
        && text != s.searched
        && now - s.edited_at >= SEARCH_DELAY
    {
        s.searched = text.clone();
        s.asked += 1;
        s.waiting = true;
        let search = if s.regex {
            Search::regex(text.clone(), SEARCH_LIMIT)
        } else {
            Search::from_user(&text, SEARCH_LIMIT)
        };
        plot.fetcher
            .search(&plot.pvs, client, search, s.asked, plot.repaint.clone());
    } else if !text.is_empty() && text != s.searched && plot.archiver.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(60));
    }

    // Enter adds the highlighted row, or what was typed.
    let mut add: Vec<String> = Vec::new();
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if enter {
        if let Some(row) = rows.get(s.selected) {
            add = row.names();
        }
        resp.request_focus();
    }
    if focused && ui.input(|i| i.key_pressed(Key::Escape)) {
        if s.open {
            s.open = false;
        } else {
            s.text.clear();
            ui.ctx().memory_mut(|m| m.surrender_focus(edit_id));
        }
    }

    // The popup of results.
    if s.open && !text.is_empty() {
        let pos = rect.left_bottom() + Vec2::new(0.0, 4.0);
        let area = egui::Area::new(plot.id.with("search-popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(10.0)
                    .show(ui, |ui| {
                        ui.set_width(width - 14.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        egui::ScrollArea::vertical()
                            .max_height(320.0)
                            .min_scrolled_height((rows.len() as f32 * 26.0).min(320.0))
                            .show(ui, |ui| {
                                for (i, row) in rows.iter().enumerate() {
                                    let picked = i == s.selected;
                                    let label = row.label();
                                    let r = ui.add(
                                        egui::Button::selectable(picked, label)
                                            .min_size(Vec2::new(ui.available_width(), 24.0)),
                                    );
                                    if picked && enter {
                                        r.scroll_to_me(None);
                                    }
                                    if r.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO)
                                    {
                                        s.selected = i;
                                    }
                                    if r.clicked() {
                                        add = row.names();
                                    }
                                }
                            });
                        let note = if s.waiting {
                            Some("Searching the archiver…".to_string())
                        } else if let Some(e) = &s.error {
                            Some(format!("Search failed: {e}"))
                        } else if plot.archiver.is_some()
                            && s.searched == text
                            && s.results.is_empty()
                        {
                            Some("No archived PVs match".to_string())
                        } else if s.results.len() as i32 >= SEARCH_LIMIT {
                            Some(format!("First {SEARCH_LIMIT} matches: type more to narrow"))
                        } else {
                            None
                        };
                        if let Some(note) = note {
                            ui.add_space(2.0);
                            ui.horizontal(|ui| {
                                if s.waiting {
                                    ui.spinner();
                                }
                                ui.label(egui::RichText::new(note).small().weak());
                            });
                        }
                    });
            });
        // A click anywhere else closes it.
        let clicked_away = ui.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !area.response.rect.contains(p) && !rect.contains(p))
        });
        if clicked_away {
            s.open = false;
        }
    }

    if !add.is_empty() {
        s.text.clear();
        s.searched.clear();
        s.results.clear();
        s.open = false;
        plot.add_pvs(&add);
    }
}

/// A row of the search popup.
enum Row {
    /// What was typed, added as it is (one or more names).
    Typed(Vec<String>),
    Found(String),
}

impl Row {
    fn names(&self) -> Vec<String> {
        match self {
            Row::Typed(n) => n.clone(),
            Row::Found(n) => vec![n.clone()],
        }
    }

    fn label(&self) -> egui::RichText {
        match self {
            Row::Typed(n) => egui::RichText::new(format!("+  Add “{}” as typed", n.join("”, “"))),
            Row::Found(n) => egui::RichText::new(n),
        }
    }
}

/// The popup's rows: archiver matches first, then the text as typed
/// (unless it is a pattern, or exactly a match).
fn rows(s: &SearchState, archiver: bool) -> Vec<Row> {
    let text = s.text.trim();
    let mut rows: Vec<Row> = if s.searched == text {
        s.results.iter().cloned().map(Row::Found).collect()
    } else {
        Vec::new()
    };
    let typed: Vec<String> = text
        .split([' ', ',', '\t'])
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect();
    let pattern = archiver && (s.regex || text.contains(['*', '?']));
    if !typed.is_empty() && !pattern && !s.results.iter().any(|r| r == text) {
        rows.push(Row::Typed(typed));
    }
    rows
}

/// Whether typing has paused for a search not yet sent.
pub fn search_due(plot: &Plot) -> bool {
    let text = plot.search.text.trim();
    plot.archiver.is_some() && !text.is_empty() && text != plot.search.searched
}

/// Take in a finished search.
pub fn take_found(plot: &mut Plot, _now: f64) {
    if let Some((n, found)) = plot.fetcher.take_found() {
        let s = &mut plot.search;
        if n != s.asked {
            return;
        }
        s.waiting = false;
        match found {
            Ok(names) => {
                s.results = names;
                s.error = None;
            }
            Err(e) => {
                s.results.clear();
                s.error = Some(e);
            }
        }
        s.selected = 0;
    }
}

// --- time range ---------------------------------------------------------------

/// The segmented control: 1h 8h 1d 7d and a custom segment showing a span
/// that is not a preset.
fn range_pill(plot: &mut Plot, ui: &mut egui::Ui, now: f64) {
    let h = icons::height(ui);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let current = plot.window.preset();
    let custom_label = if current.is_none() {
        format!("{}  ⋯", format_span(plot.window.span))
    } else {
        "⋯".to_string()
    };
    let labels: Vec<String> = PRESETS
        .iter()
        .map(|(_, l)| l.to_string())
        .chain(std::iter::once(custom_label))
        .collect();
    let widths: Vec<f32> = labels
        .iter()
        .map(|l| super::render::text_size(ui.painter(), l, &font).x + 20.0)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, h), Sense::hover());
    pill_frame(ui, rect);
    let mut x = rect.left() + 2.0;
    let v = ui.visuals().clone();
    let mut custom_rect = rect;
    for (i, (label, w)) in labels.iter().zip(&widths).enumerate() {
        let seg = Rect::from_min_size(Pos2::new(x, rect.top() + 2.0), Vec2::new(*w, h - 4.0));
        x += w;
        let is_custom = i == PRESETS.len();
        let on = if is_custom {
            current.is_none()
        } else {
            current == Some(i)
        };
        let r = ui.interact(seg, plot.id.with(("range", i)), Sense::click());
        let r = if is_custom {
            custom_rect = seg;
            r.on_hover_text("Another span, or a fixed range")
        } else {
            r.on_hover_text(format!("The last {label}"))
        };
        let (fill, fg) = if on {
            (v.selection.bg_fill, v.selection.stroke.color)
        } else if r.hovered() {
            (v.widgets.hovered.weak_bg_fill, v.text_color())
        } else {
            (Color32::TRANSPARENT, v.text_color())
        };
        ui.painter().rect_filled(seg, seg.height() / 2.0, fill);
        ui.painter().text(
            seg.center(),
            egui::Align2::CENTER_CENTER,
            label,
            font.clone(),
            fg,
        );
        if r.clicked() {
            if is_custom {
                open_custom(plot, now);
            } else {
                plot.preset(PRESETS[i].0);
            }
        }
    }
    custom_popover(plot, ui, custom_rect, now);
}

/// The range as a menu, for a narrow bar: one pill showing the span, which
/// opens the presets.
fn range_combo(plot: &mut Plot, ui: &mut egui::Ui) {
    let label = match plot.window.preset() {
        Some(i) => PRESETS[i].1.to_string(),
        None => format_span(plot.window.span),
    };
    let label = format!("{label}  ▾");
    let h = icons::height(ui);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let w = super::render::text_size(ui.painter(), &label, &font).x + 24.0;
    let (rect, r) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    pill_frame(ui, rect);
    let v = ui.visuals();
    if r.hovered() {
        let inner = rect.shrink(2.0);
        ui.painter()
            .rect_filled(inner, inner.height() / 2.0, v.widgets.hovered.weak_bg_fill);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        &label,
        font,
        v.text_color(),
    );
    let r = r.on_hover_text("Time range");
    egui::Popup::menu(&r).show(|ui| {
        let current = plot.window.preset();
        for (i, (span, name)) in PRESETS.iter().enumerate() {
            if ui.selectable_label(current == Some(i), *name).clicked() {
                plot.preset(*span);
            }
        }
        if ui.selectable_label(current.is_none(), "Custom…").clicked() {
            open_custom(plot, now());
        }
    });
    if plot.custom.open {
        custom_popover(plot, ui, rect, now());
    }
}

fn open_custom(plot: &mut Plot, now: f64) {
    let c = &mut plot.custom;
    c.open = !c.open;
    let (from, to) = plot.window.range(now);
    c.span = format_span(plot.window.span);
    c.from = format_local(from);
    c.to = if plot.window.live {
        "now".into()
    } else {
        format_local(to)
    };
    c.error = None;
}

/// The custom range popover under `anchor`: a span up to now, or fixed
/// start and end times.
fn custom_popover(plot: &mut Plot, ui: &mut egui::Ui, anchor: Rect, now: f64) {
    if !plot.custom.open {
        return;
    }
    let width = 300.0;
    let pos = Pos2::new(
        (anchor.right() - width).max(ui.ctx().content_rect().left() + 4.0),
        anchor.bottom() + 6.0,
    );
    let mut apply: Option<Window> = None;
    let c = &mut plot.custom;
    let area = egui::Area::new(plot.id.with("custom-range"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .corner_radius(10.0)
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.label(egui::RichText::new("Last").strong());
                    ui.horizontal(|ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut c.span)
                                .hint_text("90m, 3h, 2 days")
                                .desired_width(150.0),
                        );
                        let go = ui.button("Show").clicked()
                            || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                        if go {
                            match parse_span(&c.span) {
                                Some(s) => apply = Some(Window::live(s)),
                                None => c.error = Some(format!("Not a span: {:?}", c.span)),
                            }
                        }
                    });
                    ui.separator();
                    ui.label(egui::RichText::new("From … to").strong());
                    egui::Grid::new("custom-range-grid")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("From");
                            ui.add(
                                egui::TextEdit::singleline(&mut c.from)
                                    .hint_text("2026-10-17 09:00")
                                    .desired_width(170.0),
                            );
                            ui.end_row();
                            ui.label("To");
                            ui.add(
                                egui::TextEdit::singleline(&mut c.to)
                                    .hint_text("now")
                                    .desired_width(170.0),
                            );
                            ui.end_row();
                        });
                    if ui.button("Show range").clicked() {
                        let to =
                            if c.to.trim().eq_ignore_ascii_case("now") || c.to.trim().is_empty() {
                                Some(now)
                            } else {
                                parse_local(&c.to)
                            };
                        match (parse_local(&c.from), to) {
                            (Some(a), Some(b)) if a < b => {
                                let mut w = Window::fixed(a, b);
                                if b >= now {
                                    w = Window::live(b - a);
                                }
                                apply = Some(w);
                            }
                            (Some(_), Some(_)) => c.error = Some("From must be before to".into()),
                            _ => c.error = Some("Times read YYYY-MM-DD HH:MM[:SS] (local)".into()),
                        }
                    }
                    if let Some(e) = &c.error {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                });
        });
    let away = ui.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !area.response.rect.contains(p) && !anchor.contains(p))
    });
    if away || ui.input(|i| i.key_pressed(Key::Escape)) {
        plot.custom.open = false;
    }
    if let Some(w) = apply {
        plot.custom.open = false;
        plot.window = w;
        plot.home_span = w.span;
        plot.glide = 0.0;
    }
}

// --- live -------------------------------------------------------------------

/// `● Live`: the dot shows while following now, pulsing for a few seconds
/// after it starts and while the pointer is over the plot; click to pause
/// or to jump to now.
fn live_pill(plot: &mut Plot, ui: &mut egui::Ui, now: f64) {
    let h = icons::height(ui);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text_w = super::render::text_size(ui.painter(), "Live", &font).x;
    let size = Vec2::new(text_w + h * 0.9 + 18.0, h);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let live = plot.window.live;
    let r = r.on_hover_text(if live {
        "Following now: click to pause (L)"
    } else {
        "Jump to now and follow it (L)"
    });
    let v = ui.visuals();
    let dark = v.dark_mode;
    let accent = crate::style::live(dark);
    let fill = if live {
        accent.gamma_multiply(if dark { 0.22 } else { 0.10 })
    } else if r.hovered() {
        v.widgets.hovered.weak_bg_fill
    } else {
        v.extreme_bg_color
    };
    let stroke = if live {
        Stroke::new(1.0, accent.gamma_multiply(0.5))
    } else {
        v.widgets.noninteractive.bg_stroke
    };
    ui.painter()
        .rect(rect, h / 2.0, fill, stroke, StrokeKind::Inside);
    let dot = Pos2::new(rect.left() + 12.0 + 4.0, rect.center().y);
    if live && plot.pulse {
        // A slow breath: a halo growing and fading every two seconds.
        let phase = (ui.input(|i| i.time) % 2.0 / 2.0) as f32;
        ui.painter().circle_filled(
            dot,
            4.0 + 5.0 * phase,
            accent.gamma_multiply(0.45 * (1.0 - phase)),
        );
        ui.painter().circle_filled(dot, 4.0, accent);
    } else if live {
        // Still, once it has said so: no animation in an idle window.
        ui.painter()
            .circle_filled(dot, 6.5, accent.gamma_multiply(0.18));
        ui.painter().circle_filled(dot, 4.0, accent);
    } else {
        ui.painter()
            .circle_stroke(dot, 3.5, Stroke::new(1.5, v.weak_text_color()));
    }
    let fg = if live {
        v.strong_text_color()
    } else {
        v.text_color()
    };
    ui.painter().text(
        Pos2::new(dot.x + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Live",
        font,
        fg,
    );
    if r.clicked() {
        plot.toggle_live(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(text: &str, searched: &str, results: &[&str]) -> SearchState {
        SearchState {
            text: text.into(),
            searched: searched.into(),
            results: results.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn rows_list_matches_then_the_text() {
        let s = state("SINE", "SINE", &["MOCK:SINE", "MOCK:SINE2"]);
        let r = rows(&s, true);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].names(), ["MOCK:SINE"]);
        assert_eq!(r[2].names(), ["SINE"]);
        // Stale results are not offered.
        let s = state("SIN", "SINE", &["MOCK:SINE"]);
        assert_eq!(rows(&s, true).len(), 1);
        // A pattern is not a PV name; several names add together.
        assert_eq!(rows(&state("MOCK:*", "", &[]), true).len(), 0);
        let r = rows(&state("a:b, c:d", "", &[]), false);
        assert_eq!(r[0].names(), ["a:b", "c:d"]);
        // An exact match is not offered twice.
        assert_eq!(
            rows(&state("MOCK:SINE", "MOCK:SINE", &["MOCK:SINE"]), true).len(),
            1
        );
    }
}
