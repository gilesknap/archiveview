//! The slim top bar: the PV search box with live completion from the
//! archiver, the time-range pill, the Live pill, the archiver pill,
//! autoscale and reset, and the `≡` menu.

use archiver::{ArchiverClient, Search};
use egui::text::{CCursor, CCursorRange};
use egui::{Align, Color32, Key, Layout, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use super::paste::MiddleClick;
use super::status::ArchiverState;
use super::timeaxis::{format_local, format_span, parse_local, parse_span};
use super::window::{PRESETS, Window};
use super::{Plot, now};
use crate::history::{RecentSearch, SearchHistory};
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
    pub(super) text: String,
    pub(super) regex: bool,
    /// The text as last searched for, and when it was last edited.
    pub(super) searched: String,
    edited_at: f64,
    /// The latest search's number; older answers are dropped.
    asked: u64,
    waiting: bool,
    pub(super) results: Vec<String>,
    error: Option<String>,
    /// The highlighted row of the popup, once the arrows or the pointer
    /// have moved it (else `default_row`).
    selected: Option<usize>,
    pub(super) open: bool,
    /// Give the box the keyboard on the next frame (`/`).
    pub focus: bool,
    /// As `focus`, but keeping the cursor where it is: the box lent the
    /// focus to the popup for a click.
    refocus: bool,
    /// Select all the text, until the click that focused the box is let go.
    select_all: bool,
}

impl SearchState {
    /// Search again (another archiver): answers to earlier searches are
    /// dropped.
    pub fn restart(&mut self) {
        self.asked += 1;
        self.waiting = false;
        self.searched.clear();
        self.results.clear();
        self.error = None;
    }
}

