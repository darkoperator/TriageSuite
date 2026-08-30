# TriageSuite

```
──[ NORMAL DIAGNOSTICS ]──/▄╗▀═══════════/\▄▄_/▀\▄──────[ ALERT: INVESTIGATION ACTIVE ]───
  ████████╗██████╗ ██╗ █████╗  ██████╗ ███████╗   ███████╗██╗   ██╗██╗████████╗███████╗
  ╚══██╔══╝██╔══██╗██║██╔══██╗██╔════╝ ██╔════╝   ██╔════╝██║   ██║██║╚══██╔══╝██╔════╝
     ██║   ██████╔╝██║███████║██║  ███╗█████╗     ███████╗██║   ██║██║   ██║   █████╗
     ██║   ██╔══██╗██║██╔══██║██║   ██║██╔══╝     ╚════██║██║   ██║██║   ██║   ██╔══╝
     ██║   ██║  ██║██║██║  ██║╚██████╔╝███████╗   ███████║╚██████╔╝██║   ██║   ███████╗
     ╚═╝   ╚═╝  ╚═╝╚═╝╚═╝  ╚═╝ ╚═════╝ ╚══════╝   ╚══════╝ ╚═════╝ ╚═╝   ╚═╝   ╚══════╝
  ══════════════════════════════════════════════════════════════════════════════════════
  ├── PROJECT: TRIAGE SUITE        ├── STATUS: COMPROMISED HOST ASSESSMENT
  └── ENGINE:  DFIR Core v0.1.0    └── TELEMETRY: ACTIVE & STREAMING
```

*(Printed in color at the start of every `TriageSuite run`.)*

This project is a collection of Rust command-line forensic parsers for Velociraptor Windows triage captures. They all produce output compatible with Eric Zimmerman tools, given that they have become kind of a industry standard, I wanted to ensure workflows stayed the same with little to no changes. All timestamps are ISO 8601 UTC (`2024-06-29T00:05:00.1234567Z`) regardless of how the source artifact stores them; this is a lesson I learned from working over 50 IR engagements each year, having all timestamps in UTC just makes life easier.


**Author:** Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>

---

## Tools

Each tool has a full reference doc under [`docs/tools/`](docs/tools/) — flags, output layout,
exact output fields, target Windows versions, and worked examples. This README covers build
instructions, the orchestrator, and cross-tool notes; the per-tool docs are the detailed
reference.

| Tool | Description | Reference |
|---|---|---|
| PETriage | Windows Prefetch parser; compatible output with PECmd, verified against reference fixtures | [docs/tools/PETriage.md](docs/tools/PETriage.md) |
| JLETriage | Windows Jump List parser; compatible output with JLECmd, verified across all three captures | [docs/tools/JLETriage.md](docs/tools/JLETriage.md) |
| LETriage | Windows Shell Link (`.lnk`) parser; compatible output with LECmd, verified across all three captures | [docs/tools/LETriage.md](docs/tools/LETriage.md) |
| RBTriage | Windows Recycle Bin parser; compatible output with RBCmd, verified against reference fixtures | [docs/tools/RBTriage.md](docs/tools/RBTriage.md) |
| RETriage | Windows Registry parser; compatible output with RECmd, verified across all three captures | [docs/tools/RETriage.md](docs/tools/RETriage.md) |
| SBETriage | Windows shellbags parser; compatible output with SBECmd, verified against the SBECmd net9 binary | [docs/tools/SBETriage.md](docs/tools/SBETriage.md) |
| SQLETriage | Map-driven SQLite artifact parser; map-format-compatible with SQLECmd (93 bundled maps: browsers, sync clients, Windows Search, more); opt-in orchestrator tool | [docs/tools/SQLETriage.md](docs/tools/SQLETriage.md) |
| SrumETriage | SRUM (`SRUDB.dat`) parser; compatible output with SrumECmd, including ESE revision 300 | [docs/tools/SrumETriage.md](docs/tools/SrumETriage.md) |
| SumETriage | System Usage Monitor parser; compatible output with SumECmd, including ESE revision 300 (server role & licensing tracking, not "Setup & Execution Monitor") | [docs/tools/SumETriage.md](docs/tools/SumETriage.md) |
| WxTTriage | Windows Timeline (`ActivitiesCache.db`) parser; compatible output with WxTCmd (not "Windows XTension") | [docs/tools/WxTTriage.md](docs/tools/WxTTriage.md) |
| EvtxTriage | Windows Event Log (`.evtx`) streaming parser, compatible maps format with EvtxECmd | [docs/tools/EvtxTriage.md](docs/tools/EvtxTriage.md) |
| MFTriage | Streaming `$MFT`, `$J`, and `$Boot` parser; compatible output with MFTECmd | [docs/tools/MFTriage.md](docs/tools/MFTriage.md) |
| AmcacheTriage | New-format Amcache execution/inventory parser (8 datasets); compatible output with AmcacheParser | [docs/tools/AmcacheTriage.md](docs/tools/AmcacheTriage.md) |
| AppCompatTriage | Windows 10/11 AppCompatCache/ShimCache parser; compatible output with AppCompatCacheParser | [docs/tools/AppCompatTriage.md](docs/tools/AppCompatTriage.md) |
| LolTriage | Cross-references AmcacheTriage/AppCompatTriage/MFTriage/PETriage/RBTriage output against LOLDrivers and LOLRMM reference snapshots; no Zimmerman equivalent | [docs/tools/LolTriage.md](docs/tools/LolTriage.md) |
| SrumNetTriage | Rolls up SrumETriage's NetworkUsages/NetworkConnections output into per-day exfil-volume and per-hour-of-day activity tables; no Zimmerman equivalent | [docs/tools/SrumNetTriage.md](docs/tools/SrumNetTriage.md) |
| TriageSuite | Orchestrator: runs every applicable tool over a capture in one command | [docs/tools/TriageSuite.md](docs/tools/TriageSuite.md) |
| Hayabusa | Optional external tool, orchestrated if found on PATH; no Zimmerman equivalent | [docs/tools/Hayabusa.md](docs/tools/Hayabusa.md) |
| Takajo | Optional external tool, chained off Hayabusa's output; no Zimmerman equivalent | [docs/tools/Takajo.md](docs/tools/Takajo.md) |

