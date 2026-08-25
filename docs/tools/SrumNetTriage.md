# SrumNetTriage

Rolls up `SrumETriage`'s `NetworkUsages`/`NetworkConnections` CSV output into three
summary tables that answer two DFIR questions directly: how much data left the host
(exfil volume), and when (hours of abuse). Like `LolTriage`, SrumNetTriage's input is
another TriageSuite tool's CSV output, not raw forensic artifacts — run it as a second
pass after a `SrumETriage` run (or a full `TriageSuite run`) has produced output. No
Zimmerman equivalent.

## Flags

Input (exactly one required):
```
-d, --directory <DIR>         Recursively discover CSV output files under this directory
-f, --file <FILE>             Explicit CSV output file (repeatable)
```

Output (at least one required):
```
--csv <DIR>                   Write CSV output beneath this directory
--json <DIR>                  Write NDJSON output beneath this directory
--csvf <NAME>                 Override the default CSV basename
--jsonf <NAME>                Override the default JSON basename
--pretty                      Pretty-print JSON (no effect on NDJSON-framed output)
--overwrite                   Replace existing output files
--nested-output               Preserve the legacy nested output layout under <root>/SrumNetTriage/<identity>/
```

SrumNetTriage options:
```
--tz <OFFSET>                  Local UTC offset used to bucket activity by hour-of-day
                                and calendar day, as "+HH:MM"/"-HH:MM" (also accepts
                                "UTC"). Default: UTC, or the value auto-detected from
                                --system-hive if given. Always wins over --system-hive
                                when both are passed.
--system-hive <PATH>           Optional SYSTEM hive to auto-detect the local UTC
                                offset from (ControlSet\Control\TimeZoneInformation),
                                instead of passing --tz by hand. Ignored if --tz is
                                also given. Falls back to UTC with a warning if the
                                hive can't be opened or no usable bias value is found.
--business-hours <WINDOW>      Local business-hours window used to flag off-hours
                                activity in the hourly-fingerprint dataset, as
                                "HH:MM-HH:MM". Supports overnight windows (e.g.
                                "22:00-06:00"). Default: "08:00-18:00".
```

