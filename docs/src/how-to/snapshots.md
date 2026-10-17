# Take snapshots

`--snapshot` renders the plot to a PNG without opening a window, then
exits. It needs no display and no GPU, so it works in CI, over ssh and in
containers. Use it for documentation, regression checks, and reports.

```bash
archiveview --archiver http://archiver:17668 --snapshot beam.png --range 7d SR-DI-DCCT-01:SIGNAL
```

Everything else on the command line applies as usual: the PVs, `--view`,
`--range` and the archiver.

## Size, scale and theme

```bash
archiveview --snapshot out.png --size 1920x1080 ...
archiveview --snapshot out.png --scale-factor 2 ...      # as on a high-DPI screen
archiveview --snapshot out.png --theme dark ...
```

`--size` is in pixels and defaults to `1280x800`. Snapshots use the light
theme unless `--theme dark` is given.

## Wait for data

The snapshot is taken after `--wait` seconds (default 2), and not before
every archive request has finished, so history is always drawn. It gives up
waiting after 60 seconds. Raise `--wait` to give live PVs time to connect
and gather a few values:

```bash
archiveview --snapshot out.png --wait 5 --range 10m bl01t:SUM
```

## What CI does

CI starts the [mock archiver](../reference/mockarchiver.md) and
`bobview-testioc`, then snapshots a set of views and uploads them as the
`snapshots` artifact of each run:

```bash
archiveview-mockarchiver --port 17668 &
bobview-testioc &
export EPICS_CA_ADDR_LIST=127.0.0.1 EPICS_CA_AUTO_ADDR_LIST=NO
archiveview --archiver http://127.0.0.1:17668 --snapshot snapshots/mock-1h.png \
    --size 1280x800 --wait 5 --range 1h MOCK:SINE MOCK:NOISE MOCK:NAN
archiveview --archiver http://127.0.0.1:17668 --snapshot snapshots/empty.png --size 1280x800
kill %1 %2
```

The set covers the empty state, a short and a long range (raw steps and
min/max bands), the dark theme, a live trace joining its history, a narrow
window and a plot with no archiver. See `.github/workflows/ci.yml` for the
full list. Look at them, rather than only checking they were written: the
plot should fill the window, bands should shade binned data, the time labels
should read well and the chips should be tidy.
