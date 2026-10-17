//! The `archiveview` command: CLI, logging and the native window around
//! [`archiveview::app::ArchiveApp`]. (Parsers, `Failure` and the logger
//! are bobview's.)

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
use archiveview::{ArchiverClient, app, config, snapshot};

#[cfg(not(target_arch = "wasm32"))]
const AFTER_HELP: &str = "\
History comes from an EPICS Archiver Appliance's retrieval service; live
values from Channel Access, or PV Access for pva:// names. EPICS_CA_ADDR_LIST
and friends select the IOCs. With neither PVs nor --view, the default view
(saved on exit) is restored, in $XDG_CONFIG_HOME/archiveview/views, by
default ~/.config/archiveview/views.

The archiver is --archiver, else none with --no-archiver, else the one the
opened view was saved with, else archiver_url in settings.json. --archiver
is remembered there, so it is needed only once.

In the window: type a PV name (or search the archiver) in the top bar; drag
to pan, scroll to zoom time, shift+scroll or drag the value axis to zoom
values, shift+drag to zoom to a box, double-click to reset. Keys: / search,
arrows pan, +/- zoom, L live, A autoscale, Home reset, Ctrl+Z undo,
Ctrl+Shift+Z redo.

Set ARCHIVEVIEW_LOG=debug for more output.";

