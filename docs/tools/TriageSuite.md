# TriageSuite

TriageSuite is the orchestrator binary for the workspace: rather than parsing one artifact type,
it auto-detects a forensic capture and runs every applicable TriageSuite parser over it in a
single command. Captures are detected automatically — a Velociraptor collection (with
`uploads.json` and `client_info.json`) triggers collection mode; a directory containing one or
more Velociraptor collections triggers multi-host folder mode; a `.zip` collection, or a folder
of them, is extracted first and then treated the same way; any other directory falls back to
raw mounted-tree mode (treated as a single host). The orchestrator manages output routing per
host and per tool, bounded parallelism, output formats (CSV or NDJSON), an optional post-pass of
two external forensic binaries (Hayabusa and Takajo), and produces a `run_manifest.json`
chain-of-custody report with per-tool counts, file statistics, and execution outcomes.

## Flags

```
Input (required):
  <CAPTURE>              A Velociraptor collection, a folder of collections, a .zip
                         collection, a folder of .zip captures, or a mounted raw tree

Output (required):
  --out <OUT>            Output root

Output format:
  --csv                  Write CSV output (default on if neither --csv nor --json given)
  --json                 Write NDJSON output

Tool selection:
  --only <KEYS>          Only run these tools (comma-separated keys, e.g. pe,evtx,mft)
  --skip <KEYS>          Skip these tools (comma-separated keys)
  --hunt                 With --only sqle, inspect every file by SQLite content

Execution:
  --overwrite            Replace existing output files
  --jobs <N>             Max tools to run concurrently per host (default: CPU count)
  --heavy-jobs <N>       Max memory-heavy parsers running concurrently (default: 1)
  --no-progress          Disable progress bars (colored status markers are kept on a TTY)

External tools (optional):
  --config <PATH>        TOML config for Hayabusa/Takajo (see External tools section below)
  --profile <NAME>       Named profile to apply from --config's [profiles.<name>] tables;
                         requires --config
```

`--profile` requires `--config` (clap `requires = "config"`); passing `--profile` without
`--config` is a usage error. `--hunt` similarly requires `--only` (clap `requires = "only"`).

## Tool keys (for --only/--skip)

```
pe    PETriage     (Prefetch parser)
jle   JLETriage    (Jump List parser)
le    LETriage     (Shell Link parser)
rb    RBTriage     (Recycle Bin parser)
re    RETriage     (Registry parser)
sbe   SBETriage    (Shellbags parser)
sqle  SQLETriage   (Windows Search parser)
srum  SrumETriage  (System Resource Usage Monitor parser)
sum   SumETriage   (Setup & Execution Monitor parser)
wxt   WxTTriage    (Windows XTension parser)
evtx  EvtxTool     (Windows Event Log parser)
mft   MftTool      (Master File Table parser)
amc   AmcacheTriage   (Amcache execution/inventory parser)
acc   AppCompatTriage (AppCompatCache/ShimCache parser)
```

SQLETriage is intentionally excluded from the default orchestrator selection. Use
`--only sqle` (or include `sqle` in the list) for known database filenames; add
`--hunt` to inspect every file by content. Standalone `SQLETriage` follows the same
known-name default and explicit `--hunt` behavior.

`hayabusa` and `takajo` are **not** entries in this tool-key registry — they're external-binary
stages, not in-process `Tool` implementations — so `--only hayabusa` (or any `--only` list naming
them) is rejected as an unknown key. They can, however, be named in `--skip` as a special-cased
force-disable; see below.

## Collecting a capture (Velociraptor offline collector)

TriageSuite consumes a collection; it does not create one. This section covers the minimum an
offline collector has to be configured with for the parsers to have something to work on.

### What the orchestrator requires structurally

