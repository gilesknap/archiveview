# Environment variables

| variable | used by | meaning |
| --- | --- | --- |
| `ARCHIVEVIEW_LOG` | archiveview | log level on stderr: `error`, `warn` (default), `info`, `debug` or `trace`. Below `debug`, the egui, eframe, winit and EPICS libraries log warnings only. |
| `ARCHIVEVIEW_UI_SCALE` | archiveview | as `--ui-scale` |
| `XDG_CONFIG_HOME` | archiveview | the config folder is `$XDG_CONFIG_HOME/archiveview` when set, else `~/.config/archiveview` |
| `EPICS_CA_ADDR_LIST`, `EPICS_CA_AUTO_ADDR_LIST`, `EPICS_CA_SERVER_PORT`, ... | live PVs | the standard Channel Access client settings |
| `EPICS_PVA_ADDR_LIST`, `EPICS_PVA_AUTO_ADDR_LIST`, ... | live PVs | the standard PV Access client settings, for `pva://` PVs |
| `LIBGL_ALWAYS_SOFTWARE` and friends | archiveview-ssh | set by the script; see [Run over ssh](../how-to/software-rendering.md) |

The archiver URL has no environment variable; use `--archiver` or
`settings.json`.
