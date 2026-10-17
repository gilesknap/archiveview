# Config files and the view format

archiveview keeps its files in `$XDG_CONFIG_HOME/archiveview` when
`XDG_CONFIG_HOME` is set, otherwise `~/.config/archiveview`:

```
~/.config/archiveview/
├── settings.json
└── views/
    ├── default.json
    └── beam.json
```

All are pretty-printed JSON, written atomically (to a hidden temporary file,
then renamed), so a crash never leaves half a file. Unknown fields are
ignored and missing ones take their defaults, so files can be edited by hand.

## settings.json

```json
{
  "archiver_url": "http://archiver.example.org:17668",
  "theme": "system",
  "save_default_on_exit": true
}
```

| field | default | meaning |
| --- | --- | --- |
| `archiver_url` | none | the archiver used when `--archiver` is not given |
| `theme` | `"system"` | `"system"` (follow the OS), `"light"` or `"dark"`; set from the ≡ menu |
| `save_default_on_exit` | `true` | save the plot as the `default` view on quit, so the next start (with no PVs or `--view`) reopens where you left off |

A missing or unreadable `settings.json` gives the defaults (an unreadable
one logs a warning).

## views/NAME.json

```json
{
  "version": 1,
  "pvs": [
    { "name": "SR-DI-DCCT-01:SIGNAL", "axis": 0 },
    { "name": "SR-DI-DCCT-01:LIFETIME", "axis": 1 },
    { "name": "pva://bl01t:SUM", "color": "#d62728", "visible": false, "axis": 1 }
  ],
  "range": { "kind": "relative", "secs": 28800 },
  "y": { "autoscale": true },
  "axes": [ { "autoscale": false, "min": 1, "max": 100, "log": true, "name": "Lifetime" } ]
}
```

| field | meaning |
| --- | --- |
| `version` | the format version, `1`. A file with a newer version than the program knows is refused. |
| `pvs` | the traces, in chip order |
| `pvs[].name` | the PV as typed: `NAME`, `ca://NAME`, `pva://NAME`, `sim://sine`, ... |
| `pvs[].color` | `#rrggbb`; when absent, the palette colour for its position |
| `pvs[].visible` | `false` for a hidden trace; default `true` |
| `pvs[].axis` | its value axis: `0` for `y`, `1` for the first of `axes`, ... When absent (as in files from before there were several axes), the trace picks an axis when its data arrives |
| `range` | `{"kind": "relative", "secs": S}`: the last `S` seconds, following now; or `{"kind": "absolute", "start": "...", "end": "..."}` with RFC 3339 UTC times such as `"2026-10-17T09:00:00Z"` |
| `y` | the first value axis |
| `axes` | the value axes after the first, in order; omitted when there is one |
| `y.autoscale`, `axes[].autoscale` | default `true` |
| `y.min`, `y.max` | the value range when `autoscale` is `false` |
| `y.log` | `true` for a log value axis; default `false` |
| `y.name` | the name typed for the axis; when absent it names itself |

The value-axis fields were added without changing `version`: older files
load (their traces pick axes as they are added), and an older archiveview
reads newer files with every trace on its unit's axis.

View names can't be empty, start with `.`, or contain `/` or `\`. The view
called `default` opens at startup when no PVs and no `--view` are given.

A host app that embeds the plot gets the same structure from
`Plot::view()` and can store it in its own files; see
[Embed the plot](../how-to/embed.md).
