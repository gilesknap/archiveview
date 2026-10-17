//! The standalone application: one [`Plot`] filling the window, plus the
//! view menu, settings and theme. Shared by eframe and snapshot mode.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use archiver::ArchiverClient;
use pv::PvManager;

use crate::config::{self, DEFAULT_VIEW, Settings, Theme};
use crate::plot::Plot;
use crate::view::{Range, View};

/// How long the "Restored last session" notice shows (seconds).
const RESTORED_SECS: f64 = 8.0;

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
    /// Scale the whole interface (`--ui-scale`, `ARCHIVEVIEW_UI_SCALE`).
    pub ui_scale: Option<f32>,
}

/// A menu choice, applied once the plot has been drawn.
enum Action {
    Save(String),
    Open(String),
    Delete(String),
    Theme(Theme),
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
    /// The view last opened or saved.
    view_name: Option<String>,
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
    /// `--range`, which "Start empty" keeps too.
    range: Option<f64>,
    /// With `--no-archiver`: the archiver the default view was saved
    /// with, kept when it is saved again on exit, so that one run
    /// without an archiver doesn't forget it.
    kept_archiver: Option<String>,
    /// The archiver in use as last seen, to notice the user switching
    /// (in the pill, or by opening a view) and remember it.
    known_archiver: Option<String>,
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

        let mut view_name = None;
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
            view_name = Some(name);
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
            view_name,
            views: config::list_views(),
            save_as: String::new(),
            notice,
            title: String::new(),
            forced_theme: opts.theme.is_some(),
            keep_default,
            range: opts.range,
            kept_archiver,
            known_archiver,
        }
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
        self.remember_switch();
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
                    self.view_name = Some(name.clone());
                    self.save_as.clear();
                    format!("Saved view “{name}”")
                })
            }
            Action::Open(name) => config::load_view(&name).map(|v| {
                let before = self.plot.archiver().cloned();
                self.plot.set_view(&v);
                self.view_name = Some(name.clone());
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
            Action::StartEmpty => {
                // An empty view keeps the archiver, and `--range`.
                let range = self.range.map(|secs| Range::Relative { secs });
                self.plot.set_view(&View {
                    range: range.unwrap_or_default(),
                    ..View::default()
                });
                self.view_name = None;
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

    #[test]
    fn the_default_view_comes_back_with_its_archiver() {
        let dir = std::env::temp_dir().join(format!("archiveview-app-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        // SAFETY: the only test that sets the environment; the others
        // read their config from directories they are given.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
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
