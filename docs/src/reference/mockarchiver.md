# Mock archiver

`archiveview-mockarchiver` is a small stand-in for an EPICS Archiver
Appliance. It serves synthetic, deterministic history for a fixed set of PVs
over the same HTTP API and JSON as the real appliance, quirks included. It
is used by the tests, by CI's snapshots, and by the
[quick start](../tutorials/quickstart.md).

```bash
archiveview-mockarchiver                   # http://127.0.0.1:17668
archiveview-mockarchiver --port 18000 --bind 0.0.0.0
```

Point archiveview at it with `--archiver http://127.0.0.1:17668`.

## PVs

History starts 30 days before the mock starts; there is no data before that.
Values are computed from the time, so the same request always gets the same
answer.

| PV | signal |
| --- | --- |
| `MOCK:SINE` | 1 Hz, a sine with a 10-minute period, units `mA`, precision 3 |
| `MOCK:RAMP` | 0.1 Hz sawtooth, 0 to 100 |
| `MOCK:NOISE` | 10 Hz noise |
| `MOCK:SLOW` | one sample every 5 minutes, so `optimized` requests come back raw |
| `MOCK:ENUM` | enum index, 0 to 3 |
| `MOCK:STRING` | text |
| `MOCK:WAVE` | an 8-element waveform |
| `MOCK:NAN` | a sine with `NaN` every 50th sample and periodic disconnections |
| `bl01t:A`, `bl01t:B`, `bl01t:SUM`, `BL01T-MO-SIMC-01:M1.RBV`, `BL01T-MO-SIMC-01:M2.RBV` | the names `bobview-testioc` serves live, so history and live values join up |

## Endpoints

Every endpoint is on the one port.

| endpoint | notes |
| --- | --- |
| `/retrieval/data/getData.json` | `pv`, `from`, `to`. Operators: raw (with the last sample before `from`), `optimized_N`, `mean_N`, `nth_N`, `ncount`, and the `…Sample_N` variants that leave empty bins out |
| `/retrieval/bpl/getMatchingPVs` | `pv` (glob) or `regex`, `limit` |
| `/retrieval/bpl/getMetadata` | `pv` |
| `/mgmt/bpl/getAllPVs` | `pv` (comma-separated globs), `regex`, `limit` |

As the real appliance does, it answers 404 with an HTML page for an unknown
PV, 400 for an unparseable time or `to` before `from`, writes `NaN` as a bare
token, and marks disconnections with severity 3 and `cnxlostepsecs`.

## In Rust tests

The `mockarchiver` library starts one on a free port:

```rust
let mock = mockarchiver::MockArchiver::start_local()?;
let client = archiver::ArchiverClient::new(&mock.url())?;
```
