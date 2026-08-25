# EvtxTriage

EvtxTriage parses Windows Event Log (`.evtx`) files, streaming each record out of the file chunk-by-chunk rather than buffering the whole log in memory (`triage_evtx::visit_evtx_file` iterates the parser's chunks and hands each parsed record to a callback as it is produced). Each record is enriched using a bundled corpus of per-`(Channel, EventId)` "maps" — YAML definitions that pull specific fields out of an event's XML body via a small XPath-like selector and assemble them into human-readable columns (a logon event's target user and source IP, for example, rather than a raw `EventData` blob). The underlying streaming parser and maps engine live in the `triage-evtx` crate; the `evtx-triage` crate wraps it with the CLI, the embedded maps corpus, and TriageSuite's shared orchestrator/output plumbing.

## Target Windows versions

The modern `.evtx` binary XML format was introduced in Windows Vista / Windows Server 2008, superseding the legacy `.evt` format used by Windows 2000, XP, and Server 2003. EvtxTriage targets `.evtx` files as produced by Windows Vista through Windows 11 and Server 2008 through the current Server release. It does not read legacy `.evt` files (`validate_legacy` checks for the `.evtx` `"ElfFile\0"` signature).

## Compatibility

Output is compatible with Eric Zimmerman's EvtxECmd: the `EventRecord` struct's field order and `#[serde(rename)]` header names are chosen to match EvtxECmd's 27-column CSV exactly, and this is enforced by a unit test (`csv_header_matches_evtxecmd_order`) plus an integration test (`compat_evtxecmd.rs`) that diffs EvtxTriage's CSV against real EvtxECmd CSV fixtures. Value-level details are matched too — e.g. hex integers are uppercased and GUIDs lowercased in `PayloadData*`/`Payload` fields (`normalize_binxml_value`) to mirror EvtxECmd's casing convention, since the underlying `evtx` crate emits the opposite casing for both.

The bundled maps corpus uses EvtxECmd's own compatible maps format: each `.map` file is a YAML document with `Channel`, `EventId`, `Description`, and a `Maps:` list of `{Property, PropertyValue, Values}` entries, the same shape used by EricZimmerman/evtx's `Maps/` directory. `--sync` downloads maps directly from that upstream repository's file tree, so the bundled maps are the current stock EvtxECmd corpus.

## Flags

EvtxTriage takes TriageSuite's shared input/output flags (`-d`/`--directory` or `-f`/`--file`, `--csv`, `--json`, `--csvf`, `--jsonf`, `--pretty`, `--overwrite`, `--nested-output`, `--debug`, `--trace`, `-q`/`--quiet`) plus these EvtxTriage-specific flags (`crates/evtx-triage/src/cli.rs`):

| Flag | Description |
|---|---|
| `--maps <DIR>` | Maps directory override. Default: the bundled corpus embedded at compile time. |
| `--split` | Write one output file per source `.evtx`, named after the source file, instead of one aggregate output. |
| `--sync` | Refresh the bundled maps corpus from GitHub (EricZimmerman/evtx), then exit — does not parse any `.evtx` files. |
| `--id <IDS>` | Include only these Event IDs (comma-separated). |
| `--ex <IDS>` | Exclude these Event IDs (comma-separated). |
| `--ch <SUBSTRING>` | Include only channels whose name contains this substring (case-insensitive). |
| `--sd <ISO8601>` | Start datetime, UTC, RFC 3339 — skip events before this. |
| `--ed <ISO8601>` | End datetime, UTC, RFC 3339 — skip events after this. |
| `--tdt <SECONDS>` | Time-discrepancy threshold in seconds (default `1.0`). Accepted for CLI compatibility but currently unused — EvtxTriage dropped the `TimeDiscrepancy` columns for EvtxECmd CSV parity. |

At least one of `--csv`/`--json` and one of `--directory`/`--file` is required (enforced by the shared `CommonArgs` validator).

## Maps

The maps system is the mechanism that turns a raw event's `EventData`/`UserData` XML into EvtxECmd-style readable columns. For each parsed record, `MapIndex::lookup(channel, event_id)` looks for a map keyed by that exact `(Channel, EventId)` pair:

- **If a map matches**: its `Description` becomes `MapDescription` (unless a map entry overrides it), and each `MapEntry` resolves an XPath-like `Values` selector against the event XML, substitutes the extracted value(s) into the entry's `%placeholder%` template (`PropertyValue`), and assigns the result to a named output column — `UserName`, `RemoteHost`, `ExecutableInfo`, `MapDescription`, or `PayloadData1`–`PayloadData6`.
- **If no map matches**: EvtxTriage falls back to dumping the raw `EventData/Data` elements (name-prefixed when a `Name` attribute is present) directly into `PayloadData1`–`PayloadData6`, up to six values.

The bundled corpus lives at `resources/evtx-maps/` in the workspace root (468 `.map` files at the time of writing) and is embedded into the `EvtxTriage` binary at compile time via `include_dir!` (`crates/evtx-triage/src/maps_embed.rs`). `--maps <DIR>` overrides this with an on-disk directory loaded at runtime instead (`MapIndex::load`).

`--sync` (`crates/evtx-triage/src/sync.rs`) refreshes that bundled corpus: it queries the GitHub tree API for `EricZimmerman/evtx` at `master`, downloads every file under `evtx/Maps/*.map`, and writes them into `resources/evtx-maps/`. This only updates the on-disk corpus — because the maps are embedded at compile time, a refreshed corpus takes effect on the *next build* of the `EvtxTriage` binary, not immediately. `--sync` performs the download and exits; it does not also parse any `.evtx` files in the same invocation.

## Output layout

EvtxTriage uses TriageSuite's shared output layout (`triage-core`'s `OutputLayout`). By default (flat mode) every output file is written directly under the `--csv`/`--json` root, with the record's identity folded into the filename; `--nested-output` switches to the legacy `<root>/EvtxTriage/<identity>/...` tree. Because EvtxTriage's `Tool::scope()` is `SystemWide`, the identity is always `system`.

The single dataset is named `events`, with default basename `EvtxTriage_Output` and NDJSON framing for JSON output. Confirmed from the integration tests:

- Default (aggregate) run: a run-stamped file such as `<YYYYMMDDHHmmss>_EvtxTriage_Output.csv` / `.json`.
- `--split`: one file per source `.evtx`, named `<source-stem>_system.csv` / `.json` (flat mode folds the `system` identity into the name) — the aggregate `..._EvtxTriage_Output.*` file is not produced in this mode.
- `--csvf`/`--jsonf` override the basename portion of the aggregate filename.

## Output fields

The base `EventRecord` struct (`crates/triage-evtx/src/record.rs`) has 27 fields, in this exact order (also the CSV column order, verified by a unit test against EvtxECmd's header):

| # | Column | Notes |
|---|---|---|
| 1 | `RecordNumber` | Mirrors `EventRecordId` (EvtxECmd parity). |
| 2 | `EventRecordId` | The event's record ID within the log. |
| 3 | `TimeCreated` | ISO 8601 UTC, `yyyy-MM-ddTHH:mm:ss.fffffffZ`. Recovers full 100ns FILETIME precision from the record's binary header timestamp when it denotes the same instant as the XML `SystemTime`; otherwise falls back to the (microsecond-precision) XML `SystemTime`. |
| 4 | `EventId` | |
| 5 | `Level` | Friendly name: `LogAlways`, `Critical`, `Error`, `Warning`, `Information`, `Verbose`, or `Unknown`. |
| 6 | `Provider` | `System/Provider@Name`. |
| 7 | `Channel` | |
| 8 | `ProcessId` | `System/Execution@ProcessID`. |
| 9 | `ThreadId` | `System/Execution@ThreadID`. |
| 10 | `Computer` | |
| 11 | `ChunkNumber` | 0-based index of the `.evtx` chunk the record lives in. |
| 12 | `UserId` | `System/Security@UserID` (the record's own security principal SID) — distinct from map-derived `UserName`. |
| 13 | `MapDescription` | From the matching map's `Description`, or a map entry with `Property: MapDescription`. |
| 14 | `UserName` | Map-derived. |
| 15 | `RemoteHost` | Map-derived. |
| 16 | `PayloadData1` | Map-derived, or raw `EventData` fallback when no map matches. |
| 17 | `PayloadData2` | " |
| 18 | `PayloadData3` | " |
| 19 | `PayloadData4` | " |
| 20 | `PayloadData5` | " |
| 21 | `PayloadData6` | " |
| 22 | `ExecutableInfo` | Map-derived. |
| 23 | `HiddenRecord` | Always `"False"` — EvtxTriage only parses live records, never slack/recovered ones. |
| 24 | `SourceFile` | Path to the source `.evtx`. |
| 25 | `Keywords` | Friendly name for the standard audit keywords (`Audit success`, `Audit failure`), else the raw hex keyword value. |
| 26 | `ExtraDataOffset` | Always `0` in the current implementation. |
| 27 | `Payload` | The full `EventData`/`UserData` rendered as JSON, e.g. `{"EventData":{"Data":[{"@Name":"X","#text":"Y"}]}}`, with the same hex-uppercase/GUID-lowercase normalization applied to values. |

Map-derived extra fields are not a separate side payload — they are written directly into the base record's own named columns (`MapDescription`, `UserName`, `RemoteHost`, `ExecutableInfo`, `PayloadData1`–`PayloadData6`). There is no separate `ExtraDataMapped`-style column; the full raw event body is always available in the `Payload` JSON column regardless of whether a map matched.

## Examples

```sh
# Parse every .evtx under a Velociraptor capture into CSV
EvtxTriage --directory /evidence/capture --csv /out/evtx

# Parse specific logs into JSON, pretty-printed
EvtxTriage --file Security.evtx --file System.evtx --json /out/evtx --pretty

# Only Security-channel logon/logoff events (4624/4625/4634) in a time window
EvtxTriage -d /evidence/capture --csv /out/evtx \
  --ch Security --id 4624,4625,4634 \
  --sd 2026-01-01T00:00:00Z --ed 2026-01-31T23:59:59Z

# One output file per source .evtx, using a locally maintained maps directory
EvtxTriage -d /evidence/capture --csv /out/evtx --split --maps /opt/evtx-maps

# Refresh the bundled maps corpus from EricZimmerman/evtx (then rebuild to embed it)
EvtxTriage --sync
```
