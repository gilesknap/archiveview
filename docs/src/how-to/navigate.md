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

The value axis scales itself to the visible data, including the min/max
bands. Traces with different units each get their own axis (up to three: the
first on the left, the others on the right, labelled in the trace's colour).
The left axis is the one you can set yourself; the others always scale
themselves:

- **Drag** on the value axis (the labels on the left) to pan values, or
  shift- or ctrl-**scroll** over the plot to zoom them.
- Doing either turns autoscale off. The **⤢** button, or `A`, turns it back on.

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