/// Plot archived and live EPICS PVs over time.
#[cfg(not(target_arch = "wasm32"))]
#[derive(clap::Parser, Debug)]
#[command(
    name = "archiveview",
    version,
    after_help = AFTER_HELP,
    args_override_self = true,
    max_term_width = 100
)]
struct Cli {
    /// PVs to plot: NAME (Channel Access), pva://NAME, sim://sine, ...
    #[arg(value_name = "PV")]
    pvs: Vec<String>,
    /// The Archiver Appliance retrieval URL, e.g. http://archiver:17668
    /// or Phoebus's pbraw://archiver:17668/retrieval (/retrieval is added
    /// unless given); remembered for later runs
    #[arg(short, long, value_name = "URL")]
    archiver: Option<String>,
    /// Open the saved view NAME
    #[arg(long, value_name = "NAME")]
    view: Option<String>,
    /// Show this span up to now, e.g. 1h, 90m, 7d, "2 days"
    #[arg(short, long, value_name = "SPAN", value_parser = parse_range)]
    range: Option<f64>,
    /// Colour scheme (default: settings.json, else the system's)
    #[arg(long, value_name = "THEME", value_parser = parse_theme)]
    theme: Option<config::Theme>,
    /// Don't use an archiver even if a view or settings.json names one:
    /// live only
    #[arg(long, conflicts_with = "archiver")]
    no_archiver: bool,
    /// Scale the whole interface, e.g. 1.25 (default: ARCHIVEVIEW_UI_SCALE,
    /// else 1)
    #[arg(long, value_name = "F", value_parser = parse_scale_factor)]
    ui_scale: Option<f32>,
    /// Render to a PNG without a window and exit
    #[arg(long, value_name = "OUT.png")]
    snapshot: Option<PathBuf>,
    /// Snapshot size in pixels
    #[arg(long, value_name = "WxH", default_value = "1280x800", value_parser = parse_size)]
    size: [u32; 2],
    /// Snapshot as on a screen with this desktop scale, e.g. 2 for HiDPI
    #[arg(long, value_name = "F", default_value_t = 1.0, value_parser = parse_scale_factor)]
    scale_factor: f32,
    /// Time for PVs to connect before a snapshot (it then also waits for
    /// archive fetches, up to 60 s more)
    #[arg(long, value_name = "SECONDS", default_value_t = 2.0, value_parser = parse_seconds)]
    wait: f64,
    /// Rest the pointer here (pixels) in a snapshot, to show the readout
    #[arg(long, value_name = "X,Y", value_parser = parse_point)]
    hover: Option<[f32; 2]>,
    /// Type TEXT into the search box before a snapshot, to show its matches
    #[arg(long, value_name = "TEXT")]
    search: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_range(text: &str) -> Result<f64, String> {
    archiveview::plot::parse_span(text)
        .ok_or_else(|| format!("expected a span such as 1h, 90m or 7d, not {text:?}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_theme(text: &str) -> Result<config::Theme, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "system" => Ok(config::Theme::System),
        "light" => Ok(config::Theme::Light),
        "dark" => Ok(config::Theme::Dark),
        _ => Err(format!("expected system, light or dark, not {text:?}")),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_size(text: &str) -> Result<[u32; 2], String> {
    text.split_once('x')
        .and_then(|(w, h)| Some([w.trim().parse().ok()?, h.trim().parse().ok()?]))
        .filter(|[w, h]: &[u32; 2]| *w > 0 && *h > 0 && *w <= 16384 && *h <= 16384)
        .ok_or_else(|| format!("expected WIDTHxHEIGHT in pixels, not {text:?}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_scale_factor(text: &str) -> Result<f32, String> {
    text.trim()
        .parse::<f32>()
        .ok()
        .filter(|s| s.is_finite() && *s > 0.0)
        .ok_or_else(|| format!("expected a scale above 0, not {text:?}"))
}

/// A time in seconds: a number, not negative (nor too big for a Duration).
#[cfg(not(target_arch = "wasm32"))]
fn parse_seconds(text: &str) -> Result<f64, String> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|s| (0.0..1e9).contains(s))
        .ok_or_else(|| format!("expected a number of seconds, not {text:?}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_point(text: &str) -> Result<[f32; 2], String> {
    text.split_once(',')
        .and_then(|(x, y)| Some([x.trim().parse().ok()?, y.trim().parse().ok()?]))
        .filter(|[x, y]: &[f32; 2]| x.is_finite() && y.is_finite())
        .ok_or_else(|| format!("expected X,Y in pixels, not {text:?}"))
}

/// Why archiveview stopped early, and the exit status to say so.
#[cfg(not(target_arch = "wasm32"))]
struct Failure {
    code: u8,
    message: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl Failure {
    /// The command line asks for something that cannot be done.
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    use clap::Parser;
    init_logging();
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(f) => {
            eprintln!("archiveview: {}", f.message);
            std::process::ExitCode::from(f.code)
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn run(cli: Cli) -> Result<(), Failure> {
    let archiver = cli
        .archiver
        .as_deref()
        .map(ArchiverClient::new)
        .transpose()
        .map_err(|e| Failure::usage(e.to_string()))?;
    if let Some(name) = &cli.view {
        if !cli.pvs.is_empty() {
            return Err(Failure::usage("give PVs or --view, not both"));
        }
        config::load_view(name).map_err(|e| Failure::usage(format!("view {name}: {e}")))?;
    }
    let opts = app::Options {
        archiver,
        no_archiver: cli.no_archiver,
        pvs: cli.pvs.clone(),
        view: cli.view.clone(),
        range: cli.range,
        restore_default: cli.snapshot.is_none(),
        theme: cli.theme,
        ui_scale: cli.ui_scale.or_else(|| {
            let v = std::env::var("ARCHIVEVIEW_UI_SCALE").ok()?;
            let f = parse_scale_factor(&v);
            if f.is_err() {
                log::warn!("ignoring ARCHIVEVIEW_UI_SCALE={v}: expected a number above 0");
            }
            f.ok()
        }),
    };

    // Remembered, so later runs need no --archiver (but not by snapshots,
    // which are for docs and CI).
    if let Some(a) = &opts.archiver
        && cli.snapshot.is_none()
    {
        let mut settings = config::Settings::load();
        if remember_archiver(&mut settings, a) {
            settings.save();
        }
    }

    if let Some(out) = cli.snapshot {
        let snap = snapshot::Options {
            size: cli.size,
            scale_factor: cli.scale_factor,
            wait: cli.wait,
            dark: cli.theme == Some(config::Theme::Dark),
            hover: cli.hover,
            search: cli.search.clone(),
        };
        return snapshot::run(opts, &snap, &out)
            .map_err(|e| Failure::failed(format!("snapshot failed: {e}")));
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("archiveview")
            .with_app_id("archiveview")
            .with_inner_size([1100.0, 680.0])
            .with_min_inner_size([360.0, 240.0]),
        // We repaint only on change, so no vsync wait is needed (as bobview).
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: false,
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "archiveview",
        options,
        Box::new(move |cc| Ok(Box::new(app::ArchiveApp::new(&cc.egui_ctx, opts)))),
    )
    .map_err(|e| Failure::failed(e.to_string()))
}

/// Make `--archiver`'s `a` the settings' archiver and the first recent
/// one; true if that changed them (so they need saving).
#[cfg(not(target_arch = "wasm32"))]
fn remember_archiver(settings: &mut config::Settings, a: &ArchiverClient) -> bool {
    let before = settings.clone();
    settings.remember_archiver(a.base());
    *settings != before
}

struct Logger(log::LevelFilter);

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        // Library chatter (one line per CA channel connect) only when debugging.
        let library = [
            "eframe", "egui", "epics", "winit", "glutin", "ureq", "rustls",
        ]
        .iter()
        .any(|t| m.target().starts_with(t));
        let limit = if library && self.0 < log::LevelFilter::Debug {
            log::LevelFilter::Warn
        } else {
            self.0
        };
        m.level() <= limit
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("{:<5} {}", r.level(), r.args());
        }
    }

    fn flush(&self) {}
}

fn init_logging() {
    let level = match std::env::var("ARCHIVEVIEW_LOG").as_deref() {
        Ok("debug") => log::LevelFilter::Debug,
        Ok("trace") => log::LevelFilter::Trace,
        Ok("warn") => log::LevelFilter::Warn,
        Ok("error") => log::LevelFilter::Error,
        _ => log::LevelFilter::Info,
    };
    let logger = Box::leak(Box::new(Logger(level)));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(level);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_the_command_line() {
        let cli = Cli::try_parse_from([
            "archiveview",
            "--archiver",
            "http://host:17668",
            "--range",
            "90m",
            "A:B",
            "pva://C",
        ])
        .unwrap();
        assert_eq!(cli.pvs, ["A:B", "pva://C"]);
        assert_eq!(cli.range, Some(5400.0));
        assert_eq!(cli.size, [1280, 800]);
        assert!(Cli::try_parse_from(["archiveview", "--range", "soon"]).is_err());
        assert!(Cli::try_parse_from(["archiveview", "--size", "0x5"]).is_err());
        let cli =
            Cli::try_parse_from(["archiveview", "--theme", "Dark", "--hover", "10,20"]).unwrap();
        assert_eq!(cli.theme, Some(config::Theme::Dark));
        assert_eq!(cli.hover, Some([10.0, 20.0]));
        assert!(Cli::try_parse_from(["archiveview", "--archiver", "h", "--no-archiver"]).is_err());
    }

    #[test]
    fn the_archiver_given_is_remembered() {
        let mut s = config::Settings::default();
        s.remember_archiver("http://old/retrieval");
        let a = ArchiverClient::new("pbraw://archappl.diamond.ac.uk/retrieval").unwrap();
        assert!(remember_archiver(&mut s, &a));
        assert_eq!(
            s.archiver_url.as_deref(),
            Some("http://archappl.diamond.ac.uk/retrieval")
        );
        assert_eq!(
            s.recent_archivers,
            [
                "http://archappl.diamond.ac.uk/retrieval",
                "http://old/retrieval"
            ]
        );
        assert!(!remember_archiver(&mut s, &a), "nothing new to save");
    }
}
