# Save and load views

A view is the set of PVs on the plot (with their colours and whether they
are shown), the time range and the value-axis settings. Views are saved as
small JSON files in `~/.config/archiveview/views/`, one per name.

## Save a view

Open the **≡** menu at the right of the bar. Under the **Views** heading:

- **Save NAME** saves over the view you opened.
- The **Save as…** field: type a name and press Enter (or **Save**). Names can't be empty, start with `.`, or
  contain `/` or `\`.
- **Save as default** saves to `default`, which opens when archiveview
  starts with no PVs and no `--view`.

A view also records the archiver in use when it was saved.

## Open or delete a view

**≡ ▸ Open ▸ NAME** opens a saved view (the default is listed first). If
the view was saved with a different archiver, archiveview switches to it;
the archiver pill shows the change.
**≡ ▸ Delete ▸ NAME** removes one. From the command line:

```bash
archiveview --view beam
```

## Keep the default up to date

**Save default on exit** (under Views in the ≡ menu) is on unless you turn
it off: archiveview saves the current plot as `default` whenever you quit,
so starting it with no PVs and no `--view` reopens it as you left it, with
the same archiver. The choice is kept in `settings.json`.

When the default view is restored, a notice at the bottom says "Restored
last session" for a few seconds. Its **Start empty** button clears the
plot instead (keeping the archiver). PVs given on the command line start a
new plot of just those PVs rather than adding to the restored one; with
**Save default on exit** on, that plot becomes the default when you quit.

## What is restored

| saved | restored as |
| --- | --- |
| a relative range (e.g. the last 8 hours) | that span ending now, following live |
| an absolute range | exactly those start and end times |
| which trace is on which value axis | as saved: traces do not pick again |
| an axis' autoscale, or its min and max, log scale and name | as saved |
| a trace's colour and hidden state | as saved |
| the archiver | switched to, unless `--archiver` or `--no-archiver` is given |

A run with `--no-archiver` saves the default view on exit with the
archiver it had before, so one live-only run doesn't make the next start
forget it. A view saved without an archiver (or by an older archiveview)
keeps the archiver in use, so the same view works against another appliance; so
does a hand-edited view with the `archiver` field removed.

Views can be edited by hand or shared by copying the file; see
[Config files and the view format](../reference/config.md).
