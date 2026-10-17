# Navigate the plot

Everything happens on the plot itself; there are no axis dialogs. The full
list is in [Gestures and keys](../reference/controls.md).

## Choose a time range

The pill in the bar holds the presets **1h**, **8h**, **1d** and **7d**. A
preset shows that span ending now, following the current time.

The **⋯** segment opens a small popover for anything else. Its **Last**
field has the keyboard, with the current span selected: type a span such as
`90m`, `2 days` or `1 hour 30 min` and press Enter to show it ending now,
following the current time. The chips beside the label, **2h**, **12h**,
**3d** and **30d**, show those spans in one click.

For a fixed range, fill in **From** and **To** as `YYYY-MM-DD HH:MM` (local
time, seconds optional) and press Enter in either. Tab moves from From to To.
An empty To, or `now`, means now, and the range then follows the current
time.

If a field can't be read, the error appears under it and the popover stays
open to try again. Esc, or a click anywhere else, closes the popover without
changing the range.

When you have zoomed or panned away from a preset, no segment is lit and the
pill shows the span you are looking at, such as `3h 20m`.

## Pan and zoom time

- **Drag** the plot left or right to pan. Let go while moving and it glides
  to a stop.
- **Scroll**, or pinch on a trackpad, to zoom about the pointer.
- A horizontal trackpad swipe pans.
- `←` and `→` pan by a tenth of the span; `+` and `-` zoom about the middle.

## Zoom to a box

Right-drag, or shift-drag, a box over the part you want. Both the time and
value axes zoom to it.

## Zoom values

Each value axis scales itself to its traces' visible data, including the
min/max bands. Traces with different units, or values of very different
sizes, get axes of their own; see [Work with value axes](axes.md).

- **Drag** up or down on an axis (its labels), or shift- or ctrl-**scroll**
  over it, to zoom that axis alone.
- Shift- or ctrl-**scroll** over the data zooms every axis about the pointer,
  and dragging the plot pans the values of any axis that is zoomed.
- Zooming an axis turns its autoscale off. The **⤢** button, or `A`, turns it
  back on for all of them.

## Follow now, or look at the past

While the plot follows the current time, the **● Live** pill shows a red dot
and new values appear at the right edge. Panning or zooming into the past
stops following. Click the pill, press `L`, or drag back to the right edge
and let go to follow again.

## Read values

Hover over the plot. A vertical hairline follows the pointer, with a card
showing the time and each visible trace's value there, with its units,
coloured by alarm severity when it is in alarm. Where the archiver has
thinned the data into bins, the card also shows the bin's min and max.

## Start again

Double-click the plot, press `Home`, or click **⌂**: the time range returns
to the last preset and autoscale turns on.

## Zoom the interface

To make the text and controls bigger or smaller (not the data: that is
zooming time or values), use the **Zoom − 100% +** row at the top of the
**≡** menu, or `Ctrl+=` (or `Ctrl++`), `Ctrl+−` and `Ctrl+0`, as in a
browser. They work even while you type in the search box. Clicking the
percentage, or `Ctrl+0`, goes back to 100%. The steps run from 50% to 300%,
and each change shows "Zoom 125%" briefly.

The zoom is remembered for the next start (`ui_zoom` in
[`settings.json`](../reference/config.md#settingsjson)). `--ui-scale 1.25`
or `ARCHIVEVIEW_UI_SCALE=1.25` starts at 125% whatever was saved.

## Undo and redo

Explore freely: zoom in to look at something, then step back. **↶** and
**↷** in the bar undo and redo, as do `Ctrl+Z` and `Ctrl+Shift+Z` (or
`Ctrl+Y`), and the mouse's back and forward buttons, which suit stepping
through a zoom history. Each button's tooltip names the step, such as "Undo
zoom" or "Redo move SR-DI-DCCT-01:LIFETIME to Time (h)". In a narrow window
they are in the **≡** menu.

What can be undone is everything that changes what the plot shows:

- the time range: zooming and panning however done, presets, a custom
  range, and leaving or returning to Live (undo returns to Live if that is
  where the step started);
- each value axis: its zoom and pan, autoscale, log scale and a fixed range;
- a reset, as one step, so an accidental double-click can be undone;
- traces: adding PVs, removing one (it comes back with its colour, its axis,
  its place in the legend and its history, without fetching it again),
  hiding or showing one, moving one to another axis, and renaming an axis;
- opening a view, **Start empty**, and the restored session (the view's
  name in the ≡ menu comes back too; the archiver does not, so switch it
  back in the archiver pill if the view changed it).

One gesture is one step: a drag from press to release, or a run of scrolls
or key presses with no pause longer than about half a second, or an arrow
key held down. Time moving on
while Live is not a step. A new change after undoing clears what could be
redone. About 100 steps are kept, for the session only: they are not saved
with views.

Settings and files are not undone: the search text and recent searches, the
theme and UI zoom, the archiver, saving or deleting a view, the window's size
and position, and an axis picked out by a click.
