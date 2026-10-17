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
  "archiver_url": "http://archiver.example.org:17668/retrieval",
  "recent_archivers": [
    "http://archiver.example.org:17668/retrieval",
    "http://127.0.0.1:17668/retrieval"
  ],
  "theme": "system",
  "save_default_on_exit": true
}
```

| field | default | meaning |
| --- | --- | --- |
| `archiver_url` | none | the archiver used when neither `--archiver` nor the view opened at start-up names one. Written by `--archiver` and by a switch in the archiver pill. |
| `recent_archivers` | none | archivers used recently, newest first (at most 8), offered by the archiver pill |
| `theme` | `"system"` | `"system"` (follow the OS), `"light"` or `"dark"`; set from the ≡ menu |
| `save_default_on_exit` | `true` | save the plot as the `default` view on quit, so the next start (with no PVs or `--view`) reopens where you left off |

A missing or unreadable `settings.json` gives the defaults (an unreadable
one logs a warning).

## views/NAME.json

```json
{
  "version": 1,
  "pvs": [
    { "name": "SR-DI-DCCT-01:SIGNAL" },
    { "name": "pva://bl01t:SUM", "color": "#d62728", "visible": false }
  ],
  "range": { "kind": "relative", "secs": 28800 },
  "y": { "autoscale": true },
  "archiver": "http://archiver.example.org:17668/retrieval"
}
```

| field | meaning |
| --- | --- |
| `version` | the format version, `1`. A file with a newer version than the program knows is refused. |
| `pvs` | the traces, in chip order |
| `pvs[].name` | the PV as typed: `NAME`, `ca://NAME`, `pva://NAME`, `sim://sine`, ... |
| `pvs[].color` | `#rrggbb`; when absent, the palette colour for its position |
| `pvs[].visible` | `false` for a hidden trace; default `true` |
| `range` | `{"kind": "relative", "secs": S}`: the last `S` seconds, following now; or `{"kind": "absolute", "start": "...", "end": "..."}` with RFC 3339 UTC times such as `"2026-10-17T09:00:00Z"` |
| `y.autoscale` | default `true` |
| `y.min`, `y.max` | the value range when `autoscale` is `false` |
| `archiver` | the retrieval URL in use when the view was saved; opening the view switches to it. Optional: older files lack it, and the format version is still `1`. |
| `y.log` | `true` for a log value axis; default `false` |

View names can't be empty, start with `.`, or contain `/` or `\`. The view
called `default` opens at startup when no PVs and no `--view` are given.

A host app that embeds the plot gets the same structure from
`Plot::view()` and can store it in its own files; see
[Embed the plot](../how-to/embed.md).
