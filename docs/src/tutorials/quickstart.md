# Quick start

This tutorial installs archiveview and plots archived and live PVs on your
own machine. It takes about ten minutes and needs no EPICS installation, no
IOCs and no real archiver:

- `archiveview-mockarchiver` plays the part of an Archiver Appliance,
  serving 30 days of synthetic history for a handful of PVs;
- `bobview-testioc` (from [bobview](https://github.com/gilesknap/bobview))
  serves the BL01T simulated beamline live, including PVs the mock also
  archives, so you can watch the archive and the live values join up.

You need an x86_64 or aarch64 Linux machine, or a Mac.

## 1. Install

On Debian or Ubuntu, first install the graphics libraries:

```bash
sudo apt install libxkbcommon0 libwayland-client0 libegl1 libgl1-mesa-dri
```

(on RHEL and Fedora see [Install archiveview](../how-to/install.md); macOS
needs nothing). Then install archiveview into `~/.local`:

```bash
curl -sSfL https://raw.githubusercontent.com/gilesknap/archiveview/main/install.sh | bash
archiveview --version
```

If the command is not found, add `export PATH=$HOME/.local/bin:$PATH` to your
shell's start-up file and open a new terminal.

## 2. Start the mock archiver

```bash
archiveview-mockarchiver &
```

It listens on `http://127.0.0.1:17668`, the Archiver Appliance's usual
retrieval port. The PVs it serves are listed in
[Mock archiver](../reference/mockarchiver.md).

## 3. Plot some history

```bash
archiveview --archiver http://127.0.0.1:17668 MOCK:SINE MOCK:NOISE
```

The window opens on the last hour. Each PV is a line in its own colour, and a
chip for each sits along the bottom with its latest value.

- **Hover** over the plot: a crosshair shows every trace's value and the
  time under the pointer.
- **Scroll** to zoom the time axis about the pointer, and **drag** to pan.
  As you zoom out, the archiver sends the data thinned to the plot's width;
  where a pixel holds many samples you see the line's mean with a shaded
  min-to-max band.
- Click **1w** in the time-range pill to see the whole week, then
  **double-click** the plot to return to where you started.
- The pill reading **● 127.0.0.1:17668** is the archiver: its dot is green
  while the archiver answers. Hover over it for details.

archiveview remembers `--archiver`, so from now on you can leave it out.

## 4. Find another PV

Press `/` (or click the search box) and type `ramp`. The archiver's
matching PVs appear as you type; pick `MOCK:RAMP` with the arrow keys and
Enter, or click it.

## 5. Add live values

Start the simulated beamline, and tell Channel Access to look for PVs on this
machine:

```bash
bobview-testioc &
export EPICS_CA_ADDR_LIST=127.0.0.1 EPICS_CA_AUTO_ADDR_LIST=NO
archiveview --range 10m bl01t:SUM
```

(install the IOC first with `cargo install bobview-testioc`, or take it from a
[bobview release](https://github.com/gilesknap/bobview/releases)).

The mock archives `bl01t:SUM` and the IOC serves it live. The line runs from
the archive into the live values at the right edge, and the **● Live** pill
shows a red dot while the plot follows the current time. Drag the plot to the left:
live following stops. Press `L`, or click the pill, to jump back to now.

## 6. Save the view

Open the **≡** menu, type `demo` into the **Save as…** field under Views
and press Enter. Next
time, `archiveview --view demo` reopens the same PVs and time range, from
the same archiver. Plain `archiveview` reopens the plot as you left it. See [Save and load views](../how-to/views.md).

## 7. Finish

Close the window, then stop the background programs:

```bash
kill %1 %2
```

## Next steps

- Point archiveview at your own appliance and IOCs:
  [Connect to an archiver and IOCs](../how-to/connect.md).
- Learn the rest of the gestures: [Navigate the plot](../how-to/navigate.md).
