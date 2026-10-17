//! The standalone application: one [`Plot`] filling the window, plus the
//! view menu, settings and theme. Shared by eframe and snapshot mode.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use archiver::ArchiverClient;
use pv::PvManager;

use crate::config::{self, DEFAULT_VIEW, Settings, Theme};
use crate::plot::{MenuPart, MiddleClick, Plot};
use crate::selection::{MakeSelection, Worker};
use crate::view::{Range, View};

/// How long a middle-click waits for the selection (seconds).
const SELECTION_WAIT: f64 = 2.0;

/// Text selected in the search box becomes the primary selection once it
/// has stayed the same this long (seconds): not at every shift+arrow,
/// which over `ssh -X` costs a round trip each.
const PRIMARY_DEBOUNCE: f64 = 0.15;

/// How long the "Restored last session" notice shows (seconds).
const RESTORED_SECS: f64 = 8.0;

/// The zoom is saved this long after its last change (seconds), not at
/// each step of a run of them.
const ZOOM_SAVE_DELAY: f64 = 0.5;

/// The interface zoom's steps (`−` and `+` in the `≡` menu, ctrl+=/−),
/// as a browser's: round percentages, 50% to 300%.
const ZOOM_STEPS: [f32; 13] = [
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

/// The next zoom step above `zoom` (`up`) or below it; `zoom` itself at
/// either end. From a zoom between steps (`--ui-scale 1.3`), the nearest
/// step that way.
fn zoom_step(zoom: f32, up: bool) -> f32 {
    let next = if up {
        ZOOM_STEPS.iter().find(|&&s| s > zoom + 0.005)
    } else {
        ZOOM_STEPS.iter().rev().find(|&&s| s < zoom - 0.005)
    };
    next.copied().unwrap_or(zoom)
}

/// What the command line asks for.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `--archiver`. Without it the archiver is the restored view's, else
    /// `Settings::archiver_url`, else none (live only): see
    /// [`startup_archiver`].
    pub archiver: Option<ArchiverClient>,
    /// `--no-archiver`: live only, whatever the view or settings say.
    pub no_archiver: bool,
    /// PVs given on the command line.
    pub pvs: Vec<String>,
    /// `--view NAME`.
    pub view: Option<String>,
    /// `--range` (seconds back from now), overriding the view's.
    pub range: Option<f64>,
    /// Open `views/default.json` when neither PVs nor a view are given.
    pub restore_default: bool,
    /// The theme, overriding the settings (`--theme`; snapshots).
    pub theme: Option<Theme>,
    /// Scale the whole interface (`--ui-scale`, `ARCHIVEVIEW_UI_SCALE`):
    /// the zoom to start at, over the one saved in `settings.json`.
    pub ui_scale: Option<f32>,
}

/// A menu choice, applied once the plot has been drawn.
enum Action {
    Save(String),
    Open(String),
    Delete(String),
    Theme(Theme),
    /// The interface zoom (1 is 100%).
    Zoom(f32),
    /// Leave the restored view for an empty plot.
    StartEmpty,
}

/// A message shown at the bottom for a while.
#[derive(Clone)]
struct Notice {
    text: String,
    error: bool,
    /// When it goes (egui time): NaN until first shown, then `secs` on.
    until: f64,
    secs: f64,
    /// It offers "Start empty" (the default view was restored).
    start_empty: bool,
}

impl Notice {
    fn new(text: impl Into<String>, error: bool, secs: f64) -> Self {
        Self {
            text: text.into(),
            error,
            until: f64::NAN,
            secs,
            start_empty: false,
        }
    }
}

/// The archiver to start with, in this order: `--archiver`, none for
/// `--no-archiver`, the restored view's (`default`, or `--view NAME`),
/// then `settings.json`'s `archiver_url`.
pub fn startup_archiver<T>(
    cli: Option<T>,
    no_archiver: bool,
    view: Option<T>,
    settings: Option<T>,
) -> Option<T> {
    match cli {
        Some(a) => Some(a),
        None if no_archiver => None,
        None => view.or(settings),
    }
}

pub struct ArchiveApp {
    plot: Plot,
    settings: Settings,
    /// A repaint is already asked for: PV updates coalesce into one.
    asked: Arc<AtomicBool>,
    /// The saved views, as last listed.
    views: Vec<String>,
    /// The "Save as…" field.
    save_as: String,
    /// A message shown at the bottom for a while.
    notice: Option<Notice>,
    /// The window title last set.
    title: String,
    /// The theme came from the command line: the menu leaves it.
    forced_theme: bool,
    /// `views/default.json` is there but could not be read (corrupt, or
    /// from a newer version): exiting leaves it alone rather than writing
    /// over it, until the user saves the default view themselves.
    keep_default: bool,
    /// The recent searches changed but could not be saved.
    history_unsaved: bool,
    /// `--range`, which "Start empty" keeps too.
    range: Option<f64>,
    /// With `--no-archiver`: the archiver the default view was saved
    /// with, kept when it is saved again on exit, so that one run
    /// without an archiver doesn't forget it.
    kept_archiver: Option<String>,
    /// The archiver in use as last seen, to notice the user switching
    /// (in the pill, or by opening a view) and remember it.
    known_archiver: Option<String>,
    /// The primary selection and the clipboard, for middle-click, on a
    /// thread of their own; none in snapshot mode.
    selection: Option<Worker<(u64, MiddleClick)>>,
    /// Middle-clicks so far, to number them.
    clicks: u64,
    /// The middle-click waiting for the selection: its number, and when
    /// (egui time) it asked.
    reading: Option<(u64, f64)>,
    /// Text selected in the search box, and when, until it is steady
    /// enough to make the primary selection.
    selected: Option<(String, f64)>,
    /// The interface zoom: egui's zoom factor, relative to the desktop's
    /// scale (1 is 100%).
    zoom: f32,
    /// The zoom changed at this time (egui time) and is not saved yet.
    zoom_unsaved: Option<f64>,
    /// Where the `≡` menu's zoom row was drawn: `−`, the percentage, `+`.
    #[cfg(test)]
    zoom_row: [egui::Rect; 3],
}