/// The archiver popover: a URL field and the recent archivers.
#[derive(Default)]
pub struct ArchiverEdit {
    pub(super) open: bool,
    url: String,
    error: Option<String>,
    /// Give the URL field the keyboard on the next frame.
    focus: bool,
    /// Where the pill was drawn last.
    pub(super) pill: Option<Rect>,
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
                    plot.autoscale();
                }
                if ui.button("Reset view").clicked() {
                    plot.reset();
                }
                ui.separator();
            }
            let mut log = plot.all_log();
            if ui
                .checkbox(&mut log, "Log scale")
                .on_hover_text("Every value axis; right-click an axis for its own")
                .changed()
            {
                plot.set_log(log);
            }
            let any = !plot.history.is_empty();
            if ui
                .add_enabled(any, egui::Button::new("Clear search history"))
                .clicked()
            {
                plot.history.clear();
                plot.history_changed = true;
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
                     over an axis: drag or shift+wheel zooms it alone\n\
                     chip's dot or drag a chip onto an axis: move its trace\n\
                     middle-click, ctrl+V: add the selected or copied PVs\n\
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
            if icons::button(ui, Icon::Fit, plot.autoscaling())
                .on_hover_text("Autoscale values (A)")
                .clicked()
            {
                plot.autoscale();
            }
        }
        archiver_pill(plot, ui, compact, now);
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

    // The plotted PVs, to tick them in the list.
    let plotted: Vec<String> = plot.traces.iter().map(|t| t.pv.name().key()).collect();

    // Keys for the result list, taken before the text field sees them.
    let s = &mut plot.search;
    let rows = rows(s, plot.archiver.is_some(), &plot.history);
    let mut moved = false;
    if focused {
        let (down, up) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                s.open && i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
            )
        });
        let n = rows.len();
        let at = s.selected.or_else(|| default_row(&rows));
        if !s.open {
            // ↓ in a closed box brings the list back.
            s.open = down;
        } else if n > 0 && (down || up) {
            s.selected = Some(match at {
                Some(i) if down => (i + 1).min(n - 1),
                Some(i) => i.saturating_sub(1),
                None => 0,
            });
            moved = true;
        }
    }
    // The highlighted row: what Enter picks.
    let picked = s
        .selected
        .or_else(|| default_row(&rows))
        .filter(|&i| i < rows.len());

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
            .desired_width(inner.width())
            // Esc is ours: it closes the list before it leaves the box.
            .event_filter(egui::EventFilter {
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
                ..Default::default()
            }),
    );
    let refocus = std::mem::take(&mut s.refocus);
    if std::mem::take(&mut s.focus) || refocus {
        resp.request_focus();
    }
    // A click brings back a list closed with Esc.
    if resp.gained_focus() || resp.clicked() {
        s.open = true;
    }
    if resp.gained_focus() {
        // Coming back to the box selects its text, so typing replaces it
        // (→ or End to edit it instead); not when the list took the focus
        // for a moment.
        s.select_all = !refocus;
    }
    if resp.changed() {
        s.edited_at = now;
        s.selected = None;
        s.open = true;
        s.select_all = false;
    }
    // A middle-click pastes at the click, as on Linux, not over the
    // whole text.
    let middle = ui.input(|i| {
        let m = egui::PointerButton::Middle;
        i.pointer.button_down(m) || i.pointer.button_released(m)
    });
    if middle && resp.contains_pointer() {
        s.select_all = false;
    }
    // Any press or key in the box makes its selection the user's again.
    let touched = ui.input(|i| {
        i.pointer.any_pressed()
            || i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { pressed: true, .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                )
            })
    });
    if !focused || touched {
        plot.paste.auto = None;
    }
    if !focused {
        plot.paste.selected.clear();
    }
    if s.select_all {
        // Until the click that focused the box is let go, unless it drags
        // out a selection of its own.
        if resp.dragged() {
            s.select_all = false;
        } else {
            let all = CCursorRange::two(CCursor::new(0), CCursor::new(s.text.chars().count()));
            set_cursor(ui.ctx(), edit_id, all);
            // Not a selection of the user's: the primary selection keeps
            // theirs, to middle-click into the box.
            plot.paste.auto = Some(s.text.clone());
            s.select_all = ui.input(|i| i.pointer.any_down());
        }
    }
    // A middle-click pastes the primary selection here (the host reads
    // it; egui has already put the cursor there), unless it is on the ✕
    // or the regex toggle (below). Text selected here becomes the primary
    // selection, once the pointer lets go.
    let middle_at = resp
        .clicked_by(egui::PointerButton::Middle)
        .then(|| resp.interact_pointer_pos())
        .flatten();
    if focused && !ui.input(|i| i.pointer.any_down()) {
        let selected = egui::TextEdit::load_state(ui.ctx(), edit_id)
            .and_then(|st| st.cursor.char_range())
            .map_or("", |r| r.slice_str(&s.text));
        let auto = plot.paste.auto.as_deref() == Some(selected);
        if selected != plot.paste.selected && !auto {
            plot.paste.selected = selected.to_string();
            if !selected.is_empty() {
                plot.paste.selection = Some(selected.to_string());
            }
        }
    }

    // The regex toggle, inside the box on the right.
    let toggle = Rect::from_center_size(
        Pos2::new(rect.right() - regex_w / 2.0 - 5.0, rect.center().y),
        Vec2::new(regex_w, h - 8.0),
    );
    let x = if show_regex {
        toggle.left() - clear_w / 2.0
    } else {
        rect.right() - 5.0 - clear_w / 2.0
    };
    let button = Rect::from_center_size(Pos2::new(x, rect.center().y), Vec2::splat(18.0));
    let on_button =
        |p: Pos2| (show_clear && button.contains(p)) || (show_regex && toggle.contains(p));
    if let Some(p) = middle_at
        && !on_button(p)
    {
        plot.paste.middle = Some(MiddleClick::Search);
    }
    #[cfg(test)]
    {
        plot.paste.clear_rect = Some(button);
    }
    if show_clear {
        let r = ui
            .interact(button, plot.id.with("clear"), Sense::click())
            .on_hover_text("Clear");
        let v = ui.visuals();
        if r.hovered() {
            ui.painter()
                .circle_filled(button.center(), 9.0, v.widgets.hovered.weak_bg_fill);
        }
        let color = if r.hovered() {
            v.text_color()
        } else {
            v.weak_text_color()
        };
        Icon::Cross.paint(ui.painter(), button, 11.0, color);
        if r.clicked() {
            clear(s);
            s.refocus = true;
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
            s.refocus = true;
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

    // Enter picks the highlighted row. The box keeps the focus, the text
    // and the list, to add more from the same search.
    let mut pick = None;
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if enter {
        pick = picked;
        resp.request_focus();
    }
    // The list shows when open and it has something to show (the empty
    // box with no recent searches has nothing).
    let shown = s.open && (!rows.is_empty() || !text.is_empty());
    if focused && ui.input(|i| i.key_pressed(Key::Escape)) {
        if shown {
            s.open = false;
        } else {
            clear(s);
            ui.ctx().memory_mut(|m| m.surrender_focus(edit_id));
        }
    }

    // The popup: recent searches, then the archiver's matches.
    let mut forget = None;
    if shown && s.open {
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
                                    let recent = matches!(row, Row::Recent(_));
                                    if recent && i == 0 {
                                        ui.label(egui::RichText::new("Recent").small().weak());
                                    } else if !recent
                                        && i > 0
                                        && matches!(rows[i - 1], Row::Recent(_))
                                    {
                                        ui.separator();
                                    }
                                    let on = Some(i) == picked;
                                    let id = plot.id.with(("search-row", i));
                                    let (r, x) = popup_row(ui, row, on, row.plotted(&plotted), id);
                                    if on && (enter || moved) {
                                        r.scroll_to_me(None);
                                    }
                                    if r.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO)
                                    {
                                        s.selected = Some(i);
                                    }
                                    if r.clicked() {
                                        pick = Some(i);
                                        s.refocus = true;
                                    }
                                    if x {
                                        forget = Some(i);
                                        s.refocus = true;
                                    }
                                }
                            });
                        let note = if text.is_empty() {
                            None
                        } else if s.waiting {
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
        // A click anywhere else closes it, keeping the text.
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

    if let Some(Row::Recent(r)) = forget.map(|i| &rows[i])
        && plot.history.forget(r)
    {
        s.selected = None;
        plot.history_changed = true;
    }
    match pick.map(|i| &rows[i]) {
        // A recent search: back in the box, searched straight away.
        Some(Row::Recent(r)) => {
            s.text = r.text.clone();
            s.regex = r.regex;
            // Ask again, as the regex toggle does: the last answer may be
            // for the same text in the other mode.
            s.searched.clear();
            s.results.clear();
            s.error = None;
            s.edited_at = now - SEARCH_DELAY;
            s.selected = None;
            s.open = true;
            let end = CCursor::new(s.text.chars().count());
            set_cursor(ui.ctx(), edit_id, CCursorRange::one(end));
            ui.ctx().request_repaint();
        }
        // Already plotted: nothing to do.
        Some(row) if !row.plotted(&plotted) => {
            // A search that found something to add is worth remembering.
            if plot.history.record(&text, s.regex) {
                plot.history_changed = true;
            }
            plot.add_pvs(&row.names());
        }
        _ => {}
    }
}