Collection mode is detected by **two files at the collection root**: `uploads.json` and
`client_info.json`. Both must be present, and collected files must live under `uploads/`.
`client_info.json` supplies `Hostname`, `Platform`, and `PlatformVersion`, which become the
per-host output directory name and the OS string in `run_manifest.json`. The Velociraptor offline
collector produces all of this by default — the practical requirement is simply *don't hand
TriageSuite a re-zipped subfolder that lost those two files*.

If those files are absent, the directory is still processed as a **raw mounted tree** (single
host, named after the directory). Parsing works identically; only host/OS attribution is lost.

### Discovery is filename-based, not path-based

Each parser declares filename globs, and discovery walks the capture recursively matching the
**filename component only** — path-component patterns never match. Two consequences:

- The collector's directory layout is irrelevant. Velociraptor's URL-encoded layout
  (`uploads/auto/C%3A/Windows/...`, `uploads/ntfs/%5C%5C.%5CC%3A/...`) works as-is, and so does
  any other nesting.
- Filenames must survive collection intact. A collector that renames or flattens artifacts (e.g.
  writing `SYSTEM` as `SYSTEM_hostname.bin`) makes them invisible to discovery.

### Minimum artifact set

Everything below maps to `Windows.KapeFiles.Targets` target names. Enabling the compound
`_KapeTriage` target plus a `Device` list covers every row in this table in one step, and is the
recommended baseline.

