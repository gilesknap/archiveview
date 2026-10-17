# Scope, and how it differs from the Data Browser

## What archiveview is for

Looking at how PVs have behaved: what the beam current did overnight, when a
vacuum gauge started to climb, whether a motor's readback settled. You name
some PVs, choose a time range, and read the plot. For that it needs:

- history from an EPICS Archiver Appliance, and live values from Channel
  Access or PV Access, as one line per PV;
- several PVs on one shared time axis;
- fast pan and zoom from a week down to seconds;
- reading exact values and times off the plot;
- saving a set of PVs and a range to come back to.

## What it leaves out

The Phoebus Data Browser does a great deal more, and exposes it through
property sheets, trace tables, axis configuration dialogs, archive data
source lists and long context menus. archiveview has none of these. In v1 it
also leaves out:

- axis dialogs: value axes are chosen automatically, and moved from a
  chip's dot or by dragging the chip;
- exporting data (CSV, spreadsheet);
- statistics, formulas and waveform views;
- `.plt` files;
- more than one archiver at a time.

Some of these may come later, but only where they can be done without
bringing back the configuration panels.

## How it feels instead

archiveview takes its cues from modern charting tools such as Grafana,
TradingView and Apple Stocks:

- **The plot is the interface.** It fills the window; a slim bar holds
  search, the time-range pill, the live toggle and two buttons.
- **Direct manipulation** replaces settings. Drag to pan, scroll or pinch to
  zoom, drag a box, drag the value axis; double-click to go back.
- **Read by hovering.** A crosshair card shows every trace's value, units and
  severity at the pointer's time.
- **Traces are chips**, not table rows: click to hide, hover to highlight, ✕
  to remove.
- **Sensible defaults** stand in for options: autoscale over what is
  visible, colours from a palette that works on light and dark backgrounds,
  min/max bands wherever the archiver has thinned the data, and time labels
  that adapt from `14:30` to `Tue 17` to `Oct`.

## Relationship to bobview

archiveview shares bobview's look, fonts, palette and PV library, and its
plot widget is meant to replace bobview's own trend window, so a trend
opened from a bobview display gains history from the archiver.