/// Empty the box, the list and the search.
fn clear(s: &mut SearchState) {
    s.text.clear();
    s.searched.clear();
    s.results.clear();
    s.error = None;
    s.waiting = false;
    s.selected = None;
    s.open = false;
}

/// Put the search box's cursor (or selection) at `range`, from the next
/// frame.
fn set_cursor(ctx: &egui::Context, id: egui::Id, range: CCursorRange) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(range));
    state.store(ctx, id);
}

/// One row of the popup, drawn as a selectable button: a recent search
/// has a clock, and a ✕ on hover (returning true when clicked) to forget
/// it; a PV already plotted is dimmed and ticked.
fn popup_row(
    ui: &mut egui::Ui,
    row: &Row,
    picked: bool,
    plotted: bool,
    id: egui::Id,
) -> (egui::Response, bool) {
    let h = 24.0;
    let (rect, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
    let hovered = ui.rect_contains_pointer(rect);
    let v = ui.visuals().clone();
    if picked {
        ui.painter().rect_filled(rect, 6.0, v.selection.bg_fill);
    } else if hovered {
        ui.painter()
            .rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
    }
    let fg = if picked {
        v.selection.stroke.color
    } else {
        v.text_color()
    };
    let weak = if picked {
        fg.gamma_multiply(0.7)
    } else {
        v.weak_text_color()
    };
    let mut left = rect.left() + 8.0;
    let mut right = rect.right() - 4.0;
    let mut forget = false;
    let slot = |x: f32| Rect::from_center_size(Pos2::new(x, rect.center().y), Vec2::splat(18.0));
    match row {
        Row::Recent(recent) => {
            Icon::Clock.paint(ui.painter(), slot(left + 7.0), 13.0, weak);
            left += 22.0;
            let button = slot(right - 9.0);
            right -= 22.0;
            let x = ui
                .interact(button, id.with("forget"), Sense::click())
                .on_hover_text("Forget this search");
            if hovered || x.hovered() {
                if x.hovered() {
                    let fill = if picked {
                        fg.gamma_multiply(0.25)
                    } else {
                        v.widgets.active.weak_bg_fill
                    };
                    ui.painter().circle_filled(button.center(), 9.0, fill);
                }
                let color = if x.hovered() { fg } else { weak };
                Icon::Cross.paint(ui.painter(), button, 11.0, color);
            }
            forget = x.clicked();
            if recent.regex {
                ui.painter().text(
                    Pos2::new(right, rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    ".*",
                    egui::FontId::monospace(12.0),
                    weak,
                );
                right -= 24.0;
            }
        }
        _ if plotted => {
            Icon::Check.paint(ui.painter(), slot(right - 9.0), 13.0, weak);
            right -= 22.0;
        }
        _ => {}
    }
    let font = egui::TextStyle::Button.resolve(ui.style());
    let clip = Rect::from_min_max(rect.min, Pos2::new(right, rect.max.y));
    ui.painter_at(clip).text(
        Pos2::new(left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        row.text(),
        font,
        if plotted { weak } else { fg },
    );
    let r = if plotted {
        r.on_hover_text("Already on the plot")
    } else {
        r
    };
    (r, forget)
}

/// A row of the search popup.
enum Row {
    /// A recent search: picking it puts it back in the box.
    Recent(RecentSearch),
    /// What was typed, added as it is (one or more names).
    Typed(Vec<String>),
    Found(String),
}

impl Row {
    fn names(&self) -> Vec<String> {
        match self {
            Row::Recent(_) => Vec::new(),
            Row::Typed(n) => n.clone(),
            Row::Found(n) => vec![n.clone()],
        }
    }

    fn text(&self) -> String {
        match self {
            Row::Recent(r) => r.text.clone(),
            Row::Typed(n) => format!("+  Add “{}” as typed", n.join("”, “")),
            Row::Found(n) => n.clone(),
        }
    }

    /// Whether all its PVs are on the plot already (`plotted` holds their
    /// `PvName::key`s).
    fn plotted(&self, plotted: &[String]) -> bool {
        let names = self.names();
        !names.is_empty()
            && names
                .iter()
                .all(|n| plotted.contains(&pv::PvName::parse(n.trim()).key()))
    }
}

#[cfg(test)]
impl SearchState {
    /// What the box holds.
    pub(super) fn text(&self) -> &str {
        &self.text
    }
}

/// Put pasted `text` into the search box at its cursor (replacing any
/// selection; at the end if it has no cursor), on one line, and give the
/// box the keyboard.
pub(super) fn paste(plot: &mut Plot, text: &str) {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let edit_id = plot.id.with("search");
    let ctx = plot.repaint.as_ref().map(|r| r.ctx.clone());
    let range = ctx
        .as_ref()
        .and_then(|c| egui::TextEdit::load_state(c, edit_id))
        .and_then(|st| st.cursor.char_range());
    let s = &mut plot.search;
    let chars = s.text.chars().count();
    let (from, to) = range.map_or((chars, chars), |r| {
        let r = r.as_sorted_char_range();
        (
            usize::from(r.start).min(chars),
            usize::from(r.end).min(chars),
        )
    });
    let byte = |c: usize| {
        s.text
            .char_indices()
            .nth(c)
            .map_or(s.text.len(), |(b, _)| b)
    };
    let (a, b) = (byte(from), byte(to));
    s.text.replace_range(a..b, &text);
    if let Some(ctx) = &ctx {
        let end = CCursor::new(from + text.chars().count());
        set_cursor(ctx, edit_id, CCursorRange::one(end));
    }
    s.edited_at = now();
    s.selected = None;
    s.open = true;
    s.select_all = false;
    // The keyboard, without selecting all as a click into the box does.
    s.refocus = true;
    if let Some(r) = &plot.repaint {
        r.ctx.request_repaint_of(r.viewport);
    }
}

/// The recent searches shown above the matches while typing.
const RECENT_SHOWN: usize = 5;

/// The popup's rows. With the box empty, the recent searches. Else up to
/// [`RECENT_SHOWN`] recent searches holding the text, then a match that
/// is exactly the text typed, then the other archiver matches, then the
/// text as typed (unless it is a pattern, or exactly a match). Text that
/// can only be added as typed (a `pva://` or `sim://` name, or several
/// names) comes before the matches instead.
fn rows(s: &SearchState, archiver: bool, history: &SearchHistory) -> Vec<Row> {
    let text = s.text.trim();
    if text.is_empty() {
        return history
            .searches()
            .iter()
            .cloned()
            .map(Row::Recent)
            .collect();
    }
    let mut found: Vec<&String> = if s.searched == text {
        s.results.iter().collect()
    } else {
        Vec::new()
    };
    if let Some(i) = found.iter().position(|r| *r == text) {
        let exact = found.remove(i);
        found.insert(0, exact);
    }
    let typed: Vec<String> = text
        .split([' ', ',', '\t'])
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect();
    let pattern = archiver && (s.regex || text.contains(['*', '?']));
    let mut rows: Vec<Row> = found.into_iter().cloned().map(Row::Found).collect();
    if !typed.is_empty() && !pattern && !s.results.iter().any(|r| r == text) {
        let as_typed = typed.len() > 1 || text.contains("://");
        let row = Row::Typed(typed);
        if as_typed {
            rows.insert(0, row);
        } else {
            rows.push(row);
        }
    }
    let recent = history.matching(text).into_iter().take(RECENT_SHOWN);
    recent.cloned().map(Row::Recent).chain(rows).collect()
}

/// The row highlighted until the arrows move it: the first that adds
/// something, so Enter adds the best match rather than refilling the box
/// with a recent search. None with only recent searches.
fn default_row(rows: &[Row]) -> Option<usize> {
    rows.iter().position(|r| !matches!(r, Row::Recent(_)))
}

/// The popup's rows as shown, for tests.
#[cfg(test)]
pub fn rows_text(plot: &Plot) -> Vec<String> {
    let rows = rows(&plot.search, plot.archiver.is_some(), &plot.history);
    rows.iter().map(Row::text).collect()
}

/// Whether typing has paused for a search not yet sent.
pub fn search_due(plot: &Plot) -> bool {
    let text = plot.search.text.trim();
    plot.archiver.is_some() && !text.is_empty() && text != plot.search.searched
}

/// Take in a finished search, and what it says of the archiver: true
/// when the archiver is back after failing (see `status::Monitor::ok`).
pub fn take_found(plot: &mut Plot, now: f64) -> bool {
    let Some((n, found)) = plot.fetcher.take_found() else {
        return false;
    };
    let s = &mut plot.search;
    if n != s.asked {
        return false;
    }
    s.waiting = false;
    s.selected = None;
    match found {
        Ok(names) => {
            s.results = names;
            s.error = None;
            plot.monitor.ok(now)
        }
        Err(e) => {
            s.results.clear();
            s.error = Some(e.to_string());
            if super::status::archiver_fault(&e, true) {
                plot.monitor.suspect(&e, now);
            }
            false
        }
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

// --- archiver ---------------------------------------------------------------

/// The widest host name the archiver pill shows (points); longer ones are
/// cut short.
const HOST_WIDTH: f32 = 200.0;

/// `● host`: a dot for whether the archiver can be reached (green,
/// red, a spinner while checking, hollow grey for none) and its host.
/// Hover for the URL, version and last error; click to change it. A
/// compact bar shows the dot alone.
fn archiver_pill(plot: &mut Plot, ui: &mut egui::Ui, compact: bool, now: f64) {
    let h = icons::height(ui);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let state = plot.monitor.status.state;
    let label = if compact {
        String::new()
    } else {
        let host = plot.archiver.as_ref().map_or("No archiver", |a| a.host());
        elide(ui.painter(), host, &font, HOST_WIDTH)
    };
    let text_w = super::render::text_size(ui.painter(), &label, &font).x;
    let size = if compact {
        Vec2::splat(h)
    } else {
        Vec2::new(text_w + 36.0, h)
    };
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let v = ui.visuals();
    let dark = v.dark_mode;
    let fill = if r.hovered() || plot.edit.open {
        v.widgets.hovered.weak_bg_fill
    } else {
        v.extreme_bg_color
    };
    ui.painter().rect(
        rect,
        h / 2.0,
        fill,
        v.widgets.noninteractive.bg_stroke,
        StrokeKind::Inside,
    );
    let dot = if compact {
        rect.center()
    } else {
        Pos2::new(rect.left() + 16.0, rect.center().y)
    };
    match state {
        ArchiverState::None => {
            ui.painter()
                .circle_stroke(dot, 3.5, Stroke::new(1.5, v.weak_text_color()));
        }
        ArchiverState::Checking => {
            // A small turning arc, as the status line's.
            let a0 = (ui.input(|i| i.time) * 6.0) as f32;
            let pts: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let a = a0 + i as f32 * 0.45;
                    dot + Vec2::new(a.cos(), a.sin()) * 4.5
                })
                .collect();
            ui.painter().add(egui::Shape::line(
                pts,
                Stroke::new(1.5, v.weak_text_color()),
            ));
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        ArchiverState::Reachable => {
            ui.painter()
                .circle_filled(dot, 4.0, crate::style::reachable(dark));
        }
        ArchiverState::Unreachable => {
            ui.painter()
                .circle_filled(dot, 4.0, crate::style::unreachable(dark));
        }
    }
    if !label.is_empty() {
        let fg = if plot.archiver.is_some() {
            v.text_color()
        } else {
            v.weak_text_color()
        };
        ui.painter().text(
            Pos2::new(dot.x + 10.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &label,
            font,
            fg,
        );
    }
    let r = if plot.edit.open {
        r
    } else {
        r.on_hover_ui(|ui| archiver_tooltip(plot, ui, now))
    };
    if r.clicked() && plot.switch_archivers {
        let e = &mut plot.edit;
        e.open = !e.open;
        e.url = plot
            .archiver
            .as_ref()
            .map(|a| a.base().to_string())
            .unwrap_or_default();
        e.error = None;
        e.focus = e.open;
    }
    plot.edit.pill = Some(rect);
    archiver_popover(plot, ui, rect);
}

/// The archiver pill's tooltip: the URL, the version, the state and the
/// last error.
fn archiver_tooltip(plot: &Plot, ui: &mut egui::Ui, now: f64) {
    let st = &plot.monitor.status;
    let weak = |text: String| egui::RichText::new(text).small().weak();
    let Some(a) = &plot.archiver else {
        ui.label(egui::RichText::new("No archiver").strong());
        ui.label("Live data only, and search adds names as typed.");
        if plot.switch_archivers {
            ui.label(weak("Click to set an archiver".into()));
        }
        return;
    };
    ui.label(egui::RichText::new(a.base()).strong());
    if let Some(version) = &st.version {
        ui.label(version);
    }
    let (state, color) = match st.state {
        ArchiverState::Checking => ("Checking…".to_string(), ui.visuals().text_color()),
        ArchiverState::Reachable => (
            match st.last_ok {
                Some(t) => format!("Reachable (answered {})", ago(now, t)),
                None => "Reachable".into(),
            },
            crate::style::reachable(ui.visuals().dark_mode),
        ),
        ArchiverState::Unreachable => (
            "Unreachable: checked again every minute".to_string(),
            crate::style::unreachable(ui.visuals().dark_mode),
        ),
        ArchiverState::None => (String::new(), ui.visuals().text_color()),
    };
    ui.colored_label(color, state);
    if let Some((e, t)) = &st.last_error {
        // The URL is shown above already.
        let e = e.strip_prefix(&format!("{}: ", a.base())).unwrap_or(e);
        ui.label(format!("Last error {}: {e}", ago(now, *t)));
    }
    if plot.switch_archivers {
        ui.label(weak("Click to change the archiver".into()));
    }
}

/// When `t` was, from `now`: "14:02:10, 3m 5s ago".
fn ago(now: f64, t: f64) -> String {
    let local = format_local(t);
    let clock = local.get(11..).unwrap_or(&local);
    if now - t < 1.0 {
        format!("at {clock}, just now")
    } else {
        format!("at {clock}, {} ago", format_span(now - t))
    }
}

/// `text` cut short with "…" to fit `max` points.
fn elide(painter: &egui::Painter, text: &str, font: &egui::FontId, max: f32) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut shown = text.to_string();
    let mut keep = chars.len();
    while super::render::text_size(painter, &shown, font).x > max && keep > 4 {
        keep -= 1;
        shown = format!("{}…", chars[..keep].iter().collect::<String>());
    }
    shown
}

/// The archiver popover under the pill: a URL field, the recent
/// archivers, and "No archiver".
fn archiver_popover(plot: &mut Plot, ui: &mut egui::Ui, anchor: Rect) {
    if !plot.edit.open {
        return;
    }
    let width = 340.0;
    let pos = Pos2::new(
        (anchor.right() - width).max(ui.ctx().content_rect().left() + 4.0),
        anchor.bottom() + 6.0,
    );
    // The choice: Some(None) for no archiver.
    let mut pick: Option<Option<ArchiverClient>> = None;
    let current = plot.archiver.as_ref().map(|a| a.base().to_string());
    let e = &mut plot.edit;
    let area = egui::Area::new(plot.id.with("archiver-popover"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .corner_radius(10.0)
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.label(egui::RichText::new("Archiver").strong());
                    ui.horizontal(|ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut e.url)
                                .hint_text("http://archiver:17668")
                                .desired_width(width - 110.0),
                        );
                        if std::mem::take(&mut e.focus) {
                            r.request_focus();
                        }
                        let go = ui.button("Use").clicked()
                            || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                        if go {
                            match ArchiverClient::new(&e.url) {
                                Ok(a) => pick = Some(Some(a)),
                                Err(err) => e.error = Some(err.to_string()),
                            }
                        }
                    });
                    if let Some(err) = &e.error {
                        ui.colored_label(ui.visuals().error_fg_color, err);
                    }
                    if !plot.recent.is_empty() {
                        ui.separator();
                        ui.label(egui::RichText::new("Recent").small().weak());
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for url in &plot.recent {
                            let on = current.as_deref() == Some(url.as_str());
                            if ui.selectable_label(on, url).clicked() {
                                match ArchiverClient::new(url) {
                                    Ok(a) => pick = Some(Some(a)),
                                    Err(err) => e.error = Some(err.to_string()),
                                }
                            }
                        }
                        ui.spacing_mut().item_spacing.y = 8.0;
                    }
                    if current.is_some() {
                        ui.separator();
                        if ui
                            .button("No archiver (live only)")
                            .on_hover_text("Show live data only")
                            .clicked()
                        {
                            pick = Some(None);
                        }
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
        plot.edit.open = false;
    }
    if let Some(archiver) = pick {
        plot.edit.open = false;
        if archiver.as_ref().map(|a| a.base()) != current.as_deref() {
            plot.set_archiver(archiver);
            plot.archiver_changed = true;
        } else if archiver.is_some() {
            // The same one again: check it now (it may be back).
            plot.monitor.due = true;
        }
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

    fn none() -> SearchHistory {
        SearchHistory::default()
    }

    #[test]
    fn rows_list_matches_then_the_text() {
        let s = state("SINE", "SINE", &["MOCK:SINE", "MOCK:SINE2"]);
        let r = rows(&s, true, &none());
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].names(), ["MOCK:SINE"]);
        assert_eq!(r[2].names(), ["SINE"]);
        // Stale results are not offered.
        let s = state("SIN", "SINE", &["MOCK:SINE"]);
        assert_eq!(rows(&s, true, &none()).len(), 1);
        // A pattern is not a PV name; several names add together.
        assert_eq!(rows(&state("MOCK:*", "", &[]), true, &none()).len(), 0);
        let r = rows(&state("a:b, c:d", "", &[]), false, &none());
        assert_eq!(r[0].names(), ["a:b", "c:d"]);
        // An exact match is not offered twice.
        assert_eq!(
            rows(
                &state("MOCK:SINE", "MOCK:SINE", &["MOCK:SINE"]),
                true,
                &none()
            )
            .len(),
            1
        );
    }

    #[test]
    fn enter_adds_an_exact_match_before_other_matches() {
        // BL:M1:RBV sorts first, but M1:RBV is what was typed.
        let s = state("M1:RBV", "M1:RBV", &["BL:M1:RBV", "M1:RBV", "M1:RBV2"]);
        let r = rows(&s, true, &none());
        assert_eq!(r[0].names(), ["M1:RBV"]);
        assert_eq!(r.len(), 3, "and not again as typed");
        // A name only addable as typed comes first.
        let s = state("pva://M1:RBV", "pva://M1:RBV", &["X:pva"]);
        assert_eq!(rows(&s, true, &none())[0].names(), ["pva://M1:RBV"]);
    }

    #[test]
    fn recent_searches_come_first_but_enter_adds_a_match() {
        let mut h = none();
        for t in ["SR-DI", "BL16I-EA-*", "BL16I-MO", "BL16I"] {
            h.record(t, false);
        }
        // An empty box lists them all, newest first, none highlighted.
        let r = rows(&state("", "", &[]), true, &h);
        let texts: Vec<String> = r.iter().map(Row::text).collect();
        assert_eq!(texts, ["BL16I", "BL16I-MO", "BL16I-EA-*", "SR-DI"]);
        assert_eq!(default_row(&r), None);
        // Typing: those holding the text (not the text itself), then the
        // matches; Enter would add the first match.
        let r = rows(&state("bl16i", "bl16i", &["BL16I-EA-01"]), true, &h);
        let texts: Vec<String> = r.iter().map(Row::text).collect();
        assert_eq!(texts[..3], ["BL16I-MO", "BL16I-EA-*", "BL16I-EA-01"]);
        assert_eq!(default_row(&r), Some(2));
        assert!(r[0].names().is_empty(), "a recent search adds nothing");
    }

    #[test]
    fn plotted_rows_are_known_by_key() {
        let plotted = vec![pv::PvName::parse("MOCK:SINE").key()];
        assert!(Row::Found("ca://MOCK:SINE".into()).plotted(&plotted));
        assert!(!Row::Found("MOCK:RAMP".into()).plotted(&plotted));
        let both = Row::Typed(vec!["MOCK:SINE".into(), "MOCK:RAMP".into()]);
        assert!(!both.plotted(&plotted));
    }
}
