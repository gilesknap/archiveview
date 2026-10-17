# archiveview design

archiveview plots archived and live EPICS PV values over time. It covers the
part of the Phoebus Data Browser people actually use, with a modern charting
UI in bobview's style. History comes from an
EPICS Archiver Appliance and live values come from Channel Access or PV
Access through bobview's `pv` crate.

The UI is explicitly **not** the Data Browser. There are no property sheets,
no trace tables, no axis dialogs, no menus of plot options and no "archive
data source" panels. The plot fills the window. A few controls float over it
or sit tight against it, and you work by direct manipulation, in the spirit
of Grafana, TradingView and Apple Stocks.

## Crates

| Crate (package) | Lib name | Kind | Purpose |
|---|---|---|---|
| `crates/archiver` (`archiveview-archiver`) | `archiver` | lib | The retrieval API client. It builds URLs, decodes JSON tolerantly (NaN tokens, string PREC, `[ ]`, optimized bins) and does blocking HTTP with ureq on native targets. It has no egui and no pv dependency. |
| `crates/mockarchiver` (`archiveview-mockarchiver`) | `mockarchiver` | lib + bin | A synthetic Archiver Appliance used by tests, CI, docs and demos. It is deliberately independent of `archiver`, so it tests the client against the wire format and not against shared code. |
| `crates/archiveview` (`archiveview`) | `archiveview` | lib + bin | The embeddable `Plot` widget, the standalone `ArchiveApp`, config, snapshot mode, and a thin `main.rs`. |

External dependencies:

- `pv = { package = "bobview-pv", version = "1.8" }` from crates.io. There are no path dependencies on bobview.
- egui and eframe are pinned at **0.36.2**, identical to bobview, so bobview can embed the plot.
- Other dependencies: chrono, serde/serde_json, parking_lot, web-time, png, clap (native only), ureq (native only, in `archiver`) and tiny_http (mock only).

The code stays wasm-friendly where that is cheap:
- time comes from `web_time`;
- HTTP sits behind `cfg(not(wasm32))`;
- the `archiver` URL builders and `parse_*` functions are pure.

There is no wasm build in v1.

## Embedding: `archiveview::Plot`

bobview will later replace its trend window with this widget. In bobview,
`pvmenu.rs` "Trend" and "Add to trend" send `Request::Trend` / `TrendAdd`,
which open an `egui::Window` in `app.rs`. That window would then hold an
`archiveview::Plot`. The widget's API:

```rust
impl Plot {
    pub fn new(pvs: &pv::PvManager, archiver: Option<ArchiverClient>) -> Self;
    pub fn set_archiver(&mut self, archiver: Option<ArchiverClient>);
    pub fn archiver(&self) -> Option<&ArchiverClient>;
    pub fn add_pvs(&mut self, names: &[String]);
    pub fn remove_pv(&mut self, name: &str);
    pub fn names(&self) -> Vec<String>;
    pub fn title(&self) -> String;
    pub fn view(&self) -> View;            // serde, for the host's own layouts
    pub fn set_view(&mut self, view: &View);
    pub fn is_loading(&self) -> bool;
    pub fn ui(&mut self, ui: &mut egui::Ui);   // fills the space ui offers
    pub fn set_range(&mut self, range: &view::Range);
    // As ui, with the host's items (views, settings) added to the ≡ menu:
    pub fn ui_with_menu(&mut self, ui: &mut egui::Ui, menu: &mut dyn FnMut(&mut egui::Ui));
}
```

These rules make the widget safe to embed:

- **No `eframe::App`, no globals, no singletons.** All state lives in `Plot`, and any number of plots can exist side by side. Every egui `Id` is derived from a per-instance id salt, never from a fixed string: the host's (`Plot::with_id_salt`) or, with `Plot::new`, an atomic counter (the one piece of process-wide state).
- **The library alone.** The standalone app, snapshot mode and their eframe, png and clap dependencies are behind the default `app` feature. A host depends on `archiveview` with `default-features = false` and gets `Plot` on egui only. CI checks that build.
- **The same `pv` crate.** `Plot::new` takes a `bobview_pv::PvManager` (crates.io `bobview-pv`, imported as `pv`). Cargo tells crates apart by package name, so bobview's in-tree crate must be the package `bobview-pv` too (imported as `pv`), with `[patch.crates-io] bobview-pv = { path = "crates/pv" }` in bobview's workspace, or bobview must depend on the published release. A path dependency on a package called `pv` gives a different `PvManager` type that `Plot::new` will not take. This is a change on bobview's side, not done yet.
- **It reuses the caller's pv context.** `Plot::new` clones the caller's `PvManager`. A `tagged` handle from bobview repaints only the window that holds this plot. Archive fetches run as `pvs.runtime().spawn_blocking(..)` jobs on the pv crate's tokio runtime, so the widget starts no runtime and no threads of its own.
- **Its repaints are self-contained.** Each frame the widget stores `ui.ctx().clone()` and `ctx.viewport_id()`. Completed fetches call `ctx.request_repaint_of(viewport)`: outside a frame a plain `request_repaint` targets the root viewport, and bobview shows each trend in a viewport of its own. While following "now", it asks for `request_repaint_after` at the scroll pace (bobview's `scroll_repaint`). The Live pill pulses (at about 15 fps) only for 3 s after following starts and while the pointer is over the plot, so an idle live plot repaints only at the scroll pace.
- **A minimum size.** The plot takes all the space it is offered, but at least 360 × 200 points, so it neither collapses nor grows without end in an auto-sized `egui::Window`. Hosts should give such windows a `default_size`.
- **It works with no archiver.** With `archiver = None` the plot is live only, like bobview's current trend. Search then completes only the names already plotted, and the time presets still apply, limited to the live buffer.
- **It takes the host's theme and fonts.** It reads `ui.visuals()` (`dark_mode`, `extreme_bg_color`, text colour) and never installs fonts or sets the theme. Doing that is `ArchiveApp`'s job.

`main.rs` and `ArchiveApp` add only the window chrome, the CLI, the view
menu and config persistence around that widget.

## Data flow

```
 user input ──► Plot (time range, y range, traces)
                 │  range changed? ──debounce 300 ms──► Fetcher
                 │                                       │ per trace: one job at a time
                 │                                       ▼
                 │            pvs.runtime().spawn_blocking(ArchiverClient::get_data)
                 │                                       │
                 │  ◄── Arc<Mutex<FetchSlot>> result ◄───┘ + ctx.request_repaint()
                 │
 pv::Pv ──Listener (backend thread)──► Arc<Mutex<Samples>> live ring (IOC timestamps)
                 │
                 ▼
 per frame: merged = history (cut at first live t) ⧺ live → decimate to pixel columns → paint
```

### Archive fetch

The plot asks for the visible range padded by 25% of the span on each side,
so a small pan or zoom shows data immediately. The operator depends on what
the trace is known to be:

- `optimized_N` with N = one bin per device pixel of the padded window (1.5 × the plot width), capped at 4000. This is used for scalar numeric PVs. Finer bins only alias: their means zig-zag into a solid block, where the min/max envelope already shows the extremes.
- `Operator::Raw` for waveforms, strings and enums once known, that is when `PvInfo::binnable()` is false (enums are numeric, but bin means of state indices name no state). `metadata()` is fetched once per trace when it is added. If that fails, the first data response decides: an array `val` of length other than 5 means a waveform.
- Raw-only traces fetch at most the last day (`RAW_MAX_SPAN`) of a wider window, padded: a week of a busy waveform could be gigabytes. The status line says so.

Refetch rules:
- Changing the range (pan, zoom, preset, live scrolling past the padded window) triggers a refetch after a **300 ms** debounce.
- Each trace has at most one job in flight. While it runs no other starts, however the window moves; when it finishes, the next frame asks again for wherever the window is then. A new archiver (`set_archiver`) makes older results stale, using a generation counter, and their results are dropped. `set_archiver` keeps counting (it does not reset the counter), so answers from the previous archiver are stale too.
- Searches can finish out of order; the inbox keeps only the newest answer.
- A request that panics still counts down the busy count (a drop guard) and reports an error, so snapshots and live refreshes never wait on it (with the release profile's `panic = "abort"` a panic ends the process anyway).
- Until the new data arrives, the previous history stays on screen, so there is no flicker.

Errors:
- A 404 means "not archived" only if the archiver answers a PV search (`getMatchingPVs`) for the name. The trace then still plots live data, and its legend chip shows a subtle "live only" mark. If the search fails too, the `--archiver` URL is wrong (Tomcat gives 404 for every URL under a wrong port or context path), and that is reported as an error.
- Any other error shows on the chip as a tooltip and as one line in the status hint. It is retried after 10 s, doubling with each failure in a row up to 5 minutes (or after 10 s for a different window).

Live-following:
- While following now, the plot refreshes the history at most every 30 s, so it fills in behind the live data. Raw history fetches only the tail (`from` = its last sample's time) and appends it, dropping what has scrolled out of the padded window. Binned history is fetched whole again: it is at most `MAX_BINS` points, and bins of a shorter tail would not line up with the others.

### Live merge

- Each trace subscribes with `pvs.subscribe(name)` and keeps a `Listener` that pushes `(t, v)` into an `Arc<Mutex<Samples>>`. This is bobview's trend pattern.
- Timestamps:
  - `t` is the **IOC timestamp** (`PvState::timestamp`), so live samples line up with the archive.
  - It falls back to arrival time when the timestamp is absent or earlier than 1990.
- Disconnects push NaN, which becomes a gap.
- Like Phoebus, the plot drops the first state if it is disconnected or has no value.
- The ring holds 20 000 samples, or 24 h, whichever comes first.
- **Merge:** history is truncated at the first live sample's time, so live wins at the right edge. Hold-last-value step drawing for both means the join is seamless.
- Lock rule from bobview: the listener runs while the PV's state lock is held, so never call `pv.state()` while holding the samples mutex.

### Rendering

Traces:
- Raw samples are drawn as steps: the value holds until the next sample, matching archiver semantics.
- The archive's first sample may be before `from`, and is drawn held from the left edge.
- Binned samples are drawn as a mean line with a translucent min..max **envelope** (fill alpha about 0.18, in the trace colour).
- Gaps (NaN, or severity INVALID with `cnxlostepsecs`) break the line.

Decimation:
- Points are decimated per pixel column into first, min, max and last (bobview `plot::decimate`, plus the `Thinned` cache for the live ring).
- Zero-length segments are deduplicated, because they would produce NaN triangles (bobview `plot::line`).
- Lines are anti-aliased with 1.5 pt stroke width, and 2.5 pt for the hovered trace.

Value axes:
- One per distinct unit among the visible traces, in legend order, up to three; traces with yet more units share the third. The first is left of the data; the others stack outward on the right, their labels and title (the unit) in the trace's colour when one trace has the axis. With one unit there is one axis, as before.
- The first axis takes the fixed range, the log scale and value zooming (box zoom, axis drag, shift-scroll). The others always autoscale to their own traces (log too when the scale is log).
- Autoscaling covers the visible min/max including envelopes, padded with `axis::nice_range`.
- Turning the log scale on with a fixed range at or below zero goes back to autoscaling.
- Range changes animate over about 150 ms, using `ctx.animate_value_with_time` on lo and hi.

### Time model

- The range is `Range::Relative { secs }`, which is live and ends at now, or `Range::Absolute { start, end }`.
- Panning or zooming into the past leaves live mode.
- The **Live** pill, the `L` key, or panning back to the right edge (within 2% of the span) re-enters live mode.
- Presets are 1h, 8h, 1d and 7d, plus Custom, which takes typed spans parsed by bobview's `parse_span` ("90m", "2 days", ...) or two date-times.

## UI

### Layout sketch (light theme; dark follows the OS)

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ 🔍 Search PVs…            │ 1h  8h  1d  7d  ⋯ │  ● Live  │  ⤢  ⌂  │     ≡   │  ← slim top bar (one row, ~36 pt)
├──────────────────────────────────────────────────────────────────────────────┤
│ 300 ┤                                                     ┆ ╭─────────────╮ │
│     │        ╱‾‾‾╲            ▒▒▒                          ┆ │ Tue 17 14:32:05│ │
│ 250 ┤ ──────╯     ╲_______▒▒▒▒▒▒▒╲______________________●  ┆ │● SR-DI…  298.1 mA│ │  ← crosshair + floating readout
│     │                                                     ┆ │● bl01t:SUM 4.20 │ │
│ 200 ┤ - - - - - - - - - - - - - - - - - - - - - - - - - - ┆ ╰─────────────╯ │
│     └────────┬──────────┬──────────┬──────────┬─────────┬┆─────────────│now  │
│            12:00      13:00      14:00      15:00     16:00                   │
│ ( ● SR-DI-DCCT-01:SIGNAL  298.1 mA ✕ ) ( ● bl01t:SUM  4.20 ✕ ) ( ○ MOCK:SINE ✕ )│  ← legend chips hug the bottom
└──────────────────────────────────────────────────────────────────────────────┘
```

Top bar (`egui::Panel::top` inside `Plot::ui`, styled like bobview's toolbar):

- **Search box.** A rounded `TextEdit` with a search icon, filling the left of the bar. Typing debounces for 200 ms, then calls `archiver.search(Search::from_user(text, 50))`. The results show in a popup list under the box (`egui::Popup`).
  - Arrow keys move through the list and Enter adds the selected result.
  - Enter on free text adds the text as typed. That is how you add a non-archived PV or a `pva://` or `sim://` PV.
  - A small `.*` toggle inside the box switches to regex.
- **Time-range pill.** A segmented control: 1h, 8h, 1d, 7d and `⋯`. The `⋯` popover holds a span field and two date-time fields. The active segment is filled with the accent colour. When the range is custom, or has been zoomed, no segment is lit and the pill shows the span as text, e.g. "3h 20m".
- **Live toggle.** A pill reading `● Live`. While following now its red dot shows; it pulses (a halo animated with `ctx.input(|i| i.time)`, repainting at about 15 fps) for 3 s after following starts and while the pointer is over the plot, and is still otherwise. When not following, it reads `○ Live` and clicking it jumps to now.
- **Autoscale** (`⤢`) and **Reset** (`⌂`, bobview `Icon::Home`) icon buttons.
- **`≡` menu.** This belongs to the app only, not the widget, and is added by `ArchiveApp` to the right. It holds Log scale (the widget's own item), then Views (Save NAME, a Save as… field, Save as default, Open ▸, Delete ▸, "Save default on exit", on by default), Theme (System/Light/Dark) and a summary of the gestures. That is the whole menu.
- **Narrow windows** (under about 600 pt). The pill collapses to a combo, and autoscale and reset move into the menu.

Plot area (fills the rest):

- Background `extreme_bg_color` with subtle gridlines (about 6% contrast), no heavy frame, and axis labels in `weak_text_color`.
- **Adaptive time labels.** The step comes from bobview's `TIME_STEPS`, extended with 2d, 7d, 14d and 30d. Labels read "14:30" within a day, "Tue 17" at midnights or for spans of days, and "Oct" at month starts. The first tick carries its context, e.g. "Tue 17 14:00". Ticks step through local wall-clock time (midnights by calendar date, sub-day steps by local time of day, each with its own UTC offset), so day lines stay at midnight across a daylight-saving change.
- **Autoscale** pads the data by 4% (not below a minimum of exactly 0, nor above a maximum of 0) and rounds out to half a tick step, so little height is wasted.
- A **now marker**: a thin dashed vertical line with a small "now" tag at the top when now is in view, in neutral grey (red is a trace colour and an alarm colour; the Live dot alone is red).
- The **crosshair** follows the pointer. It is a vertical hairline plus a floating rounded tooltip, with a shadow, which flips to the left near the right edge.
  - The tooltip shows the timestamp, then one row per visible trace: a colour dot, the name, and the value with EGU (formatted with the PV's precision), coloured by severity when in alarm.
  - The hairline snaps to a sample of the first visible trace within 8 points, and a small time tag sits on the time axis under it.
  - Each value is the one the line shows at the hairline: for raw samples (drawn as steps) the last sample at or before it, "—" in a gap; for bins the nearest bin mean. Each trace gets a small dot where its line is.
  - For binned data the row also shows "min … max".
- The **empty state** shows a centred, muted "Type a PV name to start" with a hint line ("/ to search · drag to pan · scroll to zoom"), plus an arrow glyph pointing at the search box.
- **No data** in range (`[ ]`): when the chip has no value at all (no live value and an empty history), it shows a quiet "no data" after the "—", and the crosshair card has a "—" row for it.

Legend chips (a wrapped row hugging the bottom of the plot):

- Each chip is a pill with a colour dot, the PV name (without `ca://` or `<type>`), the latest value and EGU in tabular figures, and an ✕ that appears on hover.
- Click toggles visibility: a hidden trace's chip is greyed with a hollow dot.
- Hovering a chip highlights its trace (thicker line) and dims the others to 25% alpha.
- A disconnected trace's value shows a grey "—" and the chip's tooltip says why.
- While a trace is fetching, a small spinner replaces the dot.

### Direct manipulation

| Gesture | Effect |
|---|---|
| drag (primary) in plot | pan time; leaves live. A kinetic glide follows release (velocity decays about 5×/s) |
| scroll wheel / trackpad pinch (`zoom_delta`) | zoom time about the pointer |
| horizontal trackpad scroll | pan time |
| shift- or ctrl-scroll, or drag on the value axis | zoom or pan values; turns autoscale off |
| secondary-drag or shift-drag a box | zoom to the box (time and value) |
| double-click | reset: autoscale on, back to the preset span (live if it was live) |
| hover | crosshair readout |

Transitions animate over about 150 ms with ease-out.

Keyboard (only when no text field has focus):
- `/` focuses search and `Esc` closes popups or clears search.
- `←`/`→` pan by 10% of the span, and `+`/`-` (or `=`) zoom time ×0.8 or ×1.25 about the centre.
- `L` toggles live, `A` autoscales and `Home` resets.

### Palette and theme

- `ArchiveApp` applies `Settings::theme`. The default, System, follows the OS through eframe. Snapshots use light unless `--theme dark` is given.
- Fonts are bobview's embedded Liberation Sans set, installed by `ArchiveApp` only.
- **Trace colours** are a 10-colour palette tuned for both themes. The default is a Tableau-like set close to bobview's `COLORS` (blue, red, green, orange, purple, teal, brown, pink, grey, olive). In dark mode each colour is moved 22% of the way to white, so every colour keeps at least 3:1 contrast against `extreme_bg_color`.
- The accent (the active pill, the focused search ring) is egui's `selection.bg_fill`.
- Alarm colours follow Phoebus: MINOR (255,128,0), MAJOR (255,0,0), INVALID (255,0,255).

## Persistence (`~/.config/archiveview`)

Persistence follows bobview's `layout.rs`:
- `config_dir()` is `$XDG_CONFIG_HOME/archiveview`, falling back to `~/.config/archiveview`.
- Files are pretty JSON, written atomically.
- A `version` newer than the program's is refused.

`settings.json`:

```json
{ "archiver_url": "http://archiver:17668", "theme": "system", "save_default_on_exit": true }
```

`views/<name>.json`:

```json
{
  "version": 1,
  "pvs": [ { "name": "SR-DI-DCCT-01:SIGNAL" }, { "name": "bl01t:SUM", "color": "#d62728", "visible": false } ],
  "range": { "kind": "relative", "secs": 3600 },
  "y": { "autoscale": true }
}
```

`range` can also be `{ "kind": "absolute", "start": "2026-10-17T09:00:00Z", "end": "2026-10-17T12:00:00Z" }`.

Startup order:
1. PVs on the command line give a new view with those PVs, using `--range`.
2. Otherwise `--view NAME` opens that view.
3. Otherwise `views/default.json` opens, if it exists.
4. Otherwise the plot starts in the empty state.

## Command line

```
archiveview [--archiver URL] [--view NAME] [--range SPAN] [--theme system|light|dark]
            [--ui-scale F] [--snapshot OUT.png [--size WxH] [--scale-factor F] [--wait SECONDS]]
            [PV ...]
```

- `--archiver`: the retrieval base, e.g. `http://host:17668`. `/retrieval` is appended unless the URL already ends with it. It overrides `settings.json`, and the plot is live only with neither.
- `--range`: e.g. `1h`, `7d` or `90m`, parsed by `parse_span`.
- `--snapshot`: run headless and write a PNG. The run takes at least `--wait` seconds, then waits until `!is_loading()`, with a 60 s deadline. The size defaults to 1280x800 and the wait to 2 s. This is bobview's software rasteriser.
- Logging uses the `ARCHIVEVIEW_LOG` level, with library targets capped at warn as in bobview.
- Exit codes are 2 for usage errors and 1 for failures.
- `archiveview-mockarchiver [--port 17668] [--bind 127.0.0.1]` starts the mock appliance.

## Archiver Appliance protocol (what `archiver` implements)

The client normalises the URL to `R = <url>/retrieval`, then uses these endpoints.

**Data:** `R/data/getData.json?pv=<op(PV)>&from=<ts>&to=<ts>`
- Timestamps are sent as `YYYY-MM-DDTHH:MM:SS.mmmZ` in UTC, and every parameter is percent-encoded.
- The response is an array with one `{meta, data}` object per PV.
- `[ ]` means no data: `get_data` returns `Ok(None)`.
- Bare `NaN`, `Infinity` and `-Infinity` tokens are replaced with `null` outside strings (`sanitize_json`) before `serde_json` parses the body. A null value means NaN, which is a gap.
- Meta values may be strings or numbers.
- The first sample may be before `from`.
- With `optimized_N`, a scalar PV's `val` may be `[mean, std, min, max, count]`. That gives `binned = true`, with `t` at the bin centre.
- Severity 3 with `fields.cnxlostepsecs` gives a gap sample.
- Any non-2xx response becomes `Error::Http{status, first line of body with tags stripped}`. A 404 means the PV is not archived.
- A sample after a reconnect whose `fields.cnxlostepsecs` is later than the previous sample also gets a gap inserted at the lost time (the engine's own form).
- `get_data` reads 5-element arrays as bins only for `Operator::Optimized` requests; the public `parse_data` uses a heuristic (every array has five elements with a whole count, and meta does not say `waveform: true`).

**Search:** `R/bpl/getMatchingPVs?pv=<glob>&limit=N` (or `regex=`)
- The glob is converted naively: `*` becomes `.*`, `?` becomes `.`, and the whole name must match.
- The search box wraps text that has no wildcards as `*text*`.

**Metadata:** `R/bpl/getMetadata?pv=PV`
- Every value comes back as a string: `DBRType`, `elementCount`, `units`, `precision`, and so on.

The mock also serves `/mgmt/bpl/getAllPVs`, which the client never calls.

## Mock archiver

The mock serves every endpoint on one port, using `tiny_http` on a small
thread pool.

Its data:
- Data is deterministic: f(t) on each PV's sample grid. "Noise" is a hash of the sample index, so the mock keeps no RNG state.
- History starts 30 days before the server starts.

The PVs:

| PV | Signal |
|---|---|
| `MOCK:SINE` | 1 Hz, 10-min period, `mA`, PREC 3 |
| `MOCK:RAMP` | 0.1 Hz sawtooth |
| `MOCK:NOISE` | 10 Hz |
| `MOCK:SLOW` | every 5 min, so optimized takes the raw path |
| `MOCK:STEP` | 1 Hz grid, holds for 4 min then jumps (step drawing) |
| `MOCK:ENUM` | 0..3 |
| `MOCK:STRING` | string |
| `MOCK:WAVE` | 8-element array |
| `MOCK:NAN` | NaN every 50th sample, plus severity-3 `cnxlostepsecs` gaps |
| `MOCK:BIG` | 1 Hz sine of amplitude 3e10 (Java `3.0E10` notation on the wire) |
| `bl01t:A`, `bl01t:B`, `bl01t:SUM`, `BL01T-MO-SIMC-01:M1.RBV`, `BL01T-MO-SIMC-01:M2.RBV` | the names bobview-testioc serves live, so archive and live join up in demos and CI |

Operators:
- raw, including the last sample before `from`;
- `optimized_N`, using the exact `Optimized.java` rules;
- `mean_N`, `nth_N` and `ncount`;
- the `<op>Sample_N` no-fill variants.

Errors and encoding:
- 404 with an HTML body for unknown PVs.
- 400 for a bad timestamp or `to < from`.
- The JSON is written by hand, so `NaN` appears as a bare token, as it does from the real appliance.
- `MOCK:SINE` raises MINOR (status HIGH) near its top, for severity colouring.
- Like the appliance, filled bins (`optimized_N`, `mean_N`, ...) run to the bin holding `to`, even past now, repeating the last value. The plot should not draw history bins beyond the newest raw sample time (or now).
- Summaries of strings and waveforms come back raw (a mock simplification).

## Testing

- **`archiver`** has unit tests for URL normalisation, `sanitize_json`, `parse_data` (fixtures taken from the spec, covering NaN tokens, string PREC, `[ ]`, bins, waveforms, strings and gaps), `Search::from_user` and `format_time`. Its integration tests run against `MockArchiver::start_local()`.
- **`mockarchiver`** has unit tests for optimized binning (bin centres, fill, raw below N), and for search globs and status codes.
- **`archiveview`** tests:
  - a snapshot test that renders against the mock archiver;
  - unit tests for time ticks and labels, merge and truncation, decimation, range/interaction maths (zoom about a point, pan), and config round-trips with `XDG_CONFIG_HOME` pointed at a temp dir.
- **CI** runs fmt, clippy (`-D warnings`) and tests. It then takes snapshots by starting `archiveview-mockarchiver --port 17668 &`, then `archiveview --archiver http://127.0.0.1:17668 --snapshot snapshots/x.png --size 1280x800 --wait 5 MOCK:SINE MOCK:NOISE MOCK:NAN`, plus an empty-state snapshot and a dark-theme snapshot. The snapshots are uploaded as an artifact.
- **Review snapshots against the UI bar above**, not just "it rendered". Check that the plot fills the window, the chrome is minimal, there are envelopes for binned data, the time labels are readable, the chips look right, and the empty state is there.

## Where things live

- `crates/archiver`: the Archiver Appliance client (`ArchiverClient`, queries, JSON decoding, time).
- `crates/mockarchiver`: the mock appliance (`MockArchiver`, the synthetic `MOCK:*` and testioc PVs) and its binary.
- `crates/archiveview/src/plot.rs` and `plot/`: the embeddable `Plot` widget (series, fetch, window, timeaxis, render, bar, legend, hover).
- `crates/archiveview/src/app.rs`, `main.rs`, `config.rs`, `view.rs`, `snapshot.rs`: the standalone app, its CLI, settings, saved views and snapshot mode.
- `.github/`, `docs/`, `scripts/`, `install.sh`: CI, the mdBook documentation, packaging and installation.