`SrumETriage`'s `Timestamp` column is UTC (TriageSuite's project-wide convention — see
the root [README](../../README.md#output-compatibility)). Without `--tz` or
`--system-hive`, `HourOfDay` and `OutsideBusinessHours` reflect UTC, not the host's
local time. `--system-hive` reads a single static offset snapshot
(`ActiveTimeBias` if present, else `Bias` + `StandardBias`) from
`ControlSet00N\Control\TimeZoneInformation` — not a full per-timestamp DST calculation
against `StandardStart`/`DaylightStart` — so a capture spanning a DST transition will
have part of its data off by the DST delta (normally 60 minutes). See [Known
limitations](#known-limitations).

Diagnostics:
```
-q, --quiet                   Suppress per-file informational messages
--debug                       Emit debug-level diagnostics to stderr
--trace                       Emit trace-level diagnostics to stderr (implies --debug)
```

## Output layout

TriageSuite's default output layout is **flat**: every dataset file is written directly
under the `--csv`/`--json` root, with a 14-digit `<yyyyMMddHHmmss>_` run-stamp prefix and
the record's identity folded into the filename. SrumNetTriage's `scope()` is
`SystemWide` (findings are host-level, not per-user), so the identity label is `system`.
Basenames come from the `DatasetSpec` table in `crates/srum-net-triage/src/lib.rs`:

```
<out>/
  system_<yyyyMMddHHmmss>_SrumNetTriage_DailySummary_Output.csv
  system_<yyyyMMddHHmmss>_SrumNetTriage_HourlyFingerprint_Output.csv
  system_<yyyyMMddHHmmss>_SrumNetTriage_SessionSummary_Output.csv
```

Pass `--nested-output` to instead get the legacy tree layout:

```
<out>/
  SrumNetTriage/
    system/
      SrumNetTriage_DailySummary_Output.csv
      SrumNetTriage_HourlyFingerprint_Output.csv
      SrumNetTriage_SessionSummary_Output.csv
```

`--csvf`/`--jsonf` override the primary (`DailySummary`) basename only; the other two
datasets derive their name from it via a suffix (`_HourlyFingerprint`,
`_SessionSummary`), the same convention `SrumETriage` uses for its own multi-dataset
output.

## What it reads

A `.csv` file is accepted only if its header line matches one of two known exact
schemas (content-gated, never by filename): `SrumETriage`'s `NetworkUsages` output or
its `NetworkConnections` output (`crates/srum-net-triage/src/sniff.rs`, mirroring
`crates/srume-triage/src/datasets.rs`'s `NetworkUsageRecord`/`NetworkConnectionRecord`
field order exactly). Anything else under `-d` is silently skipped. Rows with an
unparseable or blank `Timestamp` are skipped rather than grouped under an invented date.

Each matched input file is aggregated independently: if `NetworkUsages` output from
several hosts is merged into one directory, per-file rollups are still separate runs of
`-d`/`-f` rather than a single cross-file rollup (see [Known
limitations](#known-limitations)).

## Output datasets and fields

### DailySummary (`SrumNetTriage_DailySummary_Output`)

Grouped by `(Date, ExeInfo, UserName)`, sourced from `NetworkUsages` rows — the exfil-
volume answer. Sorted by `BytesSentTotal` descending.

| Field | Notes |
|---|---|
| `Date` | Local calendar day (per `--tz`), `YYYY-MM-DD` |
| `ExeInfo` | Executable, from the source row |
| `UserName` | From the source row |
| `BytesSentTotal` | Sum of `BytesSent` for the group |
| `BytesReceivedTotal` | Sum of `BytesReceived` for the group |
| `TotalBytes` | `BytesSentTotal + BytesReceivedTotal` |
| `SampleCount` | Row count in the group (SRUM samples roughly hourly) |
| `DominantInterfaceType` | Most frequent `InterfaceType` in the group |
| `DistinctProfiles` | Count of distinct `L2ProfileId` values in the group |

### HourlyFingerprint (`SrumNetTriage_HourlyFingerprint_Output`)

Grouped by `(ExeInfo, UserName, HourOfDay)`, dates collapsed across the whole input file
— the "hours of abuse" answer. Sorted by `OutsideBusinessHours` descending, then
`PctOfExeBytesSent` descending, so off-hours concentration spikes rank first.

| Field | Notes |
|---|---|
| `ExeInfo`, `UserName` | As in `DailySummary` |
| `HourOfDay` | 0–23, local hour per `--tz` |
| `BytesSentTotal` / `BytesReceivedTotal` | Sum per bucket |
| `SampleCount` | Row count per bucket |
| `PctOfExeBytesSent` | This bucket's `BytesSentTotal` ÷ that exe's all-hour total across the file — normalizes chatty apps so a concentration at an unusual hour stands out even when raw volume is modest |
| `OutsideBusinessHours` | `true` when `HourOfDay` falls outside `--business-hours` |

### SessionSummary (`SrumNetTriage_SessionSummary_Output`)

Grouped by `(Date, ExeInfo, UserName)`, sourced from `NetworkConnections` rows. Sorted
by `TotalConnectedSeconds` descending.

| Field | Notes |
|---|---|
| `Date`, `ExeInfo`, `UserName` | As in `DailySummary` |
| `SessionCount` | Number of connection records in the group |
| `TotalConnectedSeconds` | Sum of `ConnectedTime` for the group |

## Examples

Roll up a prior `SrumETriage` run's output, emitting CSV only:

```bash
SrumNetTriage -d /path/to/prior-srumetriage-output --csv ./out
```

Roll up with an explicit local timezone offset and a custom business-hours window:

```bash
SrumNetTriage -d /path/to/prior-srumetriage-output --csv ./out --tz "-05:00" --business-hours "07:00-19:00"
```

Roll up, auto-detecting the timezone from the capture's SYSTEM hive:

```bash
SrumNetTriage -d /path/to/prior-srumetriage-output --csv ./out --system-hive /mnt/triage/.../Windows/System32/config/SYSTEM
```

Roll up specific CSV files, emitting both CSV and NDJSON:

```bash
SrumNetTriage -f ./SrumETriage_NetworkUsages_Output.csv -f ./SrumETriage_NetworkConnections_Output.csv --csv ./out --json ./out
```

## Known limitations

- `--system-hive` reads a single static bias snapshot, not a DST-aware calculation
  against `StandardStart`/`DaylightStart` — a capture spanning a DST transition will
  have part of its data off by the DST delta (normally 60 minutes).
- `--system-hive` opens the given hive file directly; unlike `RETriage`, it does not
  discover and replay sibling `SYSTEM.LOG1`/`SYSTEM.LOG2` transaction logs.
- Neither `--tz` nor `--system-hive` given defaults to UTC with no warning — pass one
  of them for an accurate hour-of-day/business-hours read.
- Each matched input file is aggregated independently, not merged across files — see
  [What it reads](#what-it-reads).
- Not wired into `TriageSuite run` automatically — run it as an explicit second pass,
  the same convention `LolTriage` uses.
