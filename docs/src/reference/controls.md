# Gestures and keys

## The bar

| control | what it does |
| --- | --- |
| search box | find archived PVs as you type; Enter adds. See [Find and add PVs](../how-to/search.md) |
| `.*` (in the search box, shown while it has the focus) | search with a regular expression instead of a glob |
| ✕ (in the search box) | clear it |
| **1h 8h 1d 7d** | show that span ending now, following live |
| **⋯** | a custom span (`90m`, `2 days`) or a start and end date-time |
| **● Live** | red dot while following now (it pulses for a few seconds, and while the pointer is over the plot); click to pause, or to jump to now and follow |
| **⤢** | autoscale every value axis |
| **⌂** | reset: back to the last preset, autoscale on |
| **≡** | Log scale (every value axis); then Views (Save NAME, the Save as… field, Save as default, Open ▸, Delete ▸, Save default on exit); then Theme; then a summary of the gestures |

In a narrow window the presets become a drop-down, and ⤢ and ⌂ move into
the ≡ menu.

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
| double-click a value axis' name | rename it (Enter keeps, Esc cancels, empty is automatic) |
| right-click a value axis | its menu: Log scale, Autoscale, Rename… |
| hover | crosshair: a hairline (snapping to a nearby sample), a time tag on the axis, and a card with every visible trace's value as its line shows it there ("—" in a gap) |

## Legend chips

| gesture | effect |
| --- | --- |
| click | hide or show the trace |
| hover | highlight the trace and dim the others |
| click ✕ | remove the PV |
| click the colour dot | choose the trace's value axis, or New axis |
| drag onto a value axis | move the trace to that axis; dropped elsewhere on the plot, to a new axis of its own |

A chip shows the latest value and units, a spinner while history loads, a
grey "—" while disconnected, and a "live only" mark when the archiver has no
history of the PV. Hover it for details.

## Keys

These work whenever no text box has the focus.

| key | effect |
| --- | --- |
| `/` | focus the search box |
| `Esc` | close the search list or popover; clear the search box |
| `←` `→` | pan by a tenth of the span |
| `+` (or `=`) `-` | zoom time in and out about the middle |
| `L` | follow now, or stop following |
| `A` | autoscale |
| `Home` | reset |

In the search list, `↑` `↓` move and `Enter` adds.