impl ArchiveApp {
    /// Build the app on `ctx`: fonts, theme, a read-only `PvManager` whose
    /// notifier repaints `ctx`, and the startup view.
    pub fn new(ctx: &egui::Context, opts: Options) -> Self {
        crate::style::install_fonts(ctx);
        let settings = Settings::load();
        apply_theme(ctx, opts.theme.unwrap_or(settings.theme));
        // The app zooms by its own steps, and saves the zoom.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let saved = settings.ui_zoom;
        let saved = if saved.is_finite() && saved > 0.0 {
            saved.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1])
        } else {
            1.0
        };
        let zoom = opts.ui_scale.unwrap_or(saved);
        ctx.set_zoom_factor(zoom);

        let asked = Arc::new(AtomicBool::new(false));
        let (c, a) = (ctx.clone(), asked.clone());
        let pvs = PvManager::new();
        pvs.set_read_only(true);
        pvs.set_notifier(Arc::new(move || {
            if !a.swap(true, Ordering::Relaxed) {
                c.request_repaint_after(Duration::from_millis(100));
            }
        }));

        // PVs on the command line give a new view; else `--view`, else the
        // default view saved last time.
        let mut notice = None;
        let mut opened = None;
        if opts.pvs.is_empty() {
            let name = opts.view.clone().or_else(|| {
                (opts.restore_default && config::view_exists(DEFAULT_VIEW))
                    .then(|| DEFAULT_VIEW.to_string())
            });
            if let Some(name) = name {
                match config::load_view(&name) {
                    Ok(v) => opened = Some((name, v)),
                    Err(e) => {
                        log::warn!("not opening view {name}: {e}");
                        notice = Some(Notice::new(e, true, f64::INFINITY));
                    }
                }
            }
        }

        let url = startup_archiver(
            opts.archiver.as_ref().map(|a| a.base().to_string()),
            opts.no_archiver,
            opened.as_ref().and_then(|(_, v)| v.archiver.clone()),
            settings.archiver_url.clone(),
        );
        let archiver = url.and_then(|u| {
            ArchiverClient::new(&u)
                .inspect_err(|e| {
                    log::warn!("{e}");
                    notice = Some(Notice::new(e.to_string(), true, f64::INFINITY));
                })
                .ok()
        });
        let mut plot = Plot::new(&pvs, archiver);
        plot.set_archiver_switching(true);
        plot.set_recent_archivers(&settings.recent_archivers);
        plot.set_search_history(config::load_history());
        let kept_archiver = if opts.no_archiver {
            match &opened {
                Some((name, v)) if name == DEFAULT_VIEW => v.archiver.clone(),
                _ => config::load_view(DEFAULT_VIEW)
                    .ok()
                    .and_then(|v| v.archiver),
            }
        } else {
            None
        };

        plot.add_pvs(&opts.pvs);
        if let Some((name, mut v)) = opened {
            // Its archiver is chosen above, in its place among the others.
            v.archiver = None;
            plot.set_view(&v);
            if opts.view.is_none() && !v.pvs.is_empty() && notice.is_none() {
                notice = Some(Notice {
                    start_empty: true,
                    ..Notice::new("Restored last session", false, RESTORED_SECS)
                });
            }
            // The view last opened or saved: the plot keeps it with each
            // undo step, so undoing an opened view brings back the name.
            plot.set_view_name(Some(&name));
        }
        if let Some(secs) = opts.range {
            plot.set_range(&Range::Relative { secs });
        }
        let keep_default =
            config::view_exists(DEFAULT_VIEW) && config::load_view(DEFAULT_VIEW).is_err();
        let known_archiver = plot.archiver().map(|a| a.base().to_string());
        Self {
            plot,
            settings,
            asked,
            views: config::list_views(),
            save_as: String::new(),
            notice,
            title: String::new(),
            forced_theme: opts.theme.is_some(),
            keep_default,
            history_unsaved: false,
            range: opts.range,
            kept_archiver,
            known_archiver,
            selection: None,
            clicks: 0,
            reading: None,
            selected: None,
            zoom,
            zoom_unsaved: None,
            #[cfg(test)]
            zoom_row: [egui::Rect::NOTHING; 3],
        }
    }

    /// The interface zoom (1 is 100%).
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Save the zoom in `settings.json`: for now the app's, until there
    /// are windows and layouts, when each window keeps its own.
    fn save_zoom(&mut self) {
        self.zoom_unsaved = None;
        let zoom = self.zoom;
        self.settings.change(|s| s.ui_zoom = zoom);
    }

    /// Ctrl+= (or ctrl++), ctrl+− and ctrl+0, read before the plot sees
    /// the keys: they work while a text field has the focus too (they mean
    /// nothing to one), and leave the plot's +/− (time) alone.
    fn zoom_keys(&self, ctx: &egui::Context) -> Option<Action> {
        use egui::gui_zoom::kb_shortcuts::{ZOOM_IN, ZOOM_IN_SECONDARY, ZOOM_OUT, ZOOM_RESET};
        ctx.input_mut(|i| {
            if i.consume_shortcut(&ZOOM_RESET) {
                Some(Action::Zoom(1.0))
            } else if i.consume_shortcut(&ZOOM_IN) || i.consume_shortcut(&ZOOM_IN_SECONDARY) {
                Some(Action::Zoom(zoom_step(self.zoom, true)))
            } else if i.consume_shortcut(&ZOOM_OUT) {
                Some(Action::Zoom(zoom_step(self.zoom, false)))
            } else {
                None
            }
        })
    }

    /// Remember an archiver the user switched to (in the pill, or by
    /// opening a view that names one) in `settings.json`, as `--archiver`
    /// is. Switching to none ("No archiver") is for this run only: the
    /// settings keep the last archiver.
    fn remember_switch(&mut self) {
        self.plot.take_archiver_change();
        let now = self.plot.archiver().map(|a| a.base().to_string());
        if now == self.known_archiver {
            return;
        }
        self.known_archiver = now.clone();
        if let Some(url) = now {
            self.settings.remember_archiver(&url);
            self.settings.save();
        }
    }

    /// Where middle-click reads the primary selection (or the clipboard),
    /// and the search box's selected text goes: `make` builds it on a
    /// thread of its own (see [`crate::selection::system`]), which wakes
    /// `ctx` when a read is answered.
    pub fn set_selection(&mut self, ctx: &egui::Context, make: Option<MakeSelection>) {
        self.selection = make.map(|make| {
            let ctx = ctx.clone();
            Worker::spawn(make, move || ctx.request_repaint())
        });
        self.reading = None;
    }

    /// Answer the plot's middle-clicks from the selection (read on its
    /// thread, so a slow owner never stalls the window), make text
    /// selected in its search box the primary selection once it is steady,
    /// and say what a paste added.
    fn paste(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(worker) = &self.selection {
            if let Some(click) = self.plot.take_middle_click() {
                self.clicks += 1;
                worker.read((self.clicks, click));
                self.reading = Some((self.clicks, now));
                ctx.request_repaint_after(Duration::from_secs_f64(SELECTION_WAIT));
            }
            while let Some(((n, click), text)) = worker.answer() {
                // Only the latest click's answer, and only in time.
                if self.reading.is_some_and(|(r, _)| r == n) {
                    self.reading = None;
                    let text = text.unwrap_or_default();
                    match click {
                        MiddleClick::Plot => {
                            self.plot.paste_pvs(&text);
                        }
                        MiddleClick::Search => self.plot.paste_into_search(&text),
                    }
                }
            }
            if let Some((_, since)) = self.reading
                && now - since >= SELECTION_WAIT
            {
                self.reading = None;
                let mut notice = Notice::new("Could not read the selection", true, 4.0);
                notice.until = now + notice.secs;
                self.notice = Some(notice);
            }
            // Queued after any read, so a click reads what was selected
            // before it.
            if let Some(text) = self.plot.take_search_selection() {
                self.selected = Some((text, now));
                ctx.request_repaint_after(Duration::from_secs_f64(PRIMARY_DEBOUNCE));
            }
            if self
                .selected
                .as_ref()
                .is_some_and(|(_, since)| now - since >= PRIMARY_DEBOUNCE)
                && let Some((text, _)) = self.selected.take()
            {
                worker.set_primary(text);
            }
        }
        if let Some(pasted) = self.plot.take_pasted() {
            let error = pasted.added.is_empty() && pasted.present.is_empty();
            let mut notice = Notice::new(pasted.message(), error, 3.0);
            notice.until = now + notice.secs;
            self.notice = Some(notice);
        }
    }

    /// The plot.
    pub fn plot(&mut self) -> &mut Plot {
        &mut self.plot
    }

    /// One frame of the whole window.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        self.asked.store(false, Ordering::Relaxed);
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);

        let mut action = self.zoom_keys(&ctx);
        let zoom = self.zoom;
        let frame = egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .inner_margin(egui::Margin::same(10));
        let Self {
            plot,
            settings,
            views,
            save_as,
            forced_theme,
            #[cfg(test)]
            zoom_row,
            ..
        } = self;
        let view_name = plot.view_name().map(str::to_string);
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let mut menu = |ui: &mut egui::Ui, part| {
                let chosen = match part {
                    MenuPart::Top => {
                        let (chosen, _rects) = zoom_menu_row(ui, zoom);
                        #[cfg(test)]
                        {
                            *zoom_row = _rects;
                        }
                        chosen
                    }
                    MenuPart::Main => view_menu(
                        ui,
                        settings,
                        view_name.as_deref(),
                        views,
                        save_as,
                        *forced_theme,
                    ),
                };
                if chosen.is_some() {
                    action = chosen;
                }
            };
            plot.ui_with_menu(ui, &mut menu);
        });
        if let Some(action) = action {
            self.apply(&ctx, action, now);
        }
        self.remember_switch();
        // Saved once per change; a failed save is tried again on exit,
        // not every frame.
        if self.plot.search_history_changed() {
            self.history_unsaved = !config::save_history(self.plot.search_history());
        }
        self.paste(&ctx, now);
        self.toast(&ctx, now);
        if let Some(at) = self.zoom_unsaved {
            if now - at >= ZOOM_SAVE_DELAY {
                self.save_zoom();
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(ZOOM_SAVE_DELAY - (now - at)));
            }
        }

        let title = match self.plot.title() {
            t if t.is_empty() => "archiveview".to_string(),
            t => format!("{t} — archiveview"),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// True while the plot is fetching (snapshot mode waits for this).
    pub fn is_loading(&self) -> bool {
        self.plot.is_loading()
    }

    /// Save the default view when the settings say to, unless the one
    /// there could not be read; and the recent searches, if saving them
    /// last failed.
    pub fn exit(&mut self) {
        if self.zoom_unsaved.is_some() {
            self.save_zoom();
        }
        if self.history_unsaved {
            self.history_unsaved = !config::save_history(self.plot.search_history());
        }
        if !self.settings.save_default_on_exit {
            return;
        }
        if self.keep_default {
            log::warn!("not saving the default view over one that could not be read");
            return;
        }
        let mut view = self.plot.view();
        if view.archiver.is_none() {
            view.archiver = self.kept_archiver.clone();
        }
        if let Err(e) = config::save_view(DEFAULT_VIEW, &view) {
            log::error!("{e}");
        }
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action, now: f64) {
        let result = match action {
            Action::Save(name) => {
                let name = name.trim().to_string();
                config::save_view(&name, &self.plot.view()).map(|()| {
                    if name == DEFAULT_VIEW {
                        self.keep_default = false;
                    }
                    self.plot.set_view_name(Some(&name));
                    self.save_as.clear();
                    format!("Saved view “{name}”")
                })
            }
            Action::Open(name) => config::load_view(&name).map(|v| {
                let before = self.plot.archiver().cloned();
                self.plot.set_view(&v);
                self.plot.set_view_name(Some(&name));
                match self.plot.archiver() {
                    Some(a) if before.as_ref() != Some(a) => {
                        format!("Opened view “{name}” from {}", a.host())
                    }
                    _ => format!("Opened view “{name}”"),
                }
            }),
            Action::Delete(name) => config::delete_view(&name).map(|()| {
                if name == DEFAULT_VIEW {
                    self.keep_default = false;
                }
                if self.plot.view_name() == Some(name.as_str()) {
                    self.plot.set_view_name(None);
                }
                format!("Deleted view “{name}”")
            }),
            Action::Theme(theme) => {
                self.settings.change(|s| s.theme = theme);
                apply_theme(ctx, theme);
                return;
            }
            Action::Zoom(zoom) => {
                // Saved shortly (a run of steps once), even when unchanged:
                // ctrl+0 at a --ui-scale of 1 still saves 100%.
                self.zoom = zoom;
                ctx.set_zoom_factor(zoom);
                self.zoom_unsaved = Some(now);
                let end = if zoom >= ZOOM_STEPS[ZOOM_STEPS.len() - 1] {
                    " (largest)"
                } else if zoom <= ZOOM_STEPS[0] {
                    " (smallest)"
                } else {
                    ""
                };
                let text = format!("Zoom {}{end}", percent(zoom));
                let mut notice = Notice::new(text, false, 1.5);
                notice.until = now + notice.secs;
                self.notice = Some(notice);
                return;
            }
            Action::StartEmpty => {
                // An empty view keeps the archiver, and `--range`.
                let range = self.range.map(|secs| Range::Relative { secs });
                self.plot.set_view(&View {
                    range: range.unwrap_or_default(),
                    ..View::default()
                });
                self.plot.set_view_name(None);
                self.notice = None;
                return;
            }
        };
        self.views = config::list_views();
        let mut notice = match result {
            Ok(msg) => Notice::new(msg, false, 3.0),
            Err(e) => Notice::new(e, true, 8.0),
        };
        notice.until = now + notice.secs;
        self.notice = Some(notice);
    }

    /// The notice, floating over the bottom of the window until it expires
    /// (or is clicked).
    fn toast(&mut self, ctx: &egui::Context, now: f64) {
        let Some(notice) = &mut self.notice else {
            return;
        };
        if notice.until.is_nan() {
            notice.until = now + notice.secs;
        }
        let Notice {
            text,
            error,
            until,
            start_empty,
            ..
        } = notice.clone();
        if now > until {
            self.notice = None;
            return;
        }
        let mut empty = false;
        let shown = egui::Area::new(egui::Id::new("archiveview-notice"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -56.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(16.0)
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        let color = if error {
                            ui.visuals().error_fg_color
                        } else {
                            ui.visuals().text_color()
                        };
                        ui.horizontal(|ui| {
                            let r = ui.add(
                                egui::Label::new(egui::RichText::new(text).color(color))
                                    .sense(egui::Sense::click()),
                            );
                            if start_empty
                                && ui
                                    .button("Start empty")
                                    .on_hover_text("Clear the plot (the archiver stays)")
                                    .clicked()
                            {
                                empty = true;
                            }
                            r
                        })
                        .inner
                    })
                    .inner
            });
        if empty {
            self.apply(ctx, Action::StartEmpty, now);
        } else if shown.inner.clicked() {
            self.notice = None;
        } else if until.is_finite() {
            ctx.request_repaint_after(Duration::from_secs_f64((until - now).max(0.05)));
        }
    }
}

