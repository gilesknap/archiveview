# Find and add PVs

## Search the archiver

Press `/`, or click the search box at the left of the bar, and type part of
a name. After a short pause the box lists the archiver's matching PVs:

- The first match is highlighted: `↑`/`↓` move through the list, `Enter`
  adds the highlighted row, and a click adds the one clicked.
- A PV whose name is exactly what you typed is always the first match, so
  typing a full name and pressing `Enter` adds that PV, even when other
  names containing it sort before it.
- Adding a PV keeps the text and the list, so you can add several from one
  search: `↓`, `Enter`, `↓`, `Enter`, ... PVs already on the plot are
  dimmed with a ✓, and picking one again does nothing.
- `Esc` closes the list and keeps the text (`↓` or a click brings the list back); a
  second `Esc`, or the ✕ in the box, clears it. A click elsewhere closes
  the list and keeps the text.
- Coming back to the box (a click, or `/`) selects its text, so typing
  starts a new search; press `→` or `End` to edit the old one instead.

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

## Reuse a recent search

archiveview remembers a search once you add a PV from it (typos and
half-typed names are not kept), with its `.*` setting. It keeps the last
50, in `history.json` in the [config directory](../reference/config.md), so
they survive a restart. Plain text that differs only in case (`bl16i`,
`BL16I`) is one search, kept as you last typed it.

- **Click the empty box, or press `/`:** the list shows your recent
  searches under **Recent**. Pick one with `↓` and `Enter`, or a click, to
  put it back in the box and search again. Nothing is added until you pick
  a match.
- **While typing:** recent searches that start with (or contain) the text
  come first, marked with a clock, above the archiver's matches. `Enter`
  still adds the first match; move up to a recent search to use it. So
  typing `BL16I` offers `BL16I-EA-*` from yesterday.
- Hover a recent search and click its ✕ to forget it, or choose
  **Clear search history** in the ≡ menu to forget them all.

## Add a PV the archiver doesn't know

Type the full name: as well as any archived matches, the list ends with an
**Add "…" as typed** row. Highlight it with `↓` (or click it) and press
`Enter`. The text is added as written, so this is how to add a PV that is
not archived; it plots live only. A PV Access or simulated name
(`pva://name`, `sim://sine`), or several names separated by spaces or
commas, can only be added as typed, so that row comes first and `Enter`
adds it straight away.

## Add PVs from another window

Select a PV name anywhere, then **middle-click** the plot or its legend: the
PV is added, and a note at the bottom says so. This is the X11 and Wayland
primary selection, the way EDM and MEDM pass PV names around:

- In bobview, middle-click a widget to select its PV name.
- In a terminal, an editor or a web page, select the name with the mouse
  (double-click selects a whole name in most terminals).

Several names separated by spaces, commas, semicolons or new lines add
several PVs. Quotes and brackets around a name are dropped. A word is used
if it could be a PV name: after an optional `ca://` or `pva://`, only the
characters EPICS record names use (letters, digits, `_ - + : [ ] < > { } $`,
and `.` before a field, as in `BL01T:X.VAL`) and at least one letter. A
plain word such as `TEMP1` counts, since some sites' names have no colon.
Numbers, times such as `12:30:00`, URLs, and words ending in `.` or `:` are
left out. If nothing is left, the note says so.

One paste adds at most 20 PVs. If the selection names more (most likely a
screenful selected by accident), nothing is added and the note says how
many it named; middle-click in the search box to paste the text there and
pick from it.

Middle-click in the search box pastes the selection there instead, at the
cursor, so you can edit it or search with it. Text you select in the search
box becomes the primary selection, so you can middle-click it into a
terminal.

`Ctrl+V` with the pointer over the plot (and the search box not focused)
adds the PVs in the clipboard, on every platform. Where there is no primary
selection (macOS, Windows, a Wayland desktop without it), or nothing is
selected, middle-click uses the clipboard too.

Over `ssh -X` the selection goes through your X server, so selecting a name
in a local window and middle-clicking the remote archiveview works.

## Add PVs from the command line

Name them after the options:

```bash
archiveview --archiver http://archiver:17668 SR-DI-DCCT-01:SIGNAL SR-DI-DCCT-01:LIFETIME
```

Globs are not expanded on the command line; each argument is one PV.

## Remove or hide a PV

Each PV has a chip along the bottom of the plot. Click a chip to hide its
trace (the dot turns hollow) and again to show it. Hover a chip to highlight
its trace: it is drawn bolder and on top, with its min/max band, while the
others dim. That finds a trace hidden under another with the same values,
or under the bands of busy PVs. Click its `✕` to remove it.

## With no archiver

Without an archiver, there is nothing to search:
the only row is **Add "…" as typed**, so type full names and press `Enter`.
