# Navigate the plot

Everything happens on the plot itself; there are no axis dialogs. The full
list is in [Gestures and keys](../reference/controls.md).

## Choose a time range

The pill in the bar holds the presets **1h**, **8h**, **1d** and **7d**. A
preset shows that span ending now, following the current time.

The **⋯** segment opens a small popover for anything else: type a span such
as `90m`, `2 days` or `1 hour 30 min`, or give a start and an end date-time.

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
stops following. Click the pill, press `L`, or drag back to the right edge to
follow again.

## Read values

Hover over the plot. A vertical hairline follows the pointer, with a card
showing the time and each visible trace's value there, with its units,
coloured by alarm severity when it is in alarm. Where the archiver has
thinned the data into bins, the card also shows the bin's min and max.

## Start again

Double-click the plot, press `Home`, or click **⌂**: the time range returns
to the last preset and autoscale turns on.