impl eframe::App for ArchiveApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.exit();
        // Before the window (and a Wayland connection) goes.
        self.selection = None;
    }
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    ctx.set_theme(match theme {
        Theme::System => egui::ThemePreference::System,
        Theme::Light => egui::ThemePreference::Light,
        Theme::Dark => egui::ThemePreference::Dark,
    });
}

/// A zoom as a percentage, e.g. "125%".
fn percent(zoom: f32) -> String {
    format!("{:.0}%", zoom * 100.0)
}

/// The top of the `≡` menu: `−  100%  +`, the interface zoom. Clicking the
/// percentage goes back to 100%. Also where the three were drawn.
fn zoom_menu_row(ui: &mut egui::Ui, zoom: f32) -> (Option<Action>, [egui::Rect; 3]) {
    let mut action = None;
    let max = ZOOM_STEPS[ZOOM_STEPS.len() - 1];
    let row = ui.horizontal(|ui| {
        ui.label("Zoom");
        let minus = ui
            .add_enabled(zoom > ZOOM_STEPS[0] + 0.005, egui::Button::new("−"))
            .on_hover_text("Zoom out (Ctrl+−)");
        if minus.clicked() {
            action = Some(Action::Zoom(zoom_step(zoom, false)));
        }
        let pct = ui
            .add(egui::Button::new(percent(zoom)).min_size(egui::vec2(52.0, 0.0)))
            .on_hover_text("Back to 100% (Ctrl+0)");
        if pct.clicked() {
            action = Some(Action::Zoom(1.0));
        }
        let plus = ui
            .add_enabled(zoom < max - 0.005, egui::Button::new("+"))
            .on_hover_text("Zoom in (Ctrl+=)");
        if plus.clicked() {
            action = Some(Action::Zoom(zoom_step(zoom, true)));
        }
        [minus.rect, pct.rect, plus.rect]
    });
    ui.separator();
    (action, row.inner)
}

