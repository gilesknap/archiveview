//! The standalone application: one [`Plot`] filling the window, plus the
//! view menu, settings and theme. Shared by eframe and snapshot mode.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use archiver::ArchiverClient;
use pv::PvManager;

use crate::config::{self, DEFAULT_VIEW, Settings, Theme};
use crate::plot::Plot;
use crate::view::Range;

/// What the command line asks for.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `--archiver`, else `Settings::archiver_url`, else live only.
    pub archiver: Option<ArchiverClient>,
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
    /// Scale the whole interface (`--ui-scale`, `ARCHIVEVIEW_UI_SCALE`).
    pub ui_scale: Option<f32>,
}

/// A menu choice, applied once the plot has been drawn.
enum Action {
    Save(String),
    Open(String),
    Delete(String),
    Theme(Theme),
}

pub struct ArchiveApp {
    plot: Plot,
    settings: Settings,
    /// A repaint is already asked for: PV updates coalesce into one.
    asked: Arc<AtomicBool>,
    /// The view last opened or saved.
    view_name: Option<String>,
    /// The saved views, as last listed.
    views: Vec<String>,
    /// The "Save as…" field.
    save_as: String,
    /// A message shown at the bottom for a while: text, error?, until.
    notice: Option<(String, bool, f64)>,
    /// The window title last set.
    title: String,
    /// The theme came from the command line: the menu leaves it.
    forced_theme: bool,
    /// `views/default.json` is there but could not be read (corrupt, or
    /// from a newer version): exiting leaves it alone rather than writing
    /// over it, until the user saves the default view themselves.
    keep_default: bool,
}

impl ArchiveApp {
    /// Build the app on `ctx`: fonts, theme, a read-only `PvManager` whose
    /// notifier repaints `ctx`, and the startup view.
    pub fn new(ctx: &egui::Context, opts: Options) -> Self {
        crate::style::install_fonts(ctx);
        let settings = Settings::load();
        apply_theme(ctx, opts.theme.unwrap_or(settings.theme));
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        if let Some(f) = opts.ui_scale {
            ctx.set_zoom_factor(f);
        }

        let asked = Arc::new(AtomicBool::new(false));
        let (c, a) = (ctx.clone(), asked.clone());
        let pvs = PvManager::new();
        pvs.set_read_only(true);
        pvs.set_notifier(Arc::new(move || {
            if !a.swap(true, Ordering::Relaxed) {
                c.request_repaint_after(Duration::from_millis(100));
            }
        }));
        let mut plot = Plot::new(&pvs, opts.archiver);

        let mut view_name = None;
        let mut notice = None;
        if !opts.pvs.is_empty() {
            plot.add_pvs(&opts.pvs);
        } else {
            let name = opts.view.clone().or_else(|| {
                (opts.restore_default && config::view_exists(DEFAULT_VIEW))
                    .then(|| DEFAULT_VIEW.to_string())
            });
            if let Some(name) = name {
                match config::load_view(&name) {
                    Ok(v) => {
                        plot.set_view(&v);
                        view_name = Some(name);
                    }
                    Err(e) => {
                        log::warn!("not opening view {name}: {e}");
                        notice = Some((e, true, f64::INFINITY));
                    }
                }
            }
        }
        if let Some(secs) = opts.range {
            plot.set_range(&Range::Relative { secs });
        }
        let keep_default =
            config::view_exists(DEFAULT_VIEW) && config::load_view(DEFAULT_VIEW).is_err();
        Self {
            plot,
            settings,
            asked,
            view_name,
            views: config::list_views(),
            save_as: String::new(),
            notice,
            title: String::new(),
            forced_theme: opts.theme.is_some(),
            keep_default,
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

        let mut action = None;
        let frame = egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .inner_margin(egui::Margin::same(10));
        let Self {
            plot,
            settings,
            view_name,
            views,
            save_as,
            forced_theme,
            ..
        } = self;
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let mut menu = |ui: &mut egui::Ui| {
                action = view_menu(
                    ui,
                    settings,
                    view_name.as_deref(),
                    views,
                    save_as,
                    *forced_theme,
                );
            };
            plot.ui_with_menu(ui, &mut menu);
        });

        if let Some(action) = action {
            self.apply(&ctx, action, now);
        }
        self.toast(&ctx, now);

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
    /// there could not be read.
    pub fn exit(&mut self) {
        if !self.settings.save_default_on_exit {
            return;
        }
        if self.keep_default {
            log::warn!("not saving the default view over one that could not be read");
        } else if let Err(e) = config::save_view(DEFAULT_VIEW, &self.plot.view()) {
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
                    self.view_name = Some(name.clone());
                    self.save_as.clear();
                    format!("Saved view “{name}”")
                })
            }
            Action::Open(name) => config::load_view(&name).map(|v| {
                self.plot.set_view(&v);
                self.view_name = Some(name.clone());
                format!("Opened view “{name}”")
            }),
            Action::Delete(name) => config::delete_view(&name).map(|()| {
                if name == DEFAULT_VIEW {
                    self.keep_default = false;
                }
                if self.view_name.as_deref() == Some(name.as_str()) {
                    self.view_name = None;
                }
                format!("Deleted view “{name}”")
            }),
            Action::Theme(theme) => {
                self.settings.theme = theme;
                self.settings.save();
                apply_theme(ctx, theme);
                return;
            }
        };
        self.views = config::list_views();
        self.notice = Some(match result {
            Ok(msg) => (msg, false, now + 3.0),
            Err(e) => (e, true, now + 8.0),
        });
    }

    /// The notice, floating over the bottom of the window until it expires
    /// (or is clicked).
    fn toast(&mut self, ctx: &egui::Context, now: f64) {
        let Some((text, error, until)) = self.notice.clone() else {
            return;
        };
        if now > until {
            self.notice = None;
            return;
        }
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
                        ui.add(
                            egui::Label::new(egui::RichText::new(text).color(color))
                                .sense(egui::Sense::click()),
                        )
                    })
                    .inner
            });
        if shown.inner.clicked() {
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
    }
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    ctx.set_theme(match theme {
        Theme::System => egui::ThemePreference::System,
        Theme::Light => egui::ThemePreference::Light,
        Theme::Dark => egui::ThemePreference::Dark,
    });
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
