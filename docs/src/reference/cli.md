# Command line

```
archiveview [OPTIONS] [PV ...]
archiveview-mockarchiver [--port PORT] [--bind ADDR]
```

`archiveview --help` prints the same list.

## archiveview

| argument | meaning |
| --- | --- |
| `PV ...` | PVs to plot. A plain name is Channel Access; `pva://name` is PV Access. With PVs given, archiveview starts a new view of them rather than opening the default view. |
| `-a`, `--archiver URL` | the Archiver Appliance retrieval URL, e.g. `http://host:17668`. Phoebus's `pbraw://host:17668/retrieval` works too, as plain HTTP. `/retrieval` is added unless the URL ends with it. Overrides the opened view's archiver and `archiver_url` in `settings.json`, and is remembered there for later runs (not by `--snapshot`). With no archiver at all the plot is live only. |
| `--no-archiver` | use no archiver, even if the view or `settings.json` names one: live values only. |
| `--view NAME` | open the saved view `NAME` from `~/.config/archiveview/views/`. Giving PVs as well is a usage error. |
| `-r`, `--range SPAN` | the time span to show, ending now: `1h`, `8h`, `7d`, `90m`, `2 days`, `1 hour 30 min`, `3w`, `6mo`, `1y`; units `s`, `m` (minutes), `h`, `d`, `w`, `mo` (30 days), `y` (365 days). Default `1h`. |
| `--theme system\|light\|dark` | the colour theme. Default: `theme` in `settings.json`, else `system` (follow the OS). |
| `--ui-scale F` | start with the whole interface zoomed by `F`, e.g. `1.25` for 125%. Default: `ARCHIVEVIEW_UI_SCALE`, else the zoom last set in the ≡ menu (`ui_zoom` in `settings.json`), else `1`. Snapshots start at `1` without it. See [Zoom the interface](../how-to/navigate.md#zoom-the-interface). |
| `--version`, `--help` | |

### Snapshot mode

| argument | meaning |
| --- | --- |
| `--snapshot OUT.png` | render to a PNG without opening a window, then exit |
| `--size WxH` | image size in pixels. Default `1280x800`. |
| `--scale-factor F` | pixels per point, as on a high-DPI screen. Default `1`. |
| `--wait SECONDS` | render at least this long before writing, and until archive requests finish (at most 60 s). Default `2`. |
| `--hover X,Y` | rest the pointer at this pixel, to show the crosshair readout |
| `--search TEXT` | type `TEXT` into the search box, to show its matches |

Snapshots use the light theme unless `--theme dark` is given. See
[Take snapshots](../how-to/snapshots.md).

### Startup

1. PVs on the command line start a new view of those PVs, over `--range`.
2. Otherwise `--view NAME` opens that view.
3. Otherwise the saved `default` view opens, if there is one.
4. Otherwise the plot starts empty, ready for a PV name.

### Exit status

`0` on success, `2` for a usage error (an unknown option or a bad value),
`1` for any other failure, such as a snapshot that can't be written. A
`--view` that can't be read, or a bad archiver URL, is a usage error. Messages are printed as `archiveview: message`.

## archiveview-mockarchiver

| argument | meaning |
| --- | --- |
| `--port PORT` | the port to serve on. Default `17668`. |
| `--bind ADDR` | the address to listen on. Default `127.0.0.1`; use `0.0.0.0` to serve other machines. |

It runs until killed. See [Mock archiver](mockarchiver.md).