/// The app's part of the `≡` menu: views and the theme.
fn view_menu(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    current: Option<&str>,
    views: &[String],
    save_as: &mut String,
    forced_theme: bool,
) -> Option<Action> {
    let mut action = None;
    ui.label(egui::RichText::new("Views").small().weak());
    if let Some(name) = current
        && ui.button(format!("Save “{name}”")).clicked()
    {
        action = Some(Action::Save(name.to_string()));
    }
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(save_as)
                .hint_text("Save as…")
                .desired_width(140.0),
        );
        let ok = config::valid_name(save_as);
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.add_enabled(ok, egui::Button::new("Save")).clicked() || enter) && ok {
            action = Some(Action::Save(save_as.clone()));
        }
    });
    if ui.button("Save as default").clicked() {
        action = Some(Action::Save(DEFAULT_VIEW.to_string()));
    }
    ui.add_enabled_ui(!views.is_empty(), |ui| {
        ui.menu_button("Open", |ui| {
            for name in views.iter() {
                if ui
                    .selectable_label(Some(name.as_str()) == current, name)
                    .clicked()
                {
                    action = Some(Action::Open(name.clone()));
                }
            }
        });
        ui.menu_button("Delete", |ui| {
            for name in views.iter() {
                if ui.button(name).clicked() {
                    action = Some(Action::Delete(name.clone()));
                }
            }
        });
    });
    if ui
        .checkbox(&mut settings.save_default_on_exit, "Save default on exit")
        .changed()
    {
        settings.save();
    }
    if !forced_theme {
        ui.separator();
        ui.label(egui::RichText::new("Theme").small().weak());
        ui.horizontal(|ui| {
            for (theme, name) in [
                (Theme::System, "System"),
                (Theme::Light, "Light"),
                (Theme::Dark, "Dark"),
            ] {
                if ui.selectable_label(settings.theme == theme, name).clicked() {
                    action = Some(Action::Theme(theme));
                }
            }
        });
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_archiver_is_chosen_in_order() {
        // (--archiver, --no-archiver, the view's, the settings') → chosen
        let table = [
            (
                Some("cli"),
                false,
                Some("view"),
                Some("settings"),
                Some("cli"),
            ),
            (Some("cli"), false, None, None, Some("cli")),
            (None, true, Some("view"), Some("settings"), None),
            (None, true, None, Some("settings"), None),
            (None, false, Some("view"), Some("settings"), Some("view")),
            (None, false, Some("view"), None, Some("view")),
            (None, false, None, Some("settings"), Some("settings")),
            (None, false, None, None, None),
        ];
        for (cli, no, view, settings, want) in table {
            assert_eq!(
                startup_archiver(cli, no, view, settings),
                want,
                "{cli:?} {no} {view:?} {settings:?}"
            );
        }
    }

    /// A selection the test fills, sharing what was set with the test.
    struct FakeSelection {
        /// There is a primary selection (`primary` may still be `None`).
        has: bool,
        primary: Option<String>,
        clipboard: Option<String>,
        set: Arc<parking_lot::Mutex<Vec<String>>>,
        /// Take this long to answer, as a hung selection owner does.
        slow: Duration,
    }

    impl crate::selection::Selection for FakeSelection {
        fn has_primary(&mut self) -> bool {
            self.has
        }
        fn primary(&mut self) -> Option<String> {
            std::thread::sleep(self.slow);
            self.primary.clone()
        }
        fn clipboard(&mut self) -> Option<String> {
            self.clipboard.clone()
        }
        fn set_primary(&mut self, text: &str) {
            self.set.lock().push(text.to_string());
        }
    }

    impl Drop for FakeSelection {
        /// A slow one says when it is dropped (it should be leaked).
        fn drop(&mut self) {
            if !self.slow.is_zero() {
                self.set.lock().push("dropped".into());
            }
        }
    }

    #[test]
    fn middle_click_adds_the_selected_pvs_and_says_so() {
        let (_lock, dir) = config_home("middle");
        let ctx = egui::Context::default();
        let mut app = ArchiveApp::new(&ctx, Options::default());
        app.plot.set_archiver(None);
        let t = std::cell::Cell::new(0.0);
        let frame = |app: &mut ArchiveApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                time: Some(t.get()),
                events,
                ..Default::default()
            };
            t.set(t.get() + 0.05);
            let mut out = ctx.run_ui(input, |ui| app.draw(ui));
            out.textures_delta.clear();
        };
        // A middle-click, then frames until the selection's answer is in.
        let click = |app: &mut ArchiveApp, pos| {
            let ev = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Middle,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(app, vec![egui::Event::PointerMoved(pos), ev(true)]);
            frame(app, vec![ev(false)]);
            assert!(app.reading.is_some(), "asked for the selection");
            let start = std::time::Instant::now();
            while app.reading.is_some() {
                assert!(start.elapsed().as_secs() < 5, "no answer");
                std::thread::sleep(Duration::from_millis(10));
                frame(app, vec![]);
            }
            frame(app, vec![]);
        };
        let set = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let select =
            |app: &mut ArchiveApp, has: bool, primary: Option<&str>, clip: Option<&str>| {
                let fake = FakeSelection {
                    has,
                    primary: primary.map(Into::into),
                    clipboard: clip.map(Into::into),
                    set: set.clone(),
                    slow: Duration::ZERO,
                };
                app.set_selection(&ctx, Some(Box::new(move || Box::new(fake))));
            };
        let on_plot = egui::pos2(500.0, 300.0);
        frame(&mut app, vec![]);

        // The primary selection's PVs, with a toast.
        let two = Some("BL16I-EA-DET-01:COUNT\nSR:X");
        select(&mut app, true, two, Some("CLIP:BOARD"));
        click(&mut app, on_plot);
        assert_eq!(app.plot.names(), ["BL16I-EA-DET-01:COUNT", "SR:X"]);
        assert_eq!(app.notice.as_ref().unwrap().text, "Added 2 PVs");
        // An empty primary selection is not the clipboard's cue.
        select(&mut app, true, None, Some("CLIP:BOARD"));
        click(&mut app, on_plot);
        assert_eq!(app.plot.names().len(), 2);
        assert!(app.notice.as_ref().unwrap().text.contains("empty"));
        // No primary selection at all: the clipboard.
        select(&mut app, false, None, Some("CLIP:BOARD"));
        click(&mut app, on_plot);
        assert_eq!(app.plot.names().last().unwrap(), "CLIP:BOARD");
        assert_eq!(app.notice.as_ref().unwrap().text, "Added CLIP:BOARD");
        // Nothing usable: the toast says so.
        let junk = Some("42 12:30:00 https://example.com/x");
        select(&mut app, true, junk, None);
        click(&mut app, on_plot);
        assert_eq!(app.plot.names().len(), 3);
        let notice = app.notice.as_ref().unwrap();
        assert!(notice.error && notice.text.contains("No PV names"));

        // In the search box: pasted there, not added.
        select(&mut app, true, Some("SR-DI-DCCT-01:SIGNAL"), None);
        click(&mut app, egui::pos2(80.0, 22.0));
        assert_eq!(app.plot.names().len(), 3);
        // Selecting it there makes it the primary selection, once steady.
        let select_all = egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        frame(&mut app, vec![select_all]);
        frame(&mut app, vec![]);
        assert!(set.lock().is_empty(), "not yet");
        for _ in 0..6 {
            frame(&mut app, vec![]);
        }
        let start = std::time::Instant::now();
        while set.lock().is_empty() && start.elapsed().as_secs() < 5 {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(*set.lock(), ["SR-DI-DCCT-01:SIGNAL"]);

        // A selection owner that never answers: the window goes on, and
        // after a while says so.
        let late = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let fake = FakeSelection {
            has: true,
            primary: Some("LATE:PV".into()),
            clipboard: None,
            set: late.clone(),
            slow: Duration::from_millis(1500),
        };
        app.set_selection(&ctx, Some(Box::new(move || Box::new(fake))));
        let ev = |pressed| egui::Event::PointerButton {
            pos: on_plot,
            button: egui::PointerButton::Middle,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let before = std::time::Instant::now();
        frame(&mut app, vec![egui::Event::PointerMoved(on_plot), ev(true)]);
        frame(&mut app, vec![ev(false)]);
        // 2.5 s of egui time pass in frames, quickly.
        for _ in 0..50 {
            frame(&mut app, vec![]);
        }
        assert!(before.elapsed().as_secs_f64() < 1.0, "never blocked");
        assert!(app.reading.is_none());
        let notice = app.notice.as_ref().unwrap();
        assert!(notice.error && notice.text.contains("Could not read"));
        assert_eq!(app.plot.names().len(), 3, "the late answer is dropped");
        // Still stuck when dropped (as at exit): given up on, and when it
        // wakes it leaks the selection rather than dropping it.
        app.set_selection(&ctx, None);
        std::thread::sleep(Duration::from_millis(1500));
        assert!(late.lock().is_empty(), "{:?}", late.lock());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Point `XDG_CONFIG_HOME` at an empty directory of the test's own,
    /// holding a lock so that the tests that do this take turns.
    fn config_home(name: &str) -> (parking_lot::MutexGuard<'static, ()>, std::path::PathBuf) {
        static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
        let lock = LOCK.lock();
        let dir = std::env::temp_dir().join(format!("archiveview-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        // SAFETY: only these tests set the environment, in turn, and only
        // they read it (the config module's tests use directories they are
        // given).
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
        (lock, dir)
    }

    #[test]
    fn zoom_steps() {
        assert_eq!(zoom_step(1.0, true), 1.1);
        assert_eq!(zoom_step(1.0, false), 0.9);
        assert_eq!(zoom_step(3.0, true), 3.0, "the top");
        assert_eq!(zoom_step(0.5, false), 0.5, "the bottom");
        // Between steps (--ui-scale 1.3): the nearest that way.
        assert_eq!(zoom_step(1.3, true), 1.5);
        assert_eq!(zoom_step(1.3, false), 1.25);
        // Beyond them: back in.
        assert_eq!(zoom_step(4.0, false), 3.0);
        assert_eq!(zoom_step(0.25, true), 0.5);
        assert_eq!(percent(0.67), "67%");
    }

    #[test]
    fn the_interface_zooms_from_the_keys_and_the_menu() {
        let (_lock, dir) = config_home("zoom");
        let ctx = egui::Context::default();
        let mut app = ArchiveApp::new(&ctx, Options::default());
        app.plot.set_archiver(None);
        let t = std::cell::Cell::new(0.0);
        let frame = |app: &mut ArchiveApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                time: Some(t.get()),
                events,
                ..Default::default()
            };
            t.set(t.get() + 0.05);
            let mut out = ctx.run_ui(input, |ui| app.draw(ui));
            out.textures_delta.clear();
        };
        let key = |app: &mut ArchiveApp, key, modifiers| {
            let ev = |pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            };
            frame(app, vec![ev(true)]);
            frame(app, vec![ev(false)]);
        };
        let ctrl = |app: &mut ArchiveApp, k| key(app, k, egui::Modifiers::COMMAND);
        let click = |app: &mut ArchiveApp, at: egui::Pos2| {
            let ev = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(app, vec![egui::Event::PointerMoved(at)]);
            frame(app, vec![ev(true)]);
            frame(app, vec![ev(false)]);
            frame(app, vec![]);
        };
        let saved = || Settings::load().ui_zoom;
        // Long enough (in egui time) for a change to be saved.
        let wait = |app: &mut ArchiveApp| {
            for _ in 0..12 {
                frame(app, vec![]);
            }
        };
        frame(&mut app, vec![]);
        assert_eq!(app.zoom(), 1.0);
        assert_eq!(ctx.zoom_factor(), 1.0);

        // The keys, as a browser's, saved and said.
        ctrl(&mut app, egui::Key::Equals);
        assert_eq!(app.zoom(), 1.1);
        assert_eq!(ctx.zoom_factor(), 1.1);
        assert_eq!(app.notice.as_ref().unwrap().text, "Zoom 110%");
        assert_eq!(saved(), 1.0, "not at once");
        // Another archiveview saves an archiver meanwhile: kept.
        let mut other = Settings::load();
        other.remember_archiver("http://other/retrieval");
        other.save();
        wait(&mut app);
        assert_eq!(saved(), 1.1);
        let back = Settings::load().archiver_url;
        assert_eq!(back.as_deref(), Some("http://other/retrieval"));
        // Ctrl++ (shift held for the +) too.
        key(
            &mut app,
            egui::Key::Plus,
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        );
        assert_eq!(app.zoom(), 1.25);
        ctrl(&mut app, egui::Key::Minus);
        ctrl(&mut app, egui::Key::Minus);
        ctrl(&mut app, egui::Key::Minus);
        assert_eq!(app.zoom(), 0.9);
        assert_eq!(app.notice.as_ref().unwrap().text, "Zoom 90%");
        ctrl(&mut app, egui::Key::Num0);
        assert_eq!(app.zoom(), 1.0);
        wait(&mut app);
        assert_eq!(saved(), 1.0);
        // Not the plot's time zoom.
        let range = app.plot.view().range;
        ctrl(&mut app, egui::Key::Equals);
        assert_eq!(app.plot.view().range, range);
        ctrl(&mut app, egui::Key::Num0);

        // With the search box focused, too.
        let on_plot = egui::pos2(500.0, 300.0);
        frame(&mut app, vec![egui::Event::PointerMoved(on_plot)]);
        key(&mut app, egui::Key::Slash, egui::Modifiers::NONE);
        assert!(ctx.memory(|m| m.focused().is_some()), "the search box");
        ctrl(&mut app, egui::Key::Equals);
        assert_eq!(app.zoom(), 1.1);
        ctrl(&mut app, egui::Key::Minus);
        ctrl(&mut app, egui::Key::Minus);
        assert_eq!(app.zoom(), 0.9);
        ctrl(&mut app, egui::Key::Num0);
        assert_eq!(app.zoom(), 1.0);
        assert!(ctx.memory(|m| m.focused().is_some()), "still focused");
        key(&mut app, egui::Key::Escape, egui::Modifiers::NONE);
        key(&mut app, egui::Key::Escape, egui::Modifiers::NONE);

        // The row at the top of ≡: −, the percentage (back to 100%), +.
        // (A click on one closes the menu: open it each time.)
        let menu = egui::pos2(1000.0 - 10.0 - 16.0, 10.0 + 14.0);
        let pick = |app: &mut ArchiveApp, which: usize| {
            app.zoom_row = [egui::Rect::NOTHING; 3];
            click(app, menu);
            let at = app.zoom_row[which];
            assert!(at.is_positive(), "the menu shows the row");
            click(app, at.center());
        };
        pick(&mut app, 2);
        assert_eq!(app.zoom(), 1.1);
        pick(&mut app, 2);
        assert_eq!(app.zoom(), 1.25);
        wait(&mut app);
        assert_eq!(saved(), 1.25);
        pick(&mut app, 1);
        assert_eq!(app.zoom(), 1.0);
        pick(&mut app, 0);
        assert_eq!(app.zoom(), 0.9);
        assert_eq!(app.notice.as_ref().unwrap().text, "Zoom 90%");

        // At the ends: it says so.
        for _ in 0..12 {
            ctrl(&mut app, egui::Key::Equals);
        }
        assert_eq!(app.zoom(), 3.0);
        assert_eq!(app.notice.as_ref().unwrap().text, "Zoom 300% (largest)");
        for _ in 0..12 {
            ctrl(&mut app, egui::Key::Minus);
        }
        assert_eq!(app.zoom(), 0.5);
        assert_eq!(app.notice.as_ref().unwrap().text, "Zoom 50% (smallest)");
        ctrl(&mut app, egui::Key::Plus);
        ctrl(&mut app, egui::Key::Plus);
        ctrl(&mut app, egui::Key::Plus);
        // Exit saves what is not saved yet.
        app.exit();
        assert_eq!(saved(), 0.8);

        // The next start begins at the saved zoom; --ui-scale wins.
        let app = ArchiveApp::new(&ctx, Options::default());
        assert_eq!(app.zoom(), 0.8);
        let mut app = ArchiveApp::new(
            &ctx,
            Options {
                ui_scale: Some(1.0),
                ..Options::default()
            },
        );
        assert_eq!(app.zoom(), 1.0);
        // Ctrl+0 there changes nothing on screen, but saves 100%.
        frame(&mut app, vec![]);
        ctrl(&mut app, egui::Key::Num0);
        wait(&mut app);
        assert_eq!(saved(), 1.0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_default_view_comes_back_with_its_archiver() {
        let (_lock, dir) = config_home("app");
        let base = |o: &ArchiveApp| o.plot.archiver().map(|a| a.base().to_string());
        let view_url = "http://view:17668/retrieval";
        let settings_url = "http://settings:17668/retrieval";
        let mut settings = Settings::default();
        settings.remember_archiver(settings_url);
        settings.save();
        let view = View {
            pvs: vec![crate::view::TraceView {
                name: "sim://sine".into(),
                color: None,
                visible: true,
                axis: None,
            }],
            archiver: Some(view_url.into()),
            ..View::default()
        };
        config::save_view(DEFAULT_VIEW, &view).unwrap();
        let ctx = egui::Context::default();
        let restore = Options {
            restore_default: true,
            ..Options::default()
        };

        // Restored, with the view's archiver, and said so.
        let app = ArchiveApp::new(&ctx, restore.clone());
        assert_eq!(base(&app), Some(view_url.into()));
        assert_eq!(app.plot.names(), ["sim://sine"]);
        let notice = app.notice.as_ref().expect("a notice");
        assert!(notice.start_empty && notice.text.contains("Restored"));
        assert_eq!(
            app.plot.recent_archivers(),
            [view_url, settings_url],
            "the pill offers both"
        );
        // A run with --no-archiver doesn't forget the default's archiver.
        let mut app = ArchiveApp::new(
            &ctx,
            Options {
                no_archiver: true,
                range: Some(600.0),
                ..restore.clone()
            },
        );
        assert_eq!(base(&app), None);
        app.exit();
        let saved = config::load_view(DEFAULT_VIEW).unwrap();
        assert_eq!(saved.archiver.as_deref(), Some(view_url));
        // "Start empty" keeps --range.
        app.apply(&ctx, Action::StartEmpty, 0.0);
        assert_eq!(app.plot.view().range, Range::Relative { secs: 600.0 });
        // Opening a view with another archiver is remembered, as a
        // switch in the pill is; switching to none is not.
        let other = View {
            archiver: Some("http://other:17668/retrieval".into()),
            ..View::default()
        };
        config::save_view("other", &other).unwrap();
        app.apply(&ctx, Action::Open("other".into()), 0.0);
        app.remember_switch();
        assert_eq!(
            Settings::load().archiver_url.as_deref(),
            Some("http://other:17668/retrieval")
        );
        app.plot.set_archiver(None);
        app.remember_switch();
        assert_eq!(
            Settings::load().archiver_url.as_deref(),
            Some("http://other:17668/retrieval")
        );
        config::delete_view("other").unwrap();
        settings.save();

        // --archiver wins, and --no-archiver.
        let cli = ArchiverClient::new("http://cli:17668").unwrap();
        let app = ArchiveApp::new(
            &ctx,
            Options {
                archiver: Some(cli.clone()),
                ..restore.clone()
            },
        );
        assert_eq!(base(&app), Some(cli.base().into()));
        let app = ArchiveApp::new(
            &ctx,
            Options {
                no_archiver: true,
                ..restore.clone()
            },
        );
        assert_eq!(base(&app), None);
        // PVs on the command line replace the view: the settings' archiver.
        let mut app = ArchiveApp::new(
            &ctx,
            Options {
                pvs: vec!["sim://ramp".into()],
                ..restore.clone()
            },
        );
        assert_eq!(base(&app), Some(settings_url.into()));
        assert_eq!(app.plot.names(), ["sim://ramp"]);
        assert!(app.notice.is_none());
        // Exit saves the default with the archiver in use.
        app.exit();
        let saved = config::load_view(DEFAULT_VIEW).unwrap();
        assert_eq!(saved.archiver.as_deref(), Some(settings_url));
        assert_eq!(saved.pvs[0].name, "sim://ramp");
        // "Start empty" keeps the archiver.
        app.apply(&ctx, Action::StartEmpty, 0.0);
        assert!(app.plot.names().is_empty());
        assert_eq!(base(&app), Some(settings_url.into()));
        std::fs::remove_dir_all(&dir).ok();
    }
}