---

## Build

Requires [Rust](https://rustup.rs/) 1.75 or later.

```bash
cargo build --release
# PETriage binary at: target/release/PETriage
```

---

## PETriage

Windows Prefetch parser. Two datasets matching PECmd's output: a main record per prefetch
file (27 fields) and a timeline of loaded resources. Prefetch format versions 17–31 (XP
through Windows 11) are supported, including MAM-compressed files.

```bash
PETriage -d /mnt/triage --csv ./out --json ./out
```

Full reference: [docs/tools/PETriage.md](docs/tools/PETriage.md) (flags, output layout, all 27+2 fields, more examples).

---

## LETriage

Windows Shell Link (`.lnk`) parser. Parses the full LNK structure — header, target IDs via
the shell-items engine, LinkInfo, StringData, and the distributed link TrackerDataBlock
(MAC + OUI vendor lookup). 27 output fields, column-compatible with LECmd. Adds NDJSON
output that LECmd does not provide.

```bash
LETriage -d /mnt/triage --csv ./out --json ./out
```

Full reference: [docs/tools/LETriage.md](docs/tools/LETriage.md) (flags, output layout, all 27 fields, more examples).

---

## JLETriage

Windows Jump List parser. Handles both Automatic Destinations (OLE compound file, DestList
+ embedded LNKs) and Custom Destinations (flat embedded-LNK sequence). Two datasets,
column-compatible with JLECmd's AutoCsvOut (44 columns) and CustomCsvOut (28 columns).

```bash
JLETriage -d /mnt/triage --csv ./out --json ./out
```

Full reference: [docs/tools/JLETriage.md](docs/tools/JLETriage.md) (flags, output layout, all 44+28 fields, more examples).

---

## RBTriage

Windows Recycle Bin parser. Handles `$I` records in v1 (pre-Windows 10) and v2 (Windows
10+) format, plus legacy INFO2 files. One record per deleted file, column-compatible with
RBCmd, adding JSON output that RBCmd does not provide.

```bash
RBTriage -d /mnt/triage --csv ./out --json ./out
```

Full reference: [docs/tools/RBTriage.md](docs/tools/RBTriage.md) (flags, output layout, fields, known limitations, more examples).

---

## RETriage

Windows Registry parser. Reads any hive (SYSTEM, SOFTWARE, NTUSER.DAT, UsrClass.dat, SAM,
SECURITY, DEFAULT, RegBack copies) via [notatin](https://github.com/strozfriedberg/notatin)
and produces a batch CSV plus per-plugin detail CSVs, column-compatible with RECmd's
DFIRBatch.reb profile (15 batch columns, 34 ported plugins).

```bash
RETriage -d /mnt/triage --csv ./out
```

Full reference: [docs/tools/RETriage.md](docs/tools/RETriage.md) (flags, search flags, the full 34-plugin table, 9 documented accepted deltas, more examples).

---

## SBETriage

Windows shellbags parser. Walks the `BagMRU`/`Bags` registry tree in NTUSER.DAT and
UsrClass.dat, reconstructs each shellbag's absolute path/timestamps/MRU ordering, and
emits a 19-column record model column-compatible with SBECmd.

```bash
SBETriage -d /mnt/triage --csv ./out --json ./out
```

Full reference: [docs/tools/SBETriage.md](docs/tools/SBETriage.md) (flags, supported hive types, all 19 columns, 11 documented accepted deltas, more examples).

---

## SQLETriage

Map-driven SQLite artifact parser: runs a named set of SQL queries (a "map") against a
matched SQLite database and emits one row per query result row, with output columns
determined by the query itself rather than a fixed schema. 93 maps ship bundled, in a
format directly compatible with — and syncable from — Eric Zimmerman's SQLECmd Maps
repository. Coverage spans browsers (Chromium/Edge/Firefox), sync clients (Dropbox,
Google Drive), the SQLite-based Windows Search index (`Windows.db`), and more. Excluded
from the orchestrator's default tool selection — opt in explicitly.

```bash
SQLETriage -d /mnt/triage --csv ./out
```

Full reference: [docs/tools/SQLETriage.md](docs/tools/SQLETriage.md) (flags, the `--sync`/`--hunt` model, bundled-map breakdown, more examples).

---

## SrumETriage

System Resource Usage Monitor (SRUM, `SRUDB.dat`) parser — an ESE database tracking
per-application network/CPU/energy usage, introduced in Windows 8. Parses 7 datasets
(network usage, network connections, app resource use, push notifications, energy usage,
app timeline provider, VFU provider), column-compatible with SrumECmd, including the
modern ESE revision-300 page layout (Windows 11 24H2 / Server 2025).

```bash
SrumETriage -f /mnt/triage/C/Windows/System32/sru/SRUDB.dat --csv ./out
```

Full reference: [docs/tools/SrumETriage.md](docs/tools/SrumETriage.md) (flags, all 7 datasets' fields, more examples).

---

## SumETriage

System Usage Monitor (SUM) parser — the Windows Server role-usage and licensing-tracking
database set (`SystemIdentity.mdb` + chained per-year `Current.mdb` files, ESE format),
used by Automated VM Activation on Windows Server 2012 and later. Column-compatible with
SumECmd, including the modern ESE revision-300 page layout (Server 2025).

```bash
SumETriage -f /mnt/triage/C/Windows/System32/LogFiles/Sum/SystemIdentity.mdb --csv ./out
```

Full reference: [docs/tools/SumETriage.md](docs/tools/SumETriage.md) (flags, dataset fields, more examples).

---

## WxTTriage

Windows Timeline (`ActivitiesCache.db`) parser — a SQLite database that backed the Windows
Timeline feature shipped in the Windows 10 April 2018 Update (1803) and removed around
2004/20H2. Parses the Activity, ActivityOperation, and Activity_PackageId tables, column-
compatible with WxTCmd.

```bash
WxTTriage -d /mnt/triage --csv ./out
```

Full reference: [docs/tools/WxTTriage.md](docs/tools/WxTTriage.md) (flags, dataset fields, more examples).

---

## EvtxTriage

Windows Event Log (`.evtx`) parser — the modern binary-XML format introduced in Vista /
Server 2008. Streaming parser, 27-field base record schema matching EvtxECmd's CSV header
order, enriched via a maps corpus (468 maps bundled, syncable from EricZimmerman/evtx)
compatible with EvtxECmd's own map format.

```bash
EvtxTriage -d /mnt/triage --csv ./out
```

Full reference: [docs/tools/EvtxTriage.md](docs/tools/EvtxTriage.md) (flags, the maps/`--sync` model, all fields, more examples).

---

## MFTriage

NTFS `$MFT`, `$J` (UsnJrnl change journal), and `$Boot` streaming parser, compatible output
with MFTECmd. Every `$MFT`/file-listing record carries a `SourceFile` field, so multi-drive
captures (C: and D: both collected) stay distinguishable by drive.

```bash
MFTriage -d /mnt/triage --csv ./out --fl
```

Full reference: [docs/tools/MFTriage.md](docs/tools/MFTriage.md) (flags, all 4 datasets' fields — 34/7/13/17 columns — multi-drive handling, more examples).

---

## AmcacheTriage

New-format Amcache (`Amcache.hve`) execution/inventory parser — introduced Windows 8,
new-format schema roughly Windows 10 1607 and later. 8 datasets covering programs,
associated/unassociated files, shortcuts, drivers, and devices, compatible output with
AmcacheParser.

```bash
AmcacheTriage -f /mnt/triage/C/Windows/AppCompat/Programs/Amcache.hve --csv ./out
```

Full reference: [docs/tools/AmcacheTriage.md](docs/tools/AmcacheTriage.md) (flags, all 8 datasets' fields, more examples).

---

## AppCompatTriage

AppCompatCache / ShimCache parser — reads the binary cache value from a SYSTEM hive's
`Session Manager\AppCompatCache` key across every `ControlSet00N`. Supports the Windows
10/11 (`"10ts"`) binary layout; compatible output with AppCompatCacheParser. (RETriage also
has a generic `AppCompatCache` registry plugin reading the same value from a different
angle — see [docs/tools/AppCompatTriage.md](docs/tools/AppCompatTriage.md) for how the two relate.)

```bash
AppCompatTriage -f /mnt/triage/C/Windows/System32/config/SYSTEM --csv ./out
```

Full reference: [docs/tools/AppCompatTriage.md](docs/tools/AppCompatTriage.md) (flags, all 7 fields, more examples).

---

## TriageSuite

Orchestrator binary that auto-detects a forensic capture (Velociraptor collection, folder of
collections, or raw mounted directory tree) and runs every applicable TriageSuite parser
over it in one command — plus, optionally, the external tools Hayabusa and Takajo (below)
if their binaries are found. Manages per-host/per-tool output routing, bounded parallelism,
output formats (CSV or NDJSON), and produces a `run_manifest.json` chain-of-custody report.

```bash
TriageSuite run /mnt/triage --out ./results
TriageSuite run /mnt/triage --out ./results --only pe,evtx,mft --csv --jobs 2
TriageSuite run /mnt/triage --out ./results --config triage.toml --profile quick

# Collector .zip archives are taken directly — one, or a whole folder of them
TriageSuite run ./Collection-HOST1.zip --out ./results
TriageSuite run ./engagement-zips      --out ./results
```

Archives are extracted to `<out>/_extracted/` and kept, so re-runs skip extraction; anything
that isn't a usable capture is reported and skipped rather than failing the run.

Collecting the input: see [Collecting a capture](docs/tools/TriageSuite.md#collecting-a-capture-velociraptor-offline-collector)
for the minimum set of artifacts a Velociraptor offline collector should be configured with,
the artifact-to-parser mapping, and the companion files (registry transaction logs, the full
`Sum\` directory, the `SOFTWARE` hive for SRUM) that are easy to leave out.

Full reference: [docs/tools/TriageSuite.md](docs/tools/TriageSuite.md) (all flags, tool keys, progress/status behavior, output layout, the full `run_manifest.json` schema, exit codes, external-tool configuration, more examples).

---

## External tools: Hayabusa and Takajo

TriageSuite can optionally invoke two independently-developed, third-party tools per host —
never TriageSuite parsers. Both are auto-run if their binary is found on PATH; neither is required, and a run with no `--config` behaves exactly as if both were left at their defaults.

- **[Hayabusa](docs/tools/Hayabusa.md)** — Yamato Security's EVTX Sigma-rule fast forensics
  timeline / threat-hunting scanner. Runs up to twice per host (CSV and/or JSONL timeline).
- **[Takajo](docs/tools/Takajo.md)** — Yamato Security's Hayabusa-results analyzer. Chains
  automatically off Hayabusa's JSONL output via its `automagic` mode.

Configure both via an optional TOML file and `--config <path> --profile <name>` on
`TriageSuite run`; disable either (even when configured on) with `--skip hayabusa,takajo`.
See [docs/tools/TriageSuite.md](docs/tools/TriageSuite.md#external-tools-hayabusa--takajo)
for the CLI wiring and [docs/tools/Hayabusa.md](docs/tools/Hayabusa.md) /
[docs/tools/Takajo.md](docs/tools/Takajo.md) for the full config field reference.

---

## Output compatibility

CSV column names and order match the corresponding Zimmerman tool for every tool listed
above; JSON property names match the CSV columns. The one systematic, intentional
difference: all timestamps are written as ISO 8601 UTC strings
(`2024-06-29T00:05:00.0000000Z`), where Zimmerman tools write local-time strings without a
zone designator.

Every tool's per-tool doc under [`docs/tools/`](docs/tools/) documents its own accepted
deltas (RETriage: 9, SBETriage: 11) and known limitations in full — fixture-based
compatibility tests enforce every accepted divergence; any undocumented divergence fails CI.

---

## License

MIT
