//! Settings and saved views in `$XDG_CONFIG_HOME/archiveview` (by default
//! `~/.config/archiveview`), the same way bobview keeps `~/.config/bobview`:
//!
//! - `settings.json`: [`Settings`].
//! - `views/<name>.json`: a [`View`]; `views/default.json` opens at startup
//!   when no PVs or `--view` are given.
//!
//! Files are pretty JSON written atomically (hidden temp file, then rename).
//! Only the standalone app uses this module; an embedding app keeps
//! [`View`]s in its own config. (Adapted from bobview's `layout.rs`.)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::view::View;

/// The view restored at startup.
pub const DEFAULT_VIEW: &str = "default";

/// `$XDG_CONFIG_HOME/archiveview` when that is set and non-empty, else
/// `$HOME/.config/archiveview`; `None` with neither.
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("archiveview"))
}

/// The colour scheme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the OS (eframe's system theme).
    #[default]
    System,
    Light,
    Dark,
}

/// How many archivers `Settings::recent_archivers` keeps.
pub const MAX_RECENT_ARCHIVERS: usize = 8;

/// `settings.json`. Every field defaults, so old and partial files load.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// The archiver last chosen (`--archiver`, or in the archiver pill):
    /// used when neither `--archiver` nor the view opened names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archiver_url: Option<String>,
    /// Archivers used recently, newest first, for the archiver pill.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_archivers: Vec<String>,
    #[serde(default)]
    pub theme: Theme,
    /// Save the current view as `default` when the window closes, so the
    /// next start reopens where this one left off (on unless turned off).
    #[serde(default = "yes")]
    pub save_default_on_exit: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            archiver_url: None,
            recent_archivers: Vec::new(),
            theme: Theme::default(),
            save_default_on_exit: true,
        }
    }
}

impl Settings {
    /// From `settings.json`, or the defaults when it is missing or bad
    /// (logging a warning for a bad one).
    pub fn load() -> Self {
        config_dir().map_or_else(Self::default, |d| Self::load_from(&d))
    }

    /// Write `settings.json`, logging a warning on failure.
    pub fn save(&self) {
        if let Some(d) = config_dir() {
            self.save_to(&d);
        }
    }

    /// Make `url` (a retrieval base) the archiver to use from now on, and
    /// the first of the recent ones.
    pub fn remember_archiver(&mut self, url: &str) {
        self.archiver_url = Some(url.to_string());
        self.recent_archivers.retain(|u| u != url);
        self.recent_archivers.insert(0, url.to_string());
        self.recent_archivers.truncate(MAX_RECENT_ARCHIVERS);
    }

    fn load_from(dir: &Path) -> Self {
        let path = dir.join("settings.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_else(|e| {
            log::warn!("{}: {e}: using the default settings", path.display());
            Self::default()
        })
    }

    fn save_to(&self, dir: &Path) {
        let written = serde_json::to_string_pretty(self)
            .map_err(std::io::Error::other)
            .and_then(|text| write_atomic(&dir.join("settings.json"), &(text + "\n")));
        if let Err(e) = written {
            log::warn!("cannot save settings in {}: {e}", dir.display());
        }
    }
}

/// A name usable as a view file name: non-empty, no leading dot, no `/` or `\`.
pub fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\'])
}

/// The saved views' names: `default` first (when saved), the rest sorted.
pub fn list_views() -> Vec<String> {
    config_dir().map_or_else(Vec::new, |d| Views(d).list())
}

/// Whether a view called `name` is saved.
pub fn view_exists(name: &str) -> bool {
    config_dir().is_some_and(|d| Views(d).exists(name))
}

/// Load a saved view; refuses a newer `version` than this program's.
pub fn load_view(name: &str) -> Result<View, String> {
    Views(config_dir().ok_or("no home directory")?).load(name)
}

pub fn save_view(name: &str, view: &View) -> Result<(), String> {
    Views(config_dir().ok_or("no home directory")?).save(name, view)
}

pub fn delete_view(name: &str) -> Result<(), String> {
    Views(config_dir().ok_or("no home directory")?).delete(name)
}

/// The views saved under a config directory.
struct Views(PathBuf);

impl Views {
    fn file(&self, name: &str) -> Option<PathBuf> {
        valid_name(name).then(|| self.0.join("views").join(format!("{}.json", name.trim())))
    }

    fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.0.join("views"))
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .filter_map(|e| {
                        let p = e.path();
                        (p.extension()? == "json")
                            .then(|| p.file_stem()?.to_str().map(str::to_string))
                            .flatten()
                    })
                    .filter(|n| !n.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default();
        names.sort_by(|a, b| (a != DEFAULT_VIEW).cmp(&(b != DEFAULT_VIEW)).then(a.cmp(b)));
        names
    }

    fn exists(&self, name: &str) -> bool {
        self.file(name).is_some_and(|f| f.is_file())
    }

    fn load(&self, name: &str) -> Result<View, String> {
        let path = self.file(name).ok_or_else(|| format!("no view {name:?}"))?;
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let view: View =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if view.version > crate::view::VERSION {
            return Err(format!(
                "{}: view version {} is newer than this archiveview's {}",
                path.display(),
                view.version,
                crate::view::VERSION
            ));
        }
        Ok(view)
    }

    fn save(&self, name: &str, view: &View) -> Result<(), String> {
        let path = self
            .file(name)
            .ok_or_else(|| format!("not a view name: {name:?}"))?;
        let write = || -> std::io::Result<()> {
            let text = serde_json::to_string_pretty(view).map_err(std::io::Error::other)?;
            write_atomic(&path, &(text + "\n"))
        };
        write().map_err(|e| format!("{}: {e}", path.display()))?;
        log::info!("saved view {}", path.display());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        let path = self.file(name).ok_or_else(|| format!("no view {name:?}"))?;
        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Write `text` to `path` through a hidden file beside it, so a crash or a
/// full disk leaves the old file whole rather than half written.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    // Hidden and not .json, so never listed as a view.
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, path))
        .inspect_err(|_| {
            std::fs::remove_file(&tmp).ok();
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Range, TraceView};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("archiveview-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn settings_round_trip() {
        let dir = temp("settings");
        assert_eq!(Settings::load_from(&dir), Settings::default());
        let s = Settings {
            archiver_url: Some("http://archiver:17668".into()),
            recent_archivers: vec!["http://archiver:17668".into()],
            theme: Theme::Dark,
            save_default_on_exit: false,
        };
        s.save_to(&dir);
        assert_eq!(Settings::load_from(&dir), s);
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(text.contains("\"theme\": \"dark\""), "{text}");
        // Partial files load; bad ones give the defaults.
        std::fs::write(dir.join("settings.json"), r#"{"theme":"light"}"#).unwrap();
        assert_eq!(Settings::load_from(&dir).theme, Theme::Light);
        assert!(
            Settings::load_from(&dir).save_default_on_exit,
            "on by default"
        );
        std::fs::write(dir.join("settings.json"), "{ bad").unwrap();
        assert_eq!(Settings::load_from(&dir), Settings::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn archivers_are_remembered() {
        let dir = temp("remember");
        let mut s = Settings::load_from(&dir);
        s.remember_archiver("http://a/retrieval");
        s.remember_archiver("http://b/retrieval");
        s.remember_archiver("http://a/retrieval");
        s.save_to(&dir);
        let back = Settings::load_from(&dir);
        assert_eq!(back.archiver_url.as_deref(), Some("http://a/retrieval"));
        assert_eq!(
            back.recent_archivers,
            ["http://a/retrieval", "http://b/retrieval"]
        );
        for i in 0..20 {
            s.remember_archiver(&format!("http://{i}/retrieval"));
        }
        assert_eq!(s.recent_archivers.len(), MAX_RECENT_ARCHIVERS);
        assert_eq!(s.recent_archivers[0], "http://19/retrieval");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn views_round_trip() {
        let dir = temp("views");
        let views = Views(dir.clone());
        assert!(views.list().is_empty());
        let view = View {
            pvs: vec![TraceView {
                name: "MOCK:SINE".into(),
                color: None,
                visible: true,
            }],
            range: Range::Relative { secs: 86_400.0 },
            ..View::default()
        };
        views.save("beam", &view).unwrap();
        views.save(DEFAULT_VIEW, &View::default()).unwrap();
        views.save("alpha", &View::default()).unwrap();
        assert_eq!(views.list(), ["default", "alpha", "beam"]);
        assert!(views.exists("beam") && !views.exists("nope"));
        assert_eq!(views.load("beam").unwrap(), view);
        // With an archiver, and an older file without one.
        let with = View {
            archiver: Some("http://archiver:17668/retrieval".into()),
            ..view.clone()
        };
        views.save("with", &with).unwrap();
        assert_eq!(views.load("with").unwrap(), with);
        std::fs::write(
            dir.join("views/old.json"),
            r#"{"version":1,"pvs":[{"name":"MOCK:SINE"}],"range":{"kind":"relative","secs":86400}}"#,
        )
        .unwrap();
        assert_eq!(views.load("old").unwrap(), view);
        views.delete("with").unwrap();
        views.delete("old").unwrap();
        assert!(views.save("../x", &view).is_err());
        assert!(views.save(".hidden", &view).is_err());
        assert!(views.load("nope").is_err());
        // No temp files left behind.
        let files = std::fs::read_dir(dir.join("views")).unwrap().count();
        assert_eq!(files, 3);

        // A newer version is refused.
        let newer = View {
            version: crate::view::VERSION + 1,
            ..View::default()
        };
        views.save("newer", &newer).unwrap();
        assert!(views.load("newer").unwrap_err().contains("newer"));

        views.delete("beam").unwrap();
        assert_eq!(views.list(), ["default", "alpha", "newer"]);
        assert!(views.delete("beam").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn names_are_checked() {
        assert!(valid_name("beam"));
        assert!(valid_name("beam current"));
        assert!(!valid_name(""));
        assert!(!valid_name("  "));
        assert!(!valid_name(".x"));
        assert!(!valid_name("a/b"));
        assert!(!valid_name("a\\b"));
    }
}
