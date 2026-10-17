# archiveview

[![CI](https://github.com/gilesknap/archiveview/actions/workflows/ci.yml/badge.svg)](https://github.com/gilesknap/archiveview/actions/workflows/ci.yml)
[![Docs](https://github.com/gilesknap/archiveview/actions/workflows/docs.yml/badge.svg)](https://gilesknap.github.io/archiveview/)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

A modern plotter for archived and live EPICS PVs, written in Rust with
[egui](https://github.com/emilk/egui). History comes from an
[EPICS Archiver Appliance](https://github.com/archiver-appliance/epicsarchiverap)
and live values from Channel Access or PV Access, joined into one line per
PV. It does what most people use the Phoebus Data Browser for, without the
dialogs: the plot fills the window, you search for PVs as you type, drag to
pan, scroll to zoom and hover to read values. The plot is also an egui
widget that other apps, such as [bobview](https://github.com/gilesknap/bobview),
can embed.

| | |
| --- | --- |
| Source | <https://github.com/gilesknap/archiveview> |
| Documentation | <https://gilesknap.github.io/archiveview> |
| Releases | <https://github.com/gilesknap/archiveview/releases> |

Install on Linux (x86_64 or aarch64) or macOS, then plot some history from
the bundled mock archiver:

```bash
curl -sSfL https://raw.githubusercontent.com/gilesknap/archiveview/main/install.sh | bash
archiveview-mockarchiver &
archiveview --archiver http://127.0.0.1:17668 MOCK:SINE MOCK:NOISE
```

Linux needs a few graphics libraries first; see
[Install archiveview](https://gilesknap.github.io/archiveview/main/how-to/install.html).

See the [documentation](https://gilesknap.github.io/archiveview) for everything else.