| Key | Artifact | Typical source | KapeFiles target |
|---|---|---|---|
| `mft` | `$MFT`, `$Boot`, `$UsnJrnl:$J` | volume root, via the NTFS accessor | `_MFT`, `_Boot`, `_J` |
| `evtx` | `*.evtx` | `C:\Windows\System32\winevt\Logs\` | `EventLogs` |
| `re` | `NTUSER.DAT`, `UsrClass.dat`, `SOFTWARE`, `SYSTEM`, `SAM`, `SECURITY`, `DEFAULT` | `C:\Windows\System32\config\`, user profiles | `RegistryHives` (or `RegistryHivesSystem` + `RegistryHivesUser`) |
| `sbe` | `NTUSER.DAT`, `UsrClass.dat` | user profiles | as above |
| `acc` | `SYSTEM` | `C:\Windows\System32\config\SYSTEM` | `RegistryHivesSystem` |
| `amc` | `Amcache.hve` | `C:\Windows\AppCompat\Programs\` | `Amcache` |
| `pe` | `*.pf` | `C:\Windows\Prefetch\` | `Prefetch` |
| `le` | `*.lnk` | Recent / Desktop / Office MRU paths | `LNKFilesAndJumpLists` |
| `jle` | `*.automaticDestinations-ms`, `*.customDestinations-ms` | `...\Recent\AutomaticDestinations\` | `LNKFilesAndJumpLists`, `JumpLists` |
| `rb` | `$I*`, `INFO2` | `C:\$Recycle.Bin\<SID>\` | `RecycleBin_InfoFiles` (or `RecycleBin`) |
| `srum` | `SRUDB.dat` | `C:\Windows\System32\sru\` | `SRUM` |
| `sum` | `SystemIdentity.mdb`, `Current.mdb`, role `{GUID}.mdb` | `C:\Windows\System32\LogFiles\Sum\` | `SUM` |
| `wxt` | `ActivitiesCache.db` | `...\ConnectedDevicesPlatform\L.<user>\` | `WindowsTimeline` |
| `sqle` | `*.db`, `*.sqlite`, `History`, `Cookies`, … | application data paths | `SQLiteDatabases` (tool is opt-in; see above) |

Compound targets are spelled with a leading underscore (`_KapeTriage`, `_SANS_Triage`,
`_BasicCollection`), as are the NTFS meta-file targets (`_MFT`, `_Boot`, `_J`). Individual targets
are not. Target names drift between KapeFiles/Velociraptor versions — check the artifact's own
parameter list in your Velociraptor instance rather than assuming this table's spelling.

### Companion files that are easy to miss

Several parsers read files they never advertise in their discovery patterns. Collecting only the
"main" artifact silently degrades output rather than failing loudly:

- **Registry transaction logs.** `RETriage` and `SBETriage` replay `.LOG1`/`.LOG2` siblings of
  each primary hive by default, so a hive collected without its logs is missing the most recent,
  not-yet-flushed writes. The `RegistryHives*` targets collect these already; a hand-rolled
  file-copy collector usually does not. (`--no-logs` opts out of replay.)
- **The whole `Sum\` directory.** `SumETriage` is *discovered* on `SystemIdentity.mdb`, but then
  reads `Current.mdb` and each chained role `{GUID}.mdb` from the same directory. Collecting only
  `SystemIdentity.mdb` yields identity rows with no usage detail.
- **The `SOFTWARE` hive, for SRUM.** `SrumETriage` resolves SIDs to usernames from the closest
  `SOFTWARE` hive in the same capture subtree. Without it, SRUM output still parses but user
  attribution falls back to raw SIDs.

### Expect artifacts to be legitimately absent

Some artifacts do not exist on a given host, and a parser reporting zero files is not a collection
failure:

- **Prefetch is disabled by default on Windows Server.** A server capture normally yields no
  `*.pf` at all, while a workstation yields hundreds.
- **SUM/UAL is a Windows Server role artifact.** `C:\Windows\System32\LogFiles\Sum\` does not
  exist on client Windows.
- **`$I` Recycle Bin files only exist for currently-deleted items.** An empty recycle bin
  collects nothing.

### ZIP archive input

The offline collector ships captures as ZIPs, and `run` takes them directly — a single `.zip`,
a folder of them, or a folder mixing `.zip`s with already-unzipped collections:

```bash
TriageSuite run ./Collection-HOST1.zip --out ./results --csv
TriageSuite run ./engagement-zips     --out ./results --csv   # one run, every host
```

Archives are extracted to **`<out>/_extracted/<archive-name>/`** and **kept** after the run, so
a re-run costs no extraction. Three consequences worth planning for:

- **Disk.** The extracted copy roughly doubles storage for the capture, and a folder of
  multi-GB archives is extracted in full. `_extracted/` is safe to delete between runs.
- **Re-runs reuse.** A marker file records the source archive's name, size and mtime. A second
  run reuses the extraction (`○ … reusing existing extraction`). If the archive changed, the
  stale copy is **refused** rather than silently parsed — rerun with `--overwrite` to re-extract.
  An interrupted extraction leaves no marker and is likewise refused, never treated as complete.
- **`--overwrite` also forces re-extraction.** Previously it governed only tool output files.

Both internal layouts are accepted: the collection at the archive root (what the collector
writes) and a collection under a single wrapper directory (what re-zipping usually produces).
A zip of a *folder of collections* works too.

**Skipping.** An archive that isn't usable is reported and skipped; the run continues and its
exit code is unaffected:

```
✔ Collection-HOST1.zip -> _extracted/Collection-HOST1 (extracted, 1284 files, 3.4 GiB, 47s)
○ notes.zip skipped: no Velociraptor collection inside (uploads.json + client_info.json not found)
○ broken.zip skipped: not a valid zip archive (invalid Zip archive: Could not find EOCD)
```

The one exception: if **nothing** usable is found anywhere, the run exits `3` with
`no usable capture found in <path> (N archive(s) skipped)` rather than reporting an empty
success — so a wrong path or a bad drop folder fails loudly in automation.

**Limits.** Unencrypted archives only; an encrypted one is skipped with a clear message.
Entries that would escape the destination (zip-slip), symlink entries, and entries using an
unsupported compression method are skipped individually without aborting the archive. Entry
names are written verbatim, never percent-decoded, since discovery matches on filename alone.

### Worked example

The configuration used for the captures this tooling was developed against:

```
Artifact: Windows.KapeFiles.Targets
  Device:       C:,D:
  _KapeTriage:  Y
