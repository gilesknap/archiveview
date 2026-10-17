# Embed the plot in an egui app

The plot is a library widget, `archiveview::Plot`, and the `archiveview`
program is a thin window around it. Any egui 0.36 app can show one, and as
many as it likes; bobview is meant to use it for its trend windows.

```toml
[dependencies]
# default-features = false: just the widget, without the standalone app's
# eframe, png and clap
archiveview = { git = "https://github.com/gilesknap/archiveview", default-features = false }
pv = { package = "bobview-pv", version = "1.8" }
egui = "0.36.2"
```

The host must use the **same `pv` crate** as archiveview, the `bobview-pv`
package: Cargo treats two packages with different names as different
crates, even from the same source, so a `PvManager` from a path dependency
on a package called `pv` cannot be passed to `Plot::new`. bobview itself
therefore names its in-tree crate's package `bobview-pv` (keeping `pv` as
the name it is imported by) and points the published one at it:

```toml
# bobview's workspace Cargo.toml
[patch.crates-io]
bobview-pv = { path = "crates/pv" }
```

```rust
use archiveview::{ArchiverClient, Plot};

// once, in the app's constructor; share the app's own PvManager
let pvs = pv::PvManager::new();
let archiver = ArchiverClient::new("http://archiver:17668").ok();
let mut plot = Plot::new(&pvs, archiver);
plot.add_pvs(&["SR-DI-DCCT-01:SIGNAL".to_string()]);

// each frame, e.g. inside an egui::Window: give it a size, as the plot
// fills whatever space it is offered (at least 360 x 200)
egui::Window::new(plot.title())
    .id(egui::Id::new("trend-1"))
    .default_size([720.0, 420.0])
    .resizable(true)
    .show(ctx, |ui| plot.ui(ui));
```

What the widget does, and doesn't, assume:

- **It uses the host's PV context.** `Plot::new` takes the host's
  `pv::PvManager`, subscribes through it, and runs archive requests on that
  manager's tokio runtime. It starts no runtime or threads of its own.
- **No archiver is fine.** With `None` it plots live values only, like
  bobview's trend window. `set_archiver` changes it later.
- **No globals.** Every plot keeps its own state and egui ids, so several can
  be open at once. `Plot::new` salts its ids with a running count;
  `Plot::with_id_salt(&pvs, archiver, "trend-1")` uses the host's own salt,
  which keeps them stable when the host rebuilds its plots.
- **The host owns the look.** The plot follows `ui.visuals()` (light or dark)
  and the host's fonts; it never sets the theme or installs fonts.
- **It repaints itself** when archive data arrives (in the viewport it was
  last drawn in, so a plot in its own native window is repainted, not the
  root one) and, while following now, at the pace the time axis scrolls.

To save a plot in the host's own layout files, store `plot.view()` (it is
`serde`-serialisable) and restore it with `plot.set_view(&view)`.

The search box remembers recent searches in memory only. To keep them
across runs, as the standalone app does in `history.json`, give the plot
the saved list with `plot.set_search_history(history)` and save
`plot.search_history()` (a `serde`-serialisable `SearchHistory`) whenever
`plot.search_history_changed()` returns true. A host with several plots
can share one list by passing it between them the same way.
`plot.is_loading()` is true while archive requests are in flight.

See the [Rust API](../reference/api.md) for the full interface.
