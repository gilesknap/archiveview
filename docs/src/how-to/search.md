# Find and add PVs

## Search the archiver

Press `/`, or click the search box at the left of the bar, and type part of
a name. After a short pause the box lists the archiver's matching PVs:

- The first row is highlighted: `↑`/`↓` move through the list, `Enter` adds
  the highlighted row, and a click adds the one clicked.
- A PV whose name is exactly what you typed is always the first row, so
  typing a full name and pressing `Enter` adds that PV, even when other
  names containing it sort before it.
- `Esc` closes the list, and a second `Esc` clears the box.

| you type | archiver is asked for (regex) | matches |
| --- | --- | --- |
| `dcct` | `(?i).*dcct.*` | any name containing `dcct`, in any case (`SR-DI-DCCT-01:SIGNAL`) |
| `ES{Dif}` | `(?i).*ES\{Dif\}.*` | as written: `{`, `(`, `.`, `+` and the like match only themselves |
| `SR-DI-*:SIGNAL` | `SR-DI-.*:SIGNAL` | the whole name, with case; `*` is any run of characters |
| `BL0?T*` | `BL0.T.*` | `?` is any one character |

Text without `*` or `?` is searched as a substring, ignoring case. With `*`
or `?` it is a glob: it must match the whole name, case and all, and every
other character stands for itself.

Click the `.*` toggle inside the box to type a Java regular expression
instead. It must match the **whole** name: `SR-DI` matches nothing, while
`SR-DI.*` works. Start it with `(?i)` to ignore case.

The list shows the first 50 matches; type more of the name to narrow it.

## Add a PV the archiver doesn't know

Type the full name: as well as any archived matches, the list ends with an
**Add "…" as typed** row. Highlight it with `↓` (or click it) and press
`Enter`. The text is added as written, so this is how to add a PV that is
not archived; it plots live only. A PV Access or simulated name
(`pva://name`, `sim://sine`), or several names separated by spaces or
commas, can only be added as typed, so that row comes first and `Enter`
adds it straight away.

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

Without an archiver, there is nothing to search:
the only row is **Add "…" as typed**, so type full names and press `Enter`.