```

That single target produced `$MFT`/`$Boot`/`$UsnJrnl:$J`, every system and user registry hive
with transaction logs, `Amcache.hve`, several hundred `.evtx`, `.lnk` and jump lists, `SRUDB.dat`,
the full `Sum\` directory, and `ActivitiesCache.db` — i.e. input for every parser in the table
above. Add other artifacts (process/network/service collectors) freely; TriageSuite ignores
anything that doesn't match a parser's patterns.

## External tools (Hayabusa / Takajo)

After a host's normal in-process tools finish, the orchestrator can optionally invoke two
external forensic binaries per host:

**Hayabusa** is an EVTX Sigma-rule scanner: it takes a directory of Windows Event Log files and
produces a detection timeline by evaluating them against a Sigma rule set. TriageSuite can run it
up to three times per host — a CSV timeline and a JSONL timeline (both via Hayabusa >= 4.0's
unified `dfir-timeline --output-type csv`/`jsonl`), plus a `logon-summary` pass.

**Takajo** is a Hayabusa-results analyzer: it doesn't touch raw evidence directly. Its
`automagic` subcommand consumes Hayabusa's JSONL timeline output and produces a folder of derived
analysis. TriageSuite chains it automatically after Hayabusa, using the JSONL file the paired
Hayabusa invocation just wrote for that host.

Both tools are entirely optional and **auto-run if their binary is found on `PATH`** — a bare
`triagesuite run <capture> --out <dir>` with no config file and no external binaries installed
runs unaffected; the orchestrator simply reports "not found" for both, with no failure. Neither
tool is ever required for the rest of the run to succeed.

For full field-by-field references, tool-specific behavior, and standalone CLI usage of each
binary, see `docs/tools/Hayabusa.md` and `docs/tools/Takajo.md`.

### Config and profiles

Behavior is driven by an optional TOML file passed via `--config <path>`; there is no
conventional auto-discovered filename — omit it entirely and both tools still auto-run at their
built-in defaults if found. The file has a `[hayabusa]` table, a `[takajo]` table, and any number
of named `[profiles.<name>.hayabusa]` / `[profiles.<name>.takajo]` overlay tables selected with
`--profile <name>`. An overlay only needs to state the fields it changes — every other field
falls through to the base table (an additive per-field merge), and CLI flags do not currently
override individual config fields (only `--skip hayabusa,takajo` acts as a blanket override; see
below). Precedence is: built-in field defaults -> base `[hayabusa]`/`[takajo]` tables -> the
selected profile's overlay.

A representative subset of fields (both tables have more; see the per-tool docs for the complete
list):

| Field | Table | Type | Default | Meaning |
|---|---|---|---|---|
| `bin` | hayabusa, takajo | string | `"hayabusa"` / `"takajo"` | binary name/path (PATH lookup if bare) |
| `enabled` | hayabusa, takajo | bool | `true` | auto-run if found; `false` disables even when present |
| `csv` | hayabusa | bool | `true` | run `dfir-timeline --output-type csv` |
| `json` | hayabusa | bool | `true` | run `dfir-timeline --output-type jsonl`; **required** if `[takajo]` is enabled |
| `logon_summary` | hayabusa | bool | `true` | run `logon-summary` |
| `rules` | hayabusa | string | unset | path to Sigma rules |
| `min_level` | hayabusa | string | unset | minimum alert severity |
| `level` | takajo | string | unset | analysis level passed to `automagic` |

**Validation:** `takajo.enabled = true` requires `hayabusa.json = true` (Takajo's `automagic -t`
needs Hayabusa's JSONL output). Because both default to `true`, this holds with zero
configuration; an explicit contradiction (`hayabusa.json = false` with `takajo.enabled = true`,
in the base table or an active profile) is a config-load error and exits with code `2`, not a
silent skip.

Worked example (base tables plus one named profile) — also shipped as `triage.example.toml`
alongside the binaries in each release archive:

```toml
# triage.toml — optional; a bare `triagesuite run <capture> --out <dir>` works with none of this.

