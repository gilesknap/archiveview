//! archiveview: archived and live EPICS PV time-series plots.
//!
//! The reusable part is [`Plot`], an egui widget that bobview (or any egui
//! app) can embed: it owns its traces, archive fetches and live
//! subscriptions, but no window, no `eframe::App`, no process-global state
//! (bar an id counter, see [`Plot::with_id_salt`]),
//! and it borrows the caller's [`pv::PvManager`] (and through it the tokio
//! runtime its fetches run on). Several `Plot`s can live side by side, and
//! one works with no archiver at all (live only).
//!
//! `app::ArchiveApp` is the standalone application around one `Plot`:
//! window chrome, saved views and settings in `~/.config/archiveview`.
//! `main.rs` adds only the command line. Both, and snapshot mode, are
//! behind the default `app` feature: a host embedding `Plot` turns it off
//! (`default-features = false`) and depends on egui only, not eframe.

#[cfg(feature = "app")]
pub mod app;
mod axis;
pub mod config;
mod icons;
pub mod plot;
#[cfg(all(feature = "app", not(target_arch = "wasm32")))]
pub mod snapshot;
pub mod style;
pub mod view;

pub use archiver::ArchiverClient;
pub use plot::Plot;
pub use view::View;
