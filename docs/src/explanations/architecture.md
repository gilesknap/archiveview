# Architecture

## Crates

| crate | what |
| --- | --- |
| `archiver` (package `archiveview-archiver`) | the Archiver Appliance retrieval client: builds URLs, makes blocking HTTP requests, and decodes the JSON tolerantly. It knows nothing of egui or live PVs. |
| `archiveview` | the `Plot` widget, the standalone app around it (window, menu, saved views, settings), snapshot mode and the command line |
| `mockarchiver` (package `archiveview-mockarchiver`) | a synthetic Archiver Appliance for tests, CI and demos |

Live values come from bobview's [`pv`](https://crates.io/crates/bobview-pv)
crate (published as `bobview-pv`), which speaks Channel Access and PV Access
in pure Rust and runs its own small tokio runtime.

The mock deliberately shares no code with the client, so the client's tests
exercise the real wire format, including its oddities, rather than a common
definition of it.

## From the time range to pixels

```
 user input ──► Plot (time range, value range, traces)
                 │  range changed? ──300 ms debounce──► archive fetch per trace
                 │                                       (on the pv runtime, blocking HTTP)
                 │  ◄─────────── result + repaint ◄──────┘
                 │
 pv::Pv ──listener──► live ring buffer per trace (IOC timestamps)
                 │
                 ▼
 each frame: history (cut at the first live sample) + live
             → thinned to pixel columns → drawn
```

### Fetching history

When the time range changes (a preset, a pan, a zoom, or the live view
scrolling on), the plot waits 300 ms for things to settle, then asks the
archiver for each PV's history over the visible range plus a quarter of the
span either side, so small pans show data straight away. Each trace has at
most one request in flight; a newer one makes older answers stale, and they
are dropped when they arrive. The old history stays on screen until the new
one lands, so nothing flickers.

For scalar numeric PVs the request is wrapped in the appliance's
`optimized_N` operator, with N twice the plot's width in device pixels (at
most 4000). The appliance then decides:

- if the range holds no more than N samples, it returns them raw, drawn as
  steps (each value holds until the next sample, which is what an archived
  sample means);
- otherwise it returns N bins, each with the mean, min, max and count,
  drawn as the mean line over a translucent min..max band.

So the amount of data sent is bounded by the plot's width, not the time
range: a week of a 10 Hz PV costs the same as an hour.

Strings and enums are fetched raw (an enum's index). The plot asks the
archiver's `getMetadata` once per PV to learn its type, and falls back to
the shape of the first response.

Array (waveform) PVs are not plotted: a line of one element over time says
little about a waveform, so their chip shows the latest array instead. When
the metadata says a PV is an array, no data is fetched at all; without
metadata, the first response tells, and its samples are dropped.

### Joining live values

Each trace subscribes to its PV through the `pv` crate and records every
update into a ring buffer, stamped with the **IOC's timestamp**, the same
clock the archiver stored, so the two line up. History is cut at the first
live sample, so at the right edge the live values always win, and since both
are drawn as steps the join is seamless. A disconnect records a gap.

While the plot follows now, the history behind the live values is topped up:
raw history every 30 seconds or so, by fetching only what is new since its
last sample; binned history by fetching the (bounded) window again, but only
once time has moved on by a few bins, so a week-long view asks the archiver
for its week of bins every half hour, not every 30 seconds. Each
trace has at most one request to the archiver at a time.

### Drawing

Each frame, the visible part of every trace is thinned to at most four
points per pixel column (first, min, max, last), which keeps every peak
while drawing a bounded number of segments. Gaps (NaN values, and the
archiver's disconnection markers) break the line. Autoscale covers the
visible values including the bands, and the value axes animate to their new
ranges. Traces with different units get a value axis each (up to three), so
a current in mA and a voltage of a few volts are both readable.

## The widget and the app

`archiveview::Plot` holds everything about one plot and nothing global, so
several can share one window or one program, and it borrows the host's PV
manager and runtime. `ArchiveApp` wraps one plot with the window chrome:
fonts, theme, the ≡ menu, and loading and saving views and settings in
`~/.config/archiveview`. `main.rs` adds the command line and either opens a
window or renders a snapshot.

## Snapshots

`--snapshot` runs the same app in a bare egui context with no window, waits
for the archive requests to finish, then draws egui's triangles with a small
software rasteriser into a PNG. It is bobview's snapshot mode.