[hayabusa]
bin = "hayabusa"
enabled = true
csv = true
json = true
rules = "./rules"
rules_config = "./rules/config"
min_level = "informational"
threads = 0
proven_rules = false

[takajo]
bin = "takajo"
enabled = true
level = ""
display_table = false

[profiles.quick.hayabusa]
min_level = "high"
proven_rules = true
# rules, csv, json, etc. all inherited from [hayabusa]

[profiles.quick.takajo]
enabled = false   # skip takajo for a quick pass
```

Run with the `quick` profile via `--config triage.toml --profile quick`.

### Disabling with --skip

`hayabusa` and `takajo` can be named in `--skip` alongside ordinary tool keys, e.g.
`--skip hayabusa,takajo` or `--skip re,hayabusa`. Internally, the orchestrator strips those two
names out of the list before it reaches the in-process tool-registry validation (`--only`/`--skip`
for keys like `pe`/`evtx`), since the registry doesn't know about them and would otherwise reject
them as unknown keys. It then checks the original `--skip` list directly: if `hayabusa` is
present, `resolved_external.hayabusa.enabled` is forced to `false`; if `takajo` is present,
`resolved_external.takajo.enabled` is forced to `false` — regardless of what the config file or
selected profile set `enabled` to. This makes `--skip hayabusa,takajo` an unconditional,
CLI-level force-disable for a single run, without needing to edit or maintain a config file.

### run_manifest.json shape

Each host entry carries an `external_tools` array (alongside the existing `tools` array),
one entry per invocation attempted:

```json
"external_tools": [
  {
    "tool": "hayabusa-csv",
    "found": true,
    "invoked": true,
    "exit_code": 0,
    "output_paths": ["HOST/Hayabusa/timeline.csv"],
    "error": null
  },
  {
    "tool": "hayabusa-json",
    "found": true,
    "invoked": true,
    "exit_code": 0,
    "output_paths": ["HOST/Hayabusa/timeline.jsonl"],
    "error": null
  },
  {
    "tool": "takajo-automagic",
    "found": true,
    "invoked": true,
    "exit_code": 0,
    "output_paths": ["HOST/Takajo"],
    "error": null
  }
]
```

Fields: `tool` (`"hayabusa-csv"`, `"hayabusa-json"`, or `"takajo-automagic"`, or the bare
`"hayabusa"`/`"takajo"` key used for a "not found" entry), `found` (binary resolved on `PATH` or
at the configured path), `invoked` (the subprocess was actually launched), `exit_code` (process
exit code, `null` if never invoked), `output_paths` (only populated when the run succeeded *and*
the expected output actually exists on disk), and `error` (omitted from the JSON when `null` —
holds stderr on a nonzero exit, the spawn error if the binary couldn't be launched, or a
"skipped: hayabusa did not produce a JSONL timeline for this host" message when Takajo is enabled
but its Hayabusa prerequisite didn't produce JSONL for that host).

A disabled tool (`enabled = false`, including via `--skip`) contributes no entry to
`external_tools` at all for that host.

## Startup banner

Before a run starts, `TriageSuite run` prints a colored diagnostic-pulse banner to stderr:
an EKG-style pulse line, a "TRIAGE SUITE" block-letter logo, and a telemetry footer showing
the project name, engine version (`crates/triage-orchestrator`'s own `CARGO_PKG_VERSION`,
so it always matches the running binary), and status/telemetry labels.

The banner is decorative only — it never affects parsing, the manifest, or exit codes.
It is suppressed entirely when stderr is not a terminal (matching every other decoration
in this file); when it is shown, its 256-color ANSI styling additionally honors `NO_COLOR`
the same way the progress bars and status markers below do. The banner's rendering logic
lives in `crates/triage-orchestrator/src/progress_ui.rs` (`banner`/`print_banner`).

## Progress & status

On a terminal (when stderr is a TTY), TriageSuite displays live progress bars: an overall per-host bar showing `Tools [██░░] N/M` and a per-tool block-style progress indicator for each running tool, with green `✔` / red `✘` status markers when tools finish.

When stderr is redirected (not a TTY) or when `--no-progress` is specified, plain text status lines are printed instead. Colored status markers (`✔` / `✘`) are still shown on a TTY even with `--no-progress`; the `NO_COLOR` environment variable is always honored.

## Output layout

`_extracted/` appears only when the input was one or more `.zip` archives; it holds the
extracted captures and is kept after the run (see "ZIP archive input").

```
<out>/
  _extracted/                     # only for .zip input
    <archive-stem>/               # the extracted capture
    <archive-stem>.source.json    # reuse marker: source name, size, mtime
  <HOST1>/
    PETriage/
      system/
        PETriage_Output.csv
        PETriage_Output_Timeline.csv
      users/
        <username>/
          PETriage_Output.csv
          PETriage_Output_Timeline.csv
    LETriage/
      system/
        LETriage_Output.csv
        LETriage_Output.json
      users/
        <username>/
          LETriage_Output.csv
          LETriage_Output.json
    ... (one per selected tool)
    Hayabusa/
      timeline.csv
      timeline.jsonl
    Takajo/
      ... (automagic's own output layout)
  <HOST2>/
    ... (multi-host folder-of-collections mode)
  run_manifest.json
  run_manifest_<run-id>.json
```

Output directories are created per host per tool per tool-specific identity (system or user).
Users are attributed to tools individually (e.g., `LETriage` routes by filesystem path, `RBTriage`
by SID; user attribution varies per tool's logic). When neither `--csv` nor `--json` is given,
CSV output is written by default. `Hayabusa/` and `Takajo/` are per-host, not per-user — they sit
alongside the other tool directories under each host, created only if the corresponding external
tool actually ran.

Each output file is written to a sibling temporary file, flushed, closed, and atomically
renamed on successful completion. Existing destinations are rejected unless `--overwrite`
is supplied. Capture-derived host and user components are sanitized; manifests retain the
original hostname and record the filesystem-safe `output_id`.

## run_manifest.json

A JSON chain-of-custody report written to the output root (`<out>/run_manifest.json`) containing:

```json
{
  "schema_version": 2,
  "run_id": "20260710123456789",
  "orchestrator_version": "x.y.z",
  "started_utc": "ISO 8601 UTC timestamp",
  "finished_utc": "ISO 8601 UTC timestamp",
  "capture_type": "velociraptor|raw",
  "final_exit_status": 0,
  "archives": [
    {
      "archive": "Collection-HOST1.zip",
      "archive_path": "/full/path/to/Collection-HOST1.zip",
      "size_bytes": 1705797,
      "status": "extracted|re-extracted|reused|skipped|failed",
      "extracted_to": "_extracted/Collection-HOST1",
      "files_written": 1284,
      "bytes_written": 3650722201,
      "skipped_entries": 0,
      "skipped_reasons": [],
      "error": null
    },
    ...
  ],
  "hosts": [
    {
      "host": "hostname",
      "output_id": "filesystem-safe-hostname",
      "os": "platform version string",
      "collection": "collection directory name",
      "source_archive": "Collection-HOST1.zip",
      "tools": [
        {
          "tool": "PETriage",
          "key": "pe",
          "files_matched": 12,
          "discovered_candidates": 12,
          "supported": 12,
          "unsupported": 0,
          "corrupt": 0,
          "unreadable": 0,
          "parsed": 12,
          "failed": 0,
          "deduplicated": 0,
          "records": 45,
          "output_paths": ["HOST/PETriage"],
          "reason_samples": [],
          "error": null
        },
        ...
      ],
      "external_tools": [
        {
          "tool": "hayabusa-csv",
          "found": true,
          "invoked": true,
          "exit_code": 0,
          "output_paths": ["HOST/Hayabusa/timeline.csv"],
          "error": null
        },
        ...
      ]
    },
    ...
  ]
}
```

Each tool entry reports:
- `files_matched`: Number of files selected by the tool's patterns and validation.
- `parsed`: Number of files successfully parsed.
- `failed`: Number of files that failed parsing.
- `records`: Total number of records output (rows in CSV or lines in NDJSON).
- `output_paths`: List of output directories created by this tool for this host.
- `error`: Non-null only if the tool execution aborted; contains the error message.

Each `external_tools` entry reports (see the External tools section above for full field
semantics): `tool`, `found`, `invoked`, `exit_code`, `output_paths`, `error`.

Exit codes are: `0` success (including unsupported-only discovery), `2` usage,
`3` missing input, `4` output/manifest failure, `5` mixed artifact success and failure,
and `6` when applicable artifacts existed but all failed.

## Exit codes

- `0` success (including unsupported-only discovery)
- `2` usage (invalid flags, unknown `--only`/`--skip` key, malformed `--config` TOML, unknown
  `--profile` name, or the `takajo.enabled` / `hayabusa.json` validation conflict)
- `3` missing input (capture path not found or not detectable, including a `.zip` input where
  no archive yielded a usable capture and no other collection was present)
- `4` output/manifest failure
- `5` mixed artifact success and failure
- `6` applicable artifacts existed but all failed

These exit codes describe the in-process tool run and manifest write; external-tool (Hayabusa /
Takajo) outcomes are reported per-invocation in the manifest's `external_tools` array and do not
change the process exit code. Skipped input archives behave the same way — they are recorded in
the manifest's `archives` array and never change the exit code, with the single exception of the
`3` case above.

## Notes

**ESE revision support:** SrumETriage and SumETriage support the modern revision-300 page
layout used by Windows 11 24H2 and Server 2025, in addition to the older fixture revisions.

**Internal API changes:** `Tool::validate` now returns structured `Validation`, tools declare
a `ResourceClass`, and all static/dynamic output must flow through `OutputRouter`. Parser
emission callbacks are fallible; downstream workspace crates must propagate write failures.

**Default output format:** When both `--csv` and `--json` are absent, CSV is written.

**Hayabusa/Takajo are not TriageSuite parsers:** they're independent, unmodified upstream
binaries the orchestrator shells out to; TriageSuite's own parsers (PETriage, EvtxTool, etc.)
separately aim for output compatible with the equivalent Eric Zimmerman tools where one exists,
but that compatibility claim does not extend to Hayabusa or Takajo — they have no Zimmerman
equivalent and no such claim is made for them.

## Examples

Run every tool over a single Velociraptor collection:

```bash
TriageSuite run /mnt/triage --out ./results
```

Run every tool over a folder of multi-host Velociraptor collections:

```bash
TriageSuite run /evidence/captures --out ./results --json --overwrite
```

Run a subset of tools (Prefetch, Event Log, MFT) over a capture with bounded parallelism:

```bash
TriageSuite run /mnt/triage --out ./results --only pe,evtx,mft --csv --jobs 2
```

Run all tools except the Registry and Event Log parsers:

```bash
TriageSuite run /mnt/triage --out ./results --skip re,evtx
```

Run against a raw mounted tree with NDJSON output:

```bash
TriageSuite run /media/suspect/C --out ./results --json
```

Run every parser plus Hayabusa/Takajo using a config file and a named profile for a quick pass:

```bash
TriageSuite run /mnt/triage --out ./results --config triage.toml --profile quick
```

Run every parser but force-disable both external tools for this run only, without touching the
config file:

```bash
TriageSuite run /mnt/triage --out ./results --config triage.toml --skip hayabusa,takajo
```
