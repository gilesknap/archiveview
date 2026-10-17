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

To avoid typing it each time, put it in `settings.json`:

```json
{ "archiver_url": "http://archiver.example.org:17668" }
```

in `~/.config/archiveview/` (see [Config files](../reference/config.md)).
`--archiver` on the command line wins over the file.

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
mark when the archiver has no record of it (HTTP 404). A `!` on a chip
means its last archive request failed; hover the chip for the error. For more detail run
with `ARCHIVEVIEW_LOG=debug`, which logs every archiver request.
