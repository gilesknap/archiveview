# The archiver URL and API

archiveview talks to the
[EPICS Archiver Appliance](https://github.com/archiver-appliance/epicsarchiverap)
over its HTTP retrieval API. It needs only read access to retrieval; nothing
on the management interface.

## The URL

Give the retrieval base, as for any retrieval client:

| `--archiver` | becomes |
| --- | --- |
| `http://host:17668` | `http://host:17668/retrieval` |
| `http://host:17668/` | `http://host:17668/retrieval` |
| `http://host:17668/retrieval` | unchanged |
| `https://archiver.example.org/retrieval` | unchanged (behind a proxy) |

An appliance installed as separate Tomcats serves retrieval on port
**17668** (17665 is management). The single-container quick-start
appliance serves everything on 17665.

## Requests

With `R` the retrieval base:

| purpose | request |
| --- | --- |
| history | `R/data/getData.json?pv=OP(PV)&from=T&to=T` |
| search | `R/bpl/getMatchingPVs?pv=GLOB&limit=50`, or `regex=RE` |
| PV type | `R/bpl/getMetadata?pv=PV`, once per PV |
| is it there? | `R/bpl/getVersion`, at start-up and after a change of archiver, then every minute while it fails |

Times are sent in UTC as `2026-10-17T09:00:00.000Z`. `OP` is
`optimized_N` for scalar numeric PVs, with N twice the plot's width in
device pixels, at most 4000; other PVs are fetched raw. See
[Architecture](../explanations/architecture.md#fetching-history).

## What the client copes with

The appliance's JSON has some quirks, all handled:

- `NaN`, `Infinity` and `-Infinity` appear as bare tokens, which is not valid
  JSON; they are read as gaps.
- A range with no data returns `[ ]`, with no metadata.
- Metadata values such as `PREC` may be strings (`"3"`) or numbers.
- The first sample may be earlier than `from`: it is the value in force at
  `from`, and is drawn held from the left edge.
- `optimized` bins carry `[mean, std, min, max, count]` as the value,
  timestamped at the bin's centre.
- Severity 3 with a `cnxlostepsecs` field marks a lost connection, drawn as a
  gap.

## Errors

| response | shown as |
| --- | --- |
| 404 from `getData.json` | the PV is not archived: it plots live only, and its chip says "live only" |
| any other non-2xx | the status and the first line of the reply, on the chip and in the status line |
| no connection | the transport error, likewise; live values carry on |

The archiver pill turns amber for errors that are the archiver's, once a
`getVersion` request confirms them: no connection, HTTP 502, 503 or 504,
or a URL that is not an archiver (404 for data and search alike). A 404
for one PV, a 400 for an odd name, or a 500 for a bad search pattern leave
it as it was. A server that answers `getVersion` with another HTTP error
(a proxy refusing it with 401 or 403, say) counts as reachable when it
answers a search.
