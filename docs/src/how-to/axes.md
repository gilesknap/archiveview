# Work with value axes

You never have to set up a value axis: each PV picks one when its data first
arrives, and the axes name themselves. When the choice is not the one you
want, moving a trace takes a click or two.

## How a new PV picks its axis

When a PV's first data arrives (its archived history, or its live value if
it has no history), archiveview compares it with the axes already on the
plot:

- **Units first.** A PV never joins an axis whose traces have other units:
  a beam current in mA and a lifetime in h get an axis each.
- **Then size.** With the same units (or none), it joins an axis whose
  values are within about a factor of ten of its own, a little more when
  their ranges overlap. SA:X and SA:Y of a BPM, both in mm, share an axis; a
  current of 300 mA and one of 0.3 mA do not.
- **States apart.** Enum and binary PVs only share axes with each other,
  with ticks at whole numbers.
- **Decades.** A PV whose values span several decades (a vacuum gauge) gets
  an axis on a log scale.
- **About four axes at most.** Past four, a new PV joins the axis it is
  closest to.

The choice is made once. Zooming, panning and reloading never move a trace
to another axis; only you do.

The axes alternate sides: the first on the left, the second on the right,
then a third left of the first and a fourth right of the second. The first
chip's axis is always the first.

## Move a trace to another axis

- **Click the colour dot** on its chip for a menu of the axes, and **New
  axis**.
- Or **drag the chip** onto an axis. Drop it anywhere else on the plot to give
  it an axis of its own.

An axis that loses its last trace disappears.

## Pick out an axis' traces

Click an axis (its labels or name) to show its traces in full and dim the
others. Click it again to show them all.

## Zoom one axis

With the pointer over an axis, **drag up or down** or **shift-scroll** to
zoom that axis alone. Over the data, shift-scroll zooms every axis about the
pointer. **⤢** or `A` autoscales them all again.

## Names

An axis names itself from its traces:

1. the quantity from the units, such as **Current (mA)**, **Position (mm)**,
   **Time (h)**; unknown units show as they are, **(counts)**;
2. when two axes have the same units, the part of the PV names that tells
   them apart: **Position X (mm)** and **Position Y (mm)**;
3. without units, the PV's description, when the archiver gives one;
4. otherwise **Axis 1**, **Axis 2**, ...

**Double-click a name** to type your own; Enter keeps it, Esc does not. Clear
it to go back to the automatic name.

An axis with one trace shows its name in the trace's colour. One with several
shows a small dot in each trace's colour by its name.

## Log scale

**Right-click an axis** for its own menu: **Log scale**, **Autoscale** and
**Rename…**. **≡ ▸ Log scale** turns the log scale on or off for every axis.

Everything here (which trace is on which axis, each axis' zoom, log scale
and name) is saved with the view.
