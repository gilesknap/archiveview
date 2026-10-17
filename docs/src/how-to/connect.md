# Connect to an archiver and IOCs

archiveview reads history from an Archiver Appliance over HTTP and live
values from IOCs over Channel Access or PV Access. Either can be left out:
with no archiver it plots live values only, and a PV the archiver does not
know about still plots live.

## Point it at the archiver

Pass the appliance's **retrieval** URL, normally port 17668:

```bash
archiveview --archiver http://archiver.example.org:17668 SR-DI-DCCT-01:SIGNAL
```

`/retrieval` is added for you, so `http://host:17668` and
`http://host:17668/retrieval` mean the same. A single-container appliance
that serves everything on one port works the same way, e.g.
`http://host:17665`. See [The archiver URL and API](../reference/archiver-api.md).

`--archiver` is remembered: archiveview writes it to `settings.json` as
`archiver_url` (see [Config files](../reference/config.md)), so later runs
use it without being told. Give `--archiver` again only to change
archivers, or `--no-archiver` for a run with live values only.

At start-up the archiver is chosen in this order:

1. `--archiver URL`;
2. none, with `--no-archiver`;
3. the archiver the opened view was saved with (the `default` view
   restored at start-up, or `--view NAME`);
4. `archiver_url` in `settings.json`.

## See and change the archiver

The pill right of **Live** in the top bar shows the archiver's host and a
dot:

| dot | meaning |
| --- | --- |
| green | the archiver answers |
| amber | it does not: no connection, a gateway error, or a URL that is not an archiver |
| spinner | checking, at start-up or after a change |
| hollow grey, "No archiver" | none is set: live values only |

archiveview asks the appliance for its version (`bpl/getVersion`) at
start-up and after a change. After that every history fetch and search
updates the dot: a success turns it green, and a failure that looks like
the archiver's is checked with another version request before the dot
turns amber (one slow query timing out is not the archiver down). While
it is amber the archiver is checked again every minute, so it turns green
when the archiver comes back. A PV that is not archived (a 404 for that
PV) is not an archiver error.

Hover over the pill for the full retrieval URL, the appliance's version,
and the last error and when it happened. Click it to switch archivers
without restarting: type a URL and press Enter (or **Use**), pick one of
the recently used archivers, or choose **No archiver**. **Use** on the
archiver already in use checks it again at once. The plotted PVs'
history is fetched again from the new archiver.

A switch to another archiver, in the pill or by opening a view saved with
one, is remembered in `settings.json` like `--archiver`. **No archiver**
is for this run only: the settings keep the last archiver, so the next
run without `--archiver` uses it again. In a narrow window the pill
shrinks to its dot.

## Reach the IOCs

Live values use the standard EPICS client environment variables, as any CA
or PVA client does:

```bash
export EPICS_CA_ADDR_LIST="10.0.0.10" EPICS_CA_AUTO_ADDR_LIST=NO
export EPICS_PVA_ADDR_LIST="10.0.0.10" EPICS_PVA_AUTO_ADDR_LIST=NO
```

A PV name with no prefix uses Channel Access. Prefix it with `pva://` for PV
Access, e.g. `pva://bl01t:SUM`. The other prefixes of bobview's `pv` crate
also work for live values (`sim://sine`, `loc://name`), which is handy for
trying the plot out; the archiver is asked only about plain names.

archiveview never writes to PVs.

## Check what is happening

The plot's chips show a PV's state: a spinner while its history loads, a
grey "—" when it is disconnected (hover for the reason), and a "live only"
mark when the archiver has no record of it (HTTP 404). For more detail run
with `ARCHIVEVIEW_LOG=debug`, which logs every archiver request.
