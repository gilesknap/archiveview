# Gestures and keys

## The bar

| control | what it does |
| --- | --- |
| search box | find archived PVs as you type; Enter adds, keeping the search to add more. Empty, it lists your recent searches. Middle-click pastes the primary selection. See [Find and add PVs](../how-to/search.md) |
| `.*` (in the search box, shown while it has the focus) | search with a regular expression instead of a glob |
| ✕ (in the search box) | clear it |
| **1h 8h 1d 1w** | show that span ending now, following live |
| **⋯** | a custom span (`90m`, `2 days`, `1y`, or a 2h 12h 3d 4w 1y chip) or a start and end date-time; Enter in a field shows it, Esc or a click away closes it unchanged. See [Choose a time range](../how-to/navigate.md#choose-a-time-range) |
| **● Live** | red dot while following now (it pulses for a few seconds, and while the pointer is over the plot); click to pause, or to jump to now and follow |
| **● host** | the archiver: green when it answers, amber when it fails, a spinner while checking, hollow grey for "No archiver". Hover for its URL, version and last error; click to switch archivers. See [Connect](../how-to/connect.md#see-and-change-the-archiver) |
| **↶** **↷** | undo and redo the last change to what the plot shows; the tooltip names it ("Undo zoom"). Greyed out with nothing to undo or redo. See [Undo and redo](../how-to/navigate.md#undo-and-redo) |
| **⤢** | autoscale every value axis |
| **⌂** | reset: back to the last preset, autoscale on |
| **≡** | Log scale (every value axis), Clear search history; then Views (Save NAME, the Save as… field, Save as default, Open ▸, Delete ▸, Save default on exit); then Theme; then a summary of the gestures |

In a narrow window the presets become a drop-down, the archiver pill shows
only its dot, and ↶ ↷, ⤢ and ⌂ move into the ≡ menu.

## On the plot

| gesture | effect |
| --- | --- |
| drag | pan time, with a glide after a quick release; stops following live |
| scroll wheel, trackpad pinch | zoom time about the pointer |
| horizontal trackpad swipe | pan time |
| shift- or ctrl-scroll | over an axis: zoom that axis; over the data: zoom every axis about the pointer; turns autoscale off |
| drag up or down on a value axis | zoom that axis; turns its autoscale off |
| right-drag, or shift-drag | zoom to the box, time and values |
| double-click | reset |
| click a value axis | pick out its traces, dimming the others; again to undo |
| hover a value axis' name | its full name, units and traces (with their colours), and the gestures |
| double-click a value axis' name | rename it (Enter keeps, Esc cancels, empty is automatic) |
| right-click a value axis | its menu: Log scale, Autoscale, Rename… |
| middle-click (here or on the legend) | add the PVs in the primary selection (the text last selected anywhere), or in the clipboard where there is no primary selection (macOS, Windows). At most 20 at once. See [Add PVs from another window](../how-to/search.md#add-pvs-from-another-window) |
| `Ctrl+V` (pointer over the plot) | add the PVs in the clipboard |
| mouse back and forward buttons | undo and redo, as in a browser |
| hover | crosshair: a hairline (snapping to a nearby sample), a time tag on the axis, and a card with every visible trace's value as its line shows it there ("—" in a gap) |

## Legend chips

| gesture | effect |
| --- | --- |
| click | hide or show the trace |
| hover | highlight the trace (bolder, drawn on top, with its band) and dim the others |
| click ✕ | remove the PV |
| click the colour dot | choose the trace's value axis, or New axis |
| drag onto a value axis | move the trace to that axis; dropped elsewhere on the plot, to a new axis of its own |

A chip shows the latest value and units, a spinner while history loads, a
grey "—" while disconnected, and a "live only" mark when the archiver has no
history of the PV. A long value is cut short with "…", and an array shows
its length and first elements, e.g. `[936] 0, 0, 0, …`. Arrays are not
plotted, and their chip says "not plotted". A `!` in place of the ✕ means
the last archive request failed. Hover a chip for its PV name, its live
and archive status, and these gestures.

## Keys

These work whenever no text box has the focus.

| key | effect |
| --- | --- |
| `/` | focus the search box |
| `Esc` | close the search list or popover; again, clear the search box |
| `←` `→` | pan by a tenth of the span |
| `+` (or `=`) `-` | zoom time in and out about the middle |
| `L` | follow now, or stop following |
| `A` | autoscale |
| `Home` | reset |
| `Ctrl+Z` (`⌘Z` on a Mac) | undo |
| `Ctrl+Shift+Z` or `Ctrl+Y` | redo |

In the search list, `↑` `↓` move and `Enter` adds (or, on a recent search,
searches for it again).
