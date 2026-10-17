# Find and add PVs

## Search the archiver

Press `/`, or click the search box at the left of the bar, and type part of
a name. After a short pause the box lists the archiver's matching PVs:

- `↑`/`↓` move through the list, `Enter` adds the highlighted PV, and a click
  adds the one clicked.
- `Esc` closes the list, and a second `Esc` clears the box.

What you type is a glob, as the Archiver Appliance understands it:

| you type | archiver is asked for | matches |
| --- | --- | --- |
| `DCCT` | `*DCCT*` | any name containing `DCCT` |
| `SR-DI-*:SIGNAL` | `SR-DI-*:SIGNAL` | as written; `*` is any run of characters |
| `BL0?T*` | `BL0?T*` | `?` is any one character |

Text without `*` or `?` is searched as a substring. Note that the appliance
turns a glob into a regular expression naively, so a `.` in the name matches
any character.

Click the `.*` toggle inside the box to type a Java regular expression
instead. It must match the **whole** name: `SR-DI` matches nothing, while
`SR-DI.*` works.

The list shows the first 50 matches; type more of the name to narrow it.

## Add a PV the archiver doesn't know

Type the full name and press `Enter` with nothing highlighted. The text is
added as written, so this is how to add a PV that is not archived, a PV
Access PV (`pva://name`), or a simulated one (`sim://sine`). It plots live
only.

## Add PVs from the command line

Name them after the options:

```bash
archiveview --archiver http://archiver:17668 SR-DI-DCCT-01:SIGNAL SR-DI-DCCT-01:LIFETIME
```

Globs are not expanded on the command line; each argument is one PV.

## Remove or hide a PV

Each PV has a chip along the bottom of the plot. Click a chip to hide its
trace (the dot turns hollow) and again to show it. Hover a chip to highlight
its trace, and click its `✕` to remove it.

## With no archiver

Without `--archiver` or a saved `archiver_url`, there is nothing to search;
type full names and press `Enter`.
