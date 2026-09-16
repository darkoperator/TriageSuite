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
  --no-timeline          Skip BrowserTriage's derived _Timeline dataset, which is routinely
                         larger than all of its typed datasets combined
  --no-individual        Skip EvtxTriage's per-source-log (channel) individual CSV exports,
                         which are written by default
  --layout <velo|native> Output tree shape (default: velo). `velo` writes a VeloProcessor-
                         shaped category tree; `native` writes the per-tool, per-identity tree
  --skip-hashes          Skip the run's two SHA256 hashing steps and nothing else:
                         CaseInfo/*_OutputHashes.txt (hashes every generated file) and
                         CaseInfo/*_SHA256_HashLog.txt (hashes the source archive), which
                         for MFT-sized CSVs means re-reading several GB. CaseInfo/ is still
                         created for the SysInfo report, and Sessions/, process_logs/ and
                         VeloResults/ are unaffected -- none of them is hashing work
  --no-validate          Skip the pre-flight capture-validation gate (needed for a
                         deliberately minimal synthetic capture built for testing)
  --start <ISO8601>      Run-wide time-range floor, UTC. Reaches only EvtxTriage and
                         Hayabusa/Takajo -- see "Time-range filtering" below
  --end <ISO8601>        Run-wide time-range ceiling, UTC; paired with --start

External tools (optional):
  --config <PATH>        TOML config for Hayabusa/Takajo (see External tools section below)
  --profile <NAME>       Named profile to apply from --config's [profiles.<name>] tables;
                         requires --config
```

`--profile` requires `--config` (clap `requires = "config"`); passing `--profile` without
`--config` is a usage error. `--hunt` similarly requires `--only` (clap `requires = "only"`).
`--end` given before `--start`, or either given as a non-RFC3339 string, is a usage error
(exit `2`).

## `TriageSuite validate <input>`

Runs only the pre-flight capture-validation gate -- the same check `run` performs -- without
processing the capture. Prints the same `Warning:`/`Error:` lines `run` would, then exits
`0` if every capture is valid (warnings do not affect the exit code) or `3` if any is not
(reusing `RunExit::InputMissing`, whose spec code is already 3, rather than a separate exit
mapping):

```bash
TriageSuite validate ./Collection-HOST1.zip
TriageSuite validate ./engagement-zips        # each archive checked on its own
```

A folder is a *container*, not a capture: its own file list is archive names rather than
artifacts, so `validate` expands it into the collector ZIPs and collection directories
inside it and reports each one under its own path. A collection directory is a capture, not
a container, and is checked whole.

`run` applies the same gate, unless `--no-validate` is given, but per collection and *after*
archives have been enumerated and extracted -- not to the path you pointed it at, which for
`run ./engagement-zips` is a container. A collection that fails is skipped and the run
continues with the others, so one deficient archive in an engagement costs that host and no
other. The invocation exits `3` only when the gate left nothing to process; otherwise the exit
status is the tools' as usual. Unlike `run`, `validate` fails on any one bad capture: it
answers "is this input fit to process", and a folder with a deficient archive in it is not.

The skip is recorded in `run_manifest.json`'s `archives` array as **one entry for that input**,
not two. An archive that unpacked cleanly and was then rejected by the gate was extracted *and*
skipped, and the entry says both: `status` flips to `skipped` and `error` carries the gate's
reasons, while `extracted_to`/`files_written` and, crucially, the archive's real `size_bytes`
and `sha256` stay. A second entry naming the extracted directory would have put a directory's
inode size and a null hash under fields called `archive_path`, `size_bytes` and `sha256`, and
wrong values are worse than absent ones in a chain-of-custody record. Produced by a real run
over a folder holding one good collector ZIP and one deliberately artifact-less one, with the
good host still processed and the run exiting `0` (`<input-folder>` stands in for the absolute
path `run` was pointed at; nothing else is edited):

```json
{
  "archive": "Collection-BADHOST.zip",
  "archive_path": "<input-folder>/Collection-BADHOST.zip",
  "size_bytes": 1639,
  "status": "skipped",
  "extracted_to": "_extracted/Collection-BADHOST",
  "files_written": 3,
  "bytes_written": 29,
  "skipped_entries": 0,
  "error": "no event log (.evtx) files found; registry hive not found: SYSTEM; registry hive not found: SOFTWARE; registry hive not found: SAM; registry hive not found: SECURITY",
  "sha256": "6cde90cf2b85a0390ed3dba4a57b313051351e33f6014727339dd6b32cdc6338",
  "sha256_skipped": false
}
```

A gate rejection whose collection did *not* come from an archive -- a collection directory
inside a folder of collections -- gets its own entry named for that directory, with
`size_bytes` and `sha256` explicitly `null`, since neither means anything for a directory
(a `0` there would read as a zero-byte archive). So does an archive that also yielded a
collection which passed: that archive was not itself skipped, so the rejection is filed under
the rejected collection's own path. Note that this is the one way a run over plain directories,
with no `.zip` anywhere, still produces an `archives` array. Again from a real run, a folder
holding one deficient collection directory and one good one:

```json
{
  "archive": "Collection-BADHOST",
  "archive_path": "<input-folder>/Collection-BADHOST",
  "size_bytes": null,
  "status": "skipped",
  "files_written": 0,
  "bytes_written": 0,
  "skipped_entries": 0,
  "error": "no event log (.evtx) files found; registry hive not found: SYSTEM; registry hive not found: SOFTWARE; registry hive not found: SAM; registry hive not found: SECURITY",
  "sha256": null,
  "sha256_skipped": false
}
```

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
browser BrowserTriage  (Chromium/Firefox browser artifact parser)
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
| `browser` | `History`, `Cookies`, `Web Data`, `Bookmarks`, `Login Data`, `Preferences`, `places.sqlite`, `cookies.sqlite`, `formhistory.sqlite`, `logins.json`, `extensions.json` | browser profile directories under user profiles | `Chrome`, `Edge`, `Firefox`, `BraveBrowser`, `Opera`, `Vivaldi` (or the compound `_KapeTriage`) |

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
○ double.zip skipped: double-zipped collection: the archive contains one .zip and nothing else
```

An archive whose only entry is another `.zip` was zipped twice, and is named as such rather
than as the generic "no Velociraptor collection inside" — the two mistakes call for different
actions, and `run` and `validate` word this one identically.

The one exception: if **nothing** usable is found anywhere, the run exits `3` with
`no usable capture found in <path> (N archive(s) skipped)` rather than reporting an empty
success — so a wrong path or a bad drop folder fails loudly in automation. It still writes
`run_manifest.json` (see "A rejected input still gets a manifest" below): a rejected run is
still a run, and it must not leave a reused `--out` holding the previous run's manifest.

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
| `rules_config` | hayabusa | string | unset | path to Hayabusa's rule-config directory |
| `min_level` | hayabusa | string | unset | minimum alert severity |
| `level` | takajo | string | unset | analysis level passed to `automagic` |

**Path resolution:** a relative capture path or `--out` directory is resolved against the
directory you ran `triagesuite` from, before any external tool runs — so which directory you
started from does affect where they end up, but the result is always an absolute path by the
time any tool sees it. `hayabusa.rules` and `hayabusa.rules_config` resolve differently: a
**relative** value there is resolved against Hayabusa's own install directory (where its bundled
`rules/`/`rules/config/` actually ship), not against wherever you ran `triagesuite` from —
Hayabusa's process always runs with its cwd set to its own install directory, and these two
fields exist so that convention keeps working unmodified regardless of your invocation directory.
Use an absolute path for either field if your Sigma rules live somewhere else.

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
# A relative rules/rules_config resolves against Hayabusa's own install
# directory, not the directory triagesuite was run from — see "Path
# resolution" above.
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
`--skip hayabusa,takajo` or `--skip re,hayabusa`. Internally, the orchestrator strips the
external-tool keys — read from the external-tool registry
(`crates/triage-orchestrator/src/external/registry.rs`), not a hardcoded list — out of `--skip`
before it reaches the in-process tool-registry validation (`--only`/`--skip` for keys like
`pe`/`evtx`), since that registry doesn't know about them and would otherwise reject them as
unknown keys. It then re-reads the original `--skip` list and force-disables each external tool
named there, regardless of what the config file or selected profile set `enabled` to. This makes
`--skip hayabusa,takajo` an unconditional, CLI-level force-disable for a single run, without
needing to edit or maintain a config file.

The filtering is deliberately one-way: **`--only` still rejects external-tool keys.** `--only`
selects which in-process parsers run, and an external binary is not one of them, so
`--only hayabusa` exits 2 with `unknown tool key: hayabusa` rather than silently doing nothing.
The two registries' key spaces are kept disjoint by a unit test, since a shared key would make
`--skip <key>` silently stop working for the in-process parser.

### run_manifest.json shape

Each host entry carries an `external_tools` array (alongside the existing `tools` array),
one entry per invocation attempted. Paths below are the default `--layout velo` shape
(`<out>/Processed-<HOST>-<stamp>/EventLogs|ThreatHunting/...`); under `--layout native`
they are `<out>/HOST/Hayabusa/...` and `<out>/HOST/Takajo/...` instead (see
"`--layout native`" below):

```json
"external_tools": [
  {
    "tool": "hayabusa-csv",
    "found": true,
    "invoked": true,
    "exit_code": 0,
    "output_paths": ["<abs-out>/Processed-HOST-<stamp>/EventLogs/timeline.csv"]
  },
  {
    "tool": "hayabusa-json",
    "found": true,
    "invoked": true,
    "exit_code": 0,
    "output_paths": ["<abs-out>/Processed-HOST-<stamp>/EventLogs/timeline.jsonl"]
  },
  {
    "tool": "hayabusa-logon-summary",
    "found": true,
    "invoked": true,
    "exit_code": 0,
    "output_paths": [
      "<abs-out>/Processed-HOST-<stamp>/EventLogs/logon-summary-failed.csv",
      "<abs-out>/Processed-HOST-<stamp>/EventLogs/logon-summary-successful.csv"
    ]
  },
  {
    "tool": "takajo-automagic",
    "found": true,
    "invoked": true,
    "exit_code": 0,
    "output_paths": ["<abs-out>/Processed-HOST-<stamp>/ThreatHunting"]
  }
]
```

The shape above is checked against a run with the real Hayabusa 4.0.0 / Takajo 2.16.1
binaries; `<abs-out>`, `HOST` and `<stamp>` stand in for that run's absolute output root, host
name and run stamp. Note what is *not* there: `error` is omitted, not `null`, on a successful
invocation, and external-tool `output_paths` are always absolute whatever `--out` was given
(see "`output_paths` are paths, not tree-relative keys" below).

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
extracted captures and is kept after the run (see "ZIP archive input"). Everything below it is
shown under the **default `--layout velo`** tree; the legacy `--layout native` tree follows.

### `--layout velo` (default)

Per host, everything lands under one `Processed-<HOST>-<stamp>/` directory, grouped by
forensic category rather than by tool -- matching VeloProcessor's own output shape. `<stamp>`
is `yyyy-MM-ddTHHmmssZ` UTC (`velo_run_stamp` in `crates/triage-core/src/output/router.rs`),
computed once per run and shared by every host and every file in this tree (`TRIAGE_RUN_STAMP`
pins it for reproducible test fixtures; see `CLAUDE.md`).

`LETriage` is used below because it is genuinely per-user (`Scope::UserElseSystem` in
`crates/le-triage/src/lib.rs`) -- unlike, say, `PETriage`, which is `Scope::SystemWide`
(`crates/pe-triage/src/lib.rs`) and has never produced a per-user split.

```
<out>/
  _extracted/                     # only for .zip input
    <archive-stem>/               # the extracted capture
    <archive-stem>.source.json    # reuse marker: source name, size, mtime
  Processed-<HOST1>-<stamp>/
    CaseInfo/
      <stamp>_SysInfo.txt               # host context, read back out of RETriage's output
      <stamp>_SHA256_HashLog.txt        # SHA256 of the run's source archive (or a note that
                                         # there wasn't one -- raw-directory input)
      <stamp>_OutputHashes.txt          # SHA256 + size of every other file this run wrote
                                         # under this Processed-<HOST>-<stamp> directory
    FileSystem/                   # mft, pe, le, jle, rb
      <stamp>_PETriage_results.csv
      <stamp>_PETriage_results_Timeline.csv
      <stamp>_MFTriage_results_$MFT.csv
      <stamp>_MFTriage_results_$Boot.csv
      <stamp>_MFTriage_results_$J.csv
      <stamp>_LETriage_results.csv      # every user merged, plus a trailing TriageUser column
      PerUser/
        <stamp>_LETriage_results_<user>.csv   # one user, Zimmerman-exact columns
        <stamp>_LETriage_results___triagesuite_internal_reclaimed_..._see_merge_rs__.csv
                                              # the same tool's system-scope slice, moved here
                                              # by the merge and folded in as TriageUser=system
                                              # ("PerUser reclaim" below explains the name)
    Registry/                     # re, sbe, amc, acc
      <stamp>_RETriage_results_Batch.csv
      <stamp>_SBETriage_results_Shellbags.csv
      <stamp>_AmcacheTriage_results_*.csv     # AmcacheTriage is Scope::SystemWide: no PerUser/
      <stamp>_AppCompatTriage_results_AppCompatCache.csv
      AppCompatCache_SYSTEM.csv               # RETriage per-plugin, per-hive dynamic output:
      AppCompatCache_SYSTEM_RegBack.csv       # no run stamp, no Velo basename, no merge --
      TypedURLs_NTUSER.DAT_Default.csv        # system-scope hives qualified by source (below)
      PerUser/
        TypedURLs_NTUSER.DAT_<user>.csv       # the same dynamic output, per user
        Batch/
          <stamp>_RETriage_results_Batch_<user>.csv
        Shellbags/
          <stamp>_SBETriage_results_Shellbags_<user>.csv
    EventLogs/                    # evtx, plus hayabusa if enabled
      <stamp>_EvtxTriage_results.csv
      Individual/
        <Channel>.csv              # one per distinct Channel value across every source .evtx,
                                     # unless --no-individual; case-colliding Channel spellings
                                     # fold into one file (see docs/tools/EvtxTriage.md)
      timeline.csv                  # Hayabusa dfir-timeline CSV, if hayabusa.csv is enabled
      timeline.jsonl                # Hayabusa dfir-timeline JSONL, if hayabusa.json is enabled
      logon-summary-successful.csv  # Hayabusa logon-summary, if hayabusa.logon_summary is enabled
      logon-summary-failed.csv
    ThreatHunting/                 # takajo automagic output, if takajo.enabled -- Takajo's own
      ...                         # tree verbatim (ListDomains.txt, StackProcesses.csv,
                                   # scriptblock-logs/, and more -- Takajo decides the shape)
    SystemActivity/                # srum, sum, wxt
      <stamp>_SrumETriage_results_*.csv
      <stamp>_SumETriage_results_*.csv
      <stamp>_WxTTriage_results_Activity.csv        # merged, plus TriageUser
      <stamp>_WxTTriage_results_Activity_PackageId.csv
      PerUser/                                      # one level per dataset discriminator --
        Activity/                                   # see "Per-user slices live under their
          <stamp>_WxTTriage_results_Activity_<user>.csv      # dataset" below
        Activity_PackageId/
          <stamp>_WxTTriage_results_Activity_PackageId_<user>.csv
    BrowserActivity/               # browser -- only created if BrowserTriage found something
      <stamp>_BrowserTriage_results_*.csv
      PerUser/
        <stamp>_BrowserTriage_results_<user>.csv    # the History dataset: no discriminator
        Downloads/
          <stamp>_BrowserTriage_results_Downloads_<user>.csv
        ...                                         # one directory per other dataset
    SQLiteArtifacts/                # sqle -- opt-in, --only sqle
    process_logs/
      PETriage.log                  # one per tool that actually processed something this run
      hayabusa-csv.log              # external tools: the exact command line, then its real
      takajo-automagic.log          # stdout and stderr -- see "Process logs" below
      ...
    Sessions/
      Execution_Analysis.tle_sess   # Timeline Explorer session files (one per session whose
      ...                           # file patterns matched at least one file in this tree)
    VeloResults/
      Custom.TSIR.SystemOverview%2FBasicInformation.csv   # passthrough of the capture's own
      ...                                                 # results/ directory, verbatim
  Processed-<HOST2>-<stamp>/
    ... (multi-host folder-of-collections mode; every host shares the same run's stamp)
  run_manifest.json
  run_manifest_<run-id>.json
```

The tree above is drawn with `--csv` only. `--json` writes an NDJSON file at the same path with
a **`.json`** extension -- not `.jsonl`, which in this tree belongs to Hayabusa's
`timeline.jsonl` alone -- and `--csv --json` writes both. The merge post-pass below runs once
per format, so a merged category-root file and its `PerUser/` slices exist in whichever formats
that dataset was written in. `EventLogs/Individual/<Channel>.json` follows the same rule.

**`--json` is not a guarantee that every CSV gets a sibling.** Three classes of file in this
tree are CSV-only regardless, and anything automating over the tree has to expect them:

| What | Why | Seen on a real `--csv --json` run |
|---|---|---|
| A dataset declaring `csv_only: true` -- today `BrowserTriage`'s and `PETriage`'s `_Timeline` (`crates/browser-triage/src/lib.rs:107`, `crates/pe-triage/src/lib.rs:130`) | the dataset itself opts out; `DatasetSpec::framing` is unused for it | `BrowserActivity/<stamp>_BrowserTriage_results_Timeline.csv` and both `PerUser/Timeline/..._Timeline_<user>.csv` slices exist with no `.json` beside them |
| `RETriage`'s dynamic per-plugin output (`Registry/AppCompatCache_SYSTEM.csv` and its `PerUser/` siblings) | RETriage writes it through the router's CSV-only dynamic entry point, `write_dynamic_csv_row`; `SQLETriage`'s dynamic output uses `write_dynamic_row` and does get both | `Registry/` held ten `.json` files, every one of them a `_results` dataset and none a per-plugin detail file |
| `VeloResults/` | a verbatim passthrough, so it keeps whatever the collector wrote | `.csv`, `.json`, and Velociraptor's own `.json.index`, unchanged |

The first two are properties of the *dataset*, not of the run: the same file is CSV-only on
every run, whatever `--csv`/`--json` combination produced it.

Hayabusa and Takajo are **not** an exception to the category tree under `--layout velo`: each
external tool names its own Velo category directly (`VELO_CATEGORY` in
`crates/triage-orchestrator/src/external/tools/hayabusa.rs` and `.../takajo.rs`, since there is
no in-process registry key for either to look up a category through). Hayabusa's `timeline.csv`
/ `timeline.jsonl` / `logon-summary-*.csv` land in `Processed-<HOST>-<stamp>/EventLogs/`
alongside `EvtxTriage`'s own output; Takajo's `automagic` tree lands in
`Processed-<HOST>-<stamp>/ThreatHunting/`, a category no in-process tool otherwise writes to.
Both are computed from the run's own Velo collection directory (`HostContext::velo_dir`), the
same root every in-process tool's category output derives from. Under `--layout native` only,
they keep the legacy `<out>/<output_id>/Hayabusa` and `<out>/<output_id>/Takajo` per-host
directories shown in the "`--layout native`" section below. Confirmed against real Hayabusa
4.0.0 / Takajo 2.16.1 binaries, `--layout velo`: `timeline.csv`, `timeline.jsonl`,
`logon-summary-successful.csv`, and `logon-summary-failed.csv` under `EventLogs/`, and the
automagic tree (`ListDomains.txt`, `StackProcesses.csv`, `scriptblock-logs/`, and more) under
`ThreatHunting/`, both directly under `Processed-<HOST>-<stamp>/`.

**Per-user slices live under their dataset.** A per-user file's name is
`<stem>_<user>`, where the stem is `<stamp>_<Tool>_results[_<Dataset>]` -- and both halves are
joined with `_`, so on the flat `PerUser/` directory the two could not be told apart. A tool with
both a bare dataset and a discriminated one (`BrowserTriage`'s History and Downloads,
`WxTTriage`'s Activity and Activity_PackageId) has one stem that is a literal `<other>_` prefix of
the other, and a profile whose sanitized name starts with the discriminator closes the gap: the
History slice for a profile named `Downloads_alice` is character-for-character the name the
Downloads dataset writes for a profile named `alice`. Reproduced on a real capture: without
`--overwrite` the second write failed and that dataset was lost, with `--overwrite` it silently
clobbered the first, and the survivor was then merged into the *other* dataset's category-root
file -- browser history rows published as `<stamp>_BrowserTriage_results_Downloads.csv`, tagged
`TriageUser=alice`, a user who does not exist on the machine.

No rule over that filename can separate the two, because it is one filename. So the discriminator
is a directory instead: a dataset's per-user slices go in `PerUser/<Dataset>/`, and a dataset
without a discriminator keeps writing straight into `PerUser/`. A stem and its discriminated
sibling therefore never share a directory, and no profile name -- whatever it is called, on any
filesystem -- can make one dataset's slice readable as another's. Filenames themselves are
unchanged, so a slice copied out of the tree still says which dataset, which user, and which run
it came from. The guard is
`every_per_user_directory_decodes_to_exactly_one_stem`
(`crates/triage-orchestrator/tests/velo_names.rs`), which asserts the decode -- not the naming
rule behind it -- over every stem pair the registry can put in one directory, and over profile
names chosen to look exactly like the ambiguity.

**The `TriageUser` rule, stated precisely:** a category-root file that merges more than one
identity's `PerUser/` output carries a trailing `TriageUser` column; every other file --
including every `PerUser/` file -- is Zimmerman-exact (plus the project-wide UTC timestamp
convention). Which category-root files are merges follows each tool's `Scope`
(`crates/*/src/lib.rs`): `Scope::SystemWide` tools (`MFTriage`, `PETriage`, `AmcacheTriage`,
`AppCompatTriage`, `EvtxTriage`, `SrumETriage`, `SumETriage`) never produce `PerUser/` and never
carry `TriageUser`. `Scope::UserSpecific` tools (`SBETriage`, `WxTTriage`, `RBTriage`,
`BrowserTriage`) write only to `PerUser/`, and the category-root file is always the merge.
`Scope::UserElseSystem` tools (`LETriage`, `RETriage`, `JLETriage`, `SQLETriage`) can produce
*both* a system-scope artifact (written straight to the category-root filename, no `TriageUser`)
and per-user artifacts (`PerUser/`) in the same run; the merge post-pass
(`crates/triage-orchestrator/src/velo/merge.rs`, invoked from `execute.rs`) then tries to
replace that category-root file with the merged, `TriageUser`-tagged version. That replacement
is subject to the same collision rule as everything else in this tree: **without `--overwrite`,
a file that already exists is left alone and the merge is recorded as a (non-fatal) failure**,
so a run that discovered both system-scope and per-user artifacts for one of these four tools
needs `--overwrite` to actually get the merged, `TriageUser`-tagged file -- otherwise the
category-root file for that tool is the system-scope slice only, with no `TriageUser` column.

**PerUser reclaim, and the odd filename an analyst will see there.** For a `Scope::UserElseSystem`
tool, `OutputRouter` writes the system-scope slice straight to the category-root filename first
(`Identity::System` routes there under Velo), the same path the eventual merged file needs to
occupy. When the merge post-pass (`crates/triage-orchestrator/src/velo/merge.rs::merge_per_user`
/ `merge_per_user_ndjson`) finds real `PerUser/` slices for that tool, it *reclaims* that
system-scope file before merging: it renames it into that dataset's `PerUser/`
directory (`PerUser/`, or `PerUser/<Dataset>/` for a dataset with a discriminator) under a
reserved label rather than merging it in place, then writes the merged, `TriageUser`-tagged file
back at the category-root path. The practical effect: that directory ends up holding one file per
real identity
*plus* one extra file for the reclaimed system slice, and the merged category-root file's rows
include that system data with `TriageUser` = `"system"` alongside every real user's rows.

The reclaimed file's name is deliberately unlike any real account:
`<stem>___triagesuite_internal_reclaimed_system_scope_slice_never_a_real_account_name_longer_than_attribution_rs_max_identity_label_incl_full_sha256_suffix_see_merge_rs__.csv`
(the label is `RECLAIM_LABEL` in `merge.rs`; its doc comment there derives why it is
provably longer than any label `Attributor::identity_for` can ever produce -- `sanitize_component`'s
`MAX_COMPONENT_CHARS` cap plus the longest possible collision suffix, `-` plus a full
hex-encoded SHA-256 digest -- so it can never collide with a genuine per-user filename, on any
filesystem, case-sensitive or not). **Seeing this file in `PerUser/` is expected on any
`UserElseSystem` tool that produced both system and per-user output; it is an internal artifact
of the merge post-pass, not a user account and not evidence of one.** The `TriageUser` column
value written for those rows is always the plain `"system"` -- the reserved on-disk label and
the value analysts read in the column are separate concerns.

Confirmed on a real capture (`LETriage`, `--layout velo`, `--overwrite`): `FileSystem/PerUser/`
held `..._LETriage_results_localadmin.csv` (9 rows) and
`..._LETriage_results___triagesuite_internal_reclaimed_..._see_merge_rs__.csv` (5 rows), and the
merged `FileSystem/<stamp>_LETriage_results.csv` at the category root carried 9 rows with
`TriageUser=localadmin` and 5 rows with `TriageUser=system`.

**`--overwrite` and a previously reclaimed slice.** A second run against the same output
directory can find its own reclaim's leftover already sitting at the reclaim's reserved
`PerUser/` filename from a prior run. `merge_per_user`/`merge_per_user_ndjson` treat that
exactly like any other output collision: without `--overwrite`, the run fails rather than
silently folding a stale prior run's system-scope data into this run's merge; with
`--overwrite`, the stale reclaim file is replaced by this run's own reclaim before the merge
proceeds, same as any other collision in this tree. (If the *current* run's own router somehow
wrote directly to that reserved path -- which should never happen, since `RECLAIM_LABEL` is
constructed to be unreachable by any real account name -- the merge refuses unconditionally,
`--overwrite` or not, and reports it as an internal invariant violation rather than guessing.)

**Only this run's own system-scope output is ever reclaimed.** The category-root path can also
be occupied by a *previous* run's merged file -- a per-user-only tool writes nothing there
itself, so on a re-run with the same `TRIAGE_RUN_STAMP` the file waiting at that path is the
last run's merged output, `TriageUser` column and all. The merge only reclaims a category-root
file that this run's router actually published there (it checks the path against
`OutputRouter::finish`'s `FinishReport::published`, the same `router_wrote` set described
below); anything else is an ordinary output collision -- refused without `--overwrite`, rebuilt
from this run's own `PerUser/` slices with it. Without that condition the previous merged file
was renamed into `PerUser/` and folded back in as system-scope data: the CSV merge then failed
on the `TriageUser` column that file already carried (after the rename had already moved the only
merged copy off the category root), and the NDJSON merge silently re-emitted those rows
relabelled `TriageUser=system`.

**And only this run's own per-user slices are ever merged.** The same `router_wrote` set picks
the merge's *sources*: the slices folded into the category-root file are the ones this run
published into `PerUser/[<Dataset>/]`, not whatever is sitting in that directory
(`velo::merge::published_per_user_sources`). The directory accumulates across runs, and a
`--overwrite` rerun only replaces the slices it writes *again* -- a slice for a profile that is
no longer on the host, or for an artifact class that stopped being collected, is simply left
behind. Because the same `TRIAGE_RUN_STAMP` produces the same stem, a leftover and this run's
own output are the same filename, so a directory listing could not tell them apart and merged
the leftover back in under its old identity, with `CaseInfo/<stamp>_OutputHashes.txt` then
attesting to a merged file carrying evidence this capture does not contain. Reproduced on a
real capture (`STDC1`, `--only le`, pinned stamp): a first run merged `Administrator` 84 rows /
`cperez` 2 / `system` 11; planting an extra `PerUser/<stem>_ghost.csv` and rerunning with
`--overwrite` produced the identical tally, with `ghost.csv` still on disk and no `ghost` row
in the merged file. **The leftovers are not deleted** -- this is source selection, not cleanup,
so `PerUser/` can hold slices that no run's merged file contains, and the way to be rid of them
is a fresh `--out`.

**The invariant this rests on:** a file that merges more than one identity's rows carries a
trailing `TriageUser` column; every other file on disk -- including every real `PerUser/` slice
and the reclaimed system slice itself -- is byte-exact against the reference tool's own columns
(Zimmerman-exact, plus the project-wide UTC timestamp convention). `merge_per_user`'s own
`router_wrote`-based guard treats a router-claimed reclaim path as an internal-invariant
violation and refuses to guess (see the doc comment on `merge_per_user` in `merge.rs`), which is
what keeps that invariant enforced rather than merely documented.

The Velo basename itself (`<Tool>_results[_<Discriminator>]`) comes from `velo_basename()` in
`crates/triage-core/src/output/router.rs`; several tools carry a discriminator suffix
(`RETriage_results_Batch`, `SBETriage_results_Shellbags`, `JLETriage_results_AutomaticDestinations`,
`AppCompatTriage_results_AppCompatCache`, `MFTriage_results_$MFT`, and more) -- there is no bare
`<Tool>_results.csv` for these tools. `RETriage`'s and `SQLETriage`'s per-plugin/per-source
dynamic outputs (e.g. `AppCompatCache_SYSTEM.csv`, `BamDam_SYSTEM.csv` sitting directly in
`Registry/`) are a separate mechanism (`OutputRouter::write_dynamic_*`) that keeps its own
runtime-determined filename verbatim -- no run stamp, no Velo basename mangling -- so they are
not `TriageUser`-eligible at all; the caller decides that identity per row.

They are still attributable, by two different means depending on scope. A dynamic file written
for a **user** carries the user in its name and lives in `PerUser/`, exactly like static
per-user output: `Registry/PerUser/TypedURLs_NTUSER.DAT_Administrator.csv`. A dynamic file
written for **system scope** sits at the category root, where every system-scope file of the
category shares one directory -- so RETriage qualifies the stem with the source hive's parent
directory whenever the hive's file name alone would not be unique there
(`AppCompatCache_SYSTEM.csv` for `System32/config/SYSTEM`, but
`AppCompatCache_SYSTEM_RegBack.csv` for `System32/config/RegBack/SYSTEM`, and
`TypedURLs_NTUSER.DAT_Default.csv` / `_LocalService.csv` / `_NetworkService.csv` for the three
service and default profile hives). Without that qualifier those hives appended into one file
with no column recording which rows came from where. SQLETriage's dynamic output solves the
same problem the other way, with the trailing `SourceFile` column SQLECmd itself writes.

What these files do **not** get is a merge: the `TriageUser` post-pass walks a tool's declared
`DatasetSpec`s, and a dynamic file has none, so there is no combined `<Plugin>_<hive>.csv`
across users. Read the per-user files individually.

### Process logs

`process_logs/<Tool>.log`, one per tool that did something this run, is the human-readable
record of what that tool saw. In-process parsers (every TriageSuite tool) run in the same
process, so there is no child stdout to capture: the log carries what was discovered, what
failed validation and why, what failed to parse and why, and a closing summary block of
`files matched` / `parsed` / `failed` / `records` / `duration`. An empty-looking body means
nothing noteworthy was recorded at those steps, not that the tool did nothing -- the summary
block is always there.

**Every closing count has a body line explaining it.** A non-zero `failed:` used to be the
whole story an analyst got; now every file the run set aside, and every step that failed after
parsing, names the artifact and the reason on one line, so a count can be reconciled against
the evidence (`crates/triage-orchestrator/src/execute.rs`):

| Line | When |
|---|---|
| `unsupported: <path> — <reason>` | pre-parse validation did not recognise the file. Counts as `unsupported`, **not** as `failed` -- not every set-aside file is a failure |
| `corrupt: <path> — <reason>` / `unreadable: <path> — <error>` | pre-parse validation rejected the file; both also increment `failed` |
| `deduplicated (content match): <path>` | an identical file was already parsed; counts as `deduplicated` only |
| `parse failed: <path> — <the parser's own message>` | one file failed to parse; the run continues |
| `aborted while parsing <path>: <error>` | a fatal error stopped the file loop; the remaining artifacts were never reached |
| `router failed to open: <error>` / `router failed to close: <error>` | output could not be opened, or the final rename/flush failed -- this is why `records: 0` can sit next to a non-zero `parsed:` |
| `merge failed: <stem> — <reason>` | the Velo `TriageUser` merge post-pass failed for that dataset (see "PerUser reclaim" above) |

The log is uncapped: every parse failure gets a line. The manifest is not -- its
`reason_samples` array keeps only the first 10 reasons *per tool*, a budget
shared with the validation notes above, so on a capture with many rejected files no parse
failure need reach the manifest at all. That asymmetry is deliberate: the manifest stays
readable, and the log stays complete.

**External tools get their real captured output, headed by the exact command line that was
run:**

```
--- command ---
/opt/hayabusa/hayabusa csv-timeline --directory /cases/.../uploads/auto/C%3A/Windows/... --output ...
--- stdout ---
...
--- stderr ---
...
exit code: 0
```

This is the only place a run records the precise invocation of Hayabusa or Takajo -- the
manifest records that the tool ran and what it produced, not its argument vector. When an
external tool's output looks wrong, or a result has to be reproduced by hand months later,
that `--- command ---` line is what to copy. The environment is deliberately **not** logged:
TriageSuite sets no tool-specific environment variables, and dumping the ambient environment
would risk writing whatever secrets happen to be set in it into the evidence tree.

Each invocation writes its own log, named for the invocation rather than the binary, so
Hayabusa's three passes appear as `hayabusa-csv.log`, `hayabusa-json.log` and
`hayabusa-logon-summary.log`. Writing the log is best-effort in one direction only: a failure
to write it never fails the run (the manifest stays authoritative), but an existing log is
never silently truncated -- like every other output, replacing one needs `--overwrite`.

### Timeline Explorer sessions

`Sessions/<Name>.tle_sess` is a Timeline Explorer session: plain JSON naming a set of
already-written CSVs to open together, so an analyst can load a whole line of enquiry in one
step instead of hunting files by hand. Five sessions ship in
`resources/velo/TimelineExplorerSessions.json`:

| Session | The question it answers |
|---|---|
| `Execution_Analysis` | What ran on this system |
| `Lateral_Movement_Logons` | Who connected and from where |
| `Persistence` | What survives reboot |
| `File_And_Folder_Access` | What was opened, and by whom |
| `Network_And_Browser` | Where this host went |

Each session carries a list of glob patterns matched against every file's path relative to
`Processed-<HOST>-<stamp>/`. **A session is written only if at least one of its patterns matched
a file that actually exists** -- a session that matched nothing is skipped rather than written
empty, because Timeline Explorer opening an empty session reads as "the data is missing" rather
than "this run produced none of it" (`crates/triage-orchestrator/src/velo/sessions.rs`). The
paths written into the file are absolute, since that is what Timeline Explorer opens.

**`*` does not cross `/`.** Patterns are compiled with `literal_separator(true)`
(`sessions::compile_pattern`), so `Registry/*_RETriage_results*.csv` selects
`Registry/<stamp>_RETriage_results_Batch.csv` and **not**
`Registry/PerUser/Batch/<stamp>_RETriage_results_Batch_<user>.csv`. That is the point: the
merged file at the category root is built from exactly those slices and carries every one of
their rows plus a `TriageUser` column (see "The `TriageUser` rule" above), so a session that
loaded both put every row into Timeline Explorer twice, with nothing in the data to tell the
copies apart -- and dragged in the reclaimed system-scope slice under its 162-character
internal label as well. A session that genuinely wants a subdirectory still names it, as
`Lateral_Movement_Logons`'s `EventLogs/Individual/Security.csv` does, or uses `**`, which does
cross separators; no shipped pattern uses `**`. The net effect an analyst sees: **a session
lists merged category-root files and named subdirectory files, never a `PerUser/` slice.**
Confirmed on a real two-profile run with the external tools enabled -- all five sessions
written, 25 files between them, none under `PerUser/`, none carrying the reclaim label.

The patterns cover this tree's own output, Hayabusa's (`EventLogs/timeline.csv`) and Takajo's
(`ThreatHunting/TimelineLogon.csv`, `StackSuccessfulLogons.csv`, `StackFailedLogons.csv`), so a
run with `--config` enabling the external tools fills `Lateral_Movement_Logons` out where a run
without it gets the event-log half only.

**Two patterns name output no `TriageSuite run` can produce.** `Execution_Analysis` names
`ThreatHunting/*_LolTriage_results.csv` and `Network_And_Browser` names
`SystemActivity/*_SrumNetTriage_results*.csv`. `LolTriage` and `SrumNetTriage` are **standalone
second-pass binaries**: they read the CSVs an earlier run already wrote rather than the capture,
so the orchestrator never invokes them and neither appears in the `--only`/`--skip` tool-key
registry above. Those two patterns are correct names, not stale ones -- but they match only
after you run the second pass yourself, over a tree the orchestrator has already produced. Until
then the patterns simply match nothing, and the sessions open with their other files. The second
pass is a separate command per host, writing into the category the session expects:

```bash
C=<out>/Processed-<HOST>-<stamp>
LolTriage -d "$C" --csv "$C/ThreatHunting" \
    --csvf "<yyyyMMddHHmmss>_LolTriage_results.csv"
SrumNetTriage -d "$C" --csv "$C/SystemActivity" \
    --csvf "<yyyyMMddHHmmss>_SrumNetTriage_results.csv"
```

The `--csvf` basename matters, and so does its 14-digit run-stamp prefix. These binaries write
the standalone flat layout, which by default names its output `<identity>_<stamp>_<Tool>_Output.csv`
-- `_Output`, not the `_results` the Velo tree and these patterns use. A `--csvf` basename that
starts with a 14-digit stamp is the one case where the flat layout puts the identity label in
*front* rather than before the extension (`OutputLayout::flat_filename`), producing
`system_<stamp>_LolTriage_results.csv`, which is what the pattern's leading `*` is there to
absorb. Verified on a real tree: with the default `--csvf` the file is
`system_<stamp>_LolTriage_Output.csv` and the pattern does not match it.

The `.tle_sess` files are written during the run that produced the tree, so they will not list
a file the second pass added afterwards. Add it to the open session in Timeline Explorer, or
hand-edit the `SessionFiles` object -- it is a flat JSON map of absolute path to an empty array.

### Time-range filtering (`--start`/`--end`)

`--start`/`--end` reach **only** `EvtxTriage` and `Hayabusa` (`Takajo` inherits transitively,
since it consumes Hayabusa's already-filtered JSONL timeline) -- every other tool has no shared
notion of "the" record timestamp to filter on (an `$MFT` record has eight, a Prefetch record up
to eight) and emits its full output regardless. This is stated in three places so an analyst
scanning any one of them sees it: the run prints a one-line notice to stdout when a range is
given, the manifest's per-tool `time_filter` field says `"applied"` or `"not_applicable"`
(`tool_applies_time_filter` in `crates/triage-orchestrator/src/registry.rs`), and
`CaseInfo/<stamp>_SysInfo.txt` carries the same notice text. Treating a `--start`/`--end` run as
uniformly scoped, when only the event-log tools actually filtered, is exactly the wrong
conclusion this three-way redundancy exists to prevent.

### `--layout native`

The legacy per-tool, per-identity tree, unchanged from pre-Velo releases:

```
<out>/
  _extracted/                     # only for .zip input
    <archive-stem>/
    <archive-stem>.source.json
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
tool actually ran. Under `--layout native`, `CaseInfo/`, `process_logs/`, `Sessions/` and
`VeloResults/` are not produced at all -- they are a Velo-layout concept. `--skip-hashes` is a
narrower thing: it suppresses only the two files that are hashing work
(`CaseInfo/<stamp>_OutputHashes.txt` and `CaseInfo/<stamp>_SHA256_HashLog.txt`), so a
`--skip-hashes` run still writes `CaseInfo/<stamp>_SysInfo.txt`, `Sessions/`, `process_logs/`
and `VeloResults/`.

Each output file is written to a sibling temporary file, flushed, closed, and atomically
renamed on successful completion. Existing destinations are rejected unless `--overwrite`
is supplied. Capture-derived host and user components are sanitized; manifests retain the
original hostname and record the filesystem-safe `output_id`.

### Repeated hostnames

A single run can legitimately contain the same host more than once — the same machine
collected on day 1 and day 5, or a re-collection after a partial run. Both collections
report the same `Hostname`, so both would otherwise resolve to one host directory and
overwrite each other.

When a hostname is contested, each of its collections gets its own directory suffixed with
that collection's timestamp:

```
<out>/IT02877_20260724T045726/
<out>/IT02877_20260728T110213/
```

The suffix is derived from the collection itself, never from its position in the run, so a
given capture always resolves to the same directory — adding another collection of the same
host later does not relocate the ones already written. A host with a single collection is
unaffected and keeps the bare `<out>/IT02877/`. `run_manifest.json` maps every `output_id`
back to its `collection`.

## run_manifest.json

A JSON chain-of-custody report written to the output root (`<out>/run_manifest.json`) containing:

Copied verbatim from a produced manifest, not hand-edited. The run was a folder of two
collector ZIPs -- one real collection, one file that is not a collection at all -- with the
real Hayabusa 4.0.0 / Takajo 2.16.1 binaries enabled:

```bash
cd <workdir>
TRIAGE_RUN_STAMP=2026-09-13T120000Z TriageSuite run --out out --csv \
    --config <path-to-external.toml> --no-progress <workdir>/../zips
```

The only edit to the JSON is that `<workdir>` stands in for the absolute working directory on
the machine that produced it. It is trimmed to two of the 14 `tools` entries, two of the four
`external_tools` entries, and the first two of `EvtxTriage`'s 107 `output_paths`; `...` marks
every elision.

```json
{
  "schema_version": 3,
  "run_id": "20260913200120375",
  "orchestrator_version": "0.2.0",
  "started_utc": "2026-09-13T20:01:20.3758620Z",
  "finished_utc": "2026-09-13T20:03:26.9479110Z",
  "capture_type": "velociraptor",
  "final_exit_status": 0,
  "archives": [
    {
      "archive": "Collection-DESKTOP-OA8SHHC.zip",
      "archive_path": "<workdir>/../zips/Collection-DESKTOP-OA8SHHC.zip",
      "size_bytes": 616580589,
      "status": "extracted",
      "extracted_to": "_extracted/Collection-DESKTOP-OA8SHHC",
      "files_written": 1046,
      "bytes_written": 616088545,
      "skipped_entries": 0,
      "sha256": "a4c7cdd3d8ead47a5dcc555da30f9c62a23ccb368fffe1953af77c955ce495bc",
      "sha256_skipped": false
    },
    {
      "archive": "notes.zip",
      "archive_path": "<workdir>/../zips/notes.zip",
      "size_bytes": 184,
      "status": "skipped",
      "files_written": 0,
      "bytes_written": 0,
      "skipped_entries": 0,
      "error": "no Velociraptor collection inside (uploads.json + client_info.json not found)",
      "sha256": "b41fffea300eef70b42e30939eed8f66a30f631d0964cb80e056a7fb1d3a17d9",
      "sha256_skipped": false
    }
  ],
  "hosts": [
    {
      "host": "DESKTOP-OA8SHHC",
      "output_id": "DESKTOP-OA8SHHC",
      "os": "Microsoft Windows 11 Enterprise 23H2",
      "collection": "Collection-DESKTOP-OA8SHHC-2026-03-12T22_54_56Z",
      "source_archive": "Collection-DESKTOP-OA8SHHC.zip",
      "inaccessible_entries": 0,
      "tools": [
        {
          "tool": "PETriage",
          "key": "pe",
          "time_filter": null,
          "files_matched": 312,
          "discovered_candidates": 312,
          "supported": 312,
          "unsupported": 0,
          "corrupt": 0,
          "unreadable": 0,
          "parsed": 312,
          "failed": 0,
          "deduplicated": 0,
          "records": 1155,
          "output_paths": [
            "out/Processed-DESKTOP-OA8SHHC-2026-09-13T120000Z/FileSystem/2026-09-13T120000Z_PETriage_results.csv",
            "out/Processed-DESKTOP-OA8SHHC-2026-09-13T120000Z/FileSystem/2026-09-13T120000Z_PETriage_results_Timeline.csv"
          ],
          "reason_samples": []
        },
        {
          "tool": "EvtxTriage",
          "key": "evtx",
          "time_filter": null,
          "files_matched": 159,
          "discovered_candidates": 159,
          "supported": 159,
          "unsupported": 0,
          "corrupt": 0,
          "unreadable": 0,
          "parsed": 116,
          "failed": 0,
          "deduplicated": 43,
          "records": 149640,
          "output_paths": [
            "out/Processed-DESKTOP-OA8SHHC-2026-09-13T120000Z/EventLogs/2026-09-13T120000Z_EvtxTriage_results.csv",
            "out/Processed-DESKTOP-OA8SHHC-2026-09-13T120000Z/EventLogs/Individual/Application.csv",
            ...
          ],
          "reason_samples": []
        },
        ...
      ],
      "external_tools": [
        {
          "tool": "hayabusa-csv",
          "found": true,
          "invoked": true,
          "exit_code": 0,
          "output_paths": [
            "<workdir>/out/Processed-DESKTOP-OA8SHHC-2026-09-13T120000Z/EventLogs/timeline.csv"
          ]
        },
        {
          "tool": "takajo-automagic",
          "found": true,
          "invoked": true,
          "exit_code": 0,
          "output_paths": [
            "<workdir>/out/Processed-DESKTOP-OA8SHHC-2026-09-13T120000Z/ThreatHunting"
          ]
        },
        ...
      ]
    },
    ...
  ]
}
```

**`capture_type` has exactly three values** (`CaptureType` in
`crates/triage-orchestrator/src/capture.rs`, serialized lowercase):

| Value | The run it describes |
|---|---|
| `"velociraptor"` | at least one Velociraptor collection was found -- the `uploads.json` + `client_info.json` pair, in the input directly or inside an archive |
| `"raw"` | no collection was found, and the input directory was processed as a single raw mounted tree. The one host entry is named after that directory and its `os` is `"unknown"`, since there is no `client_info.json` to read it from. Never reached from a folder of `.zip`s: a folder holding only archives must not be mistaken for a raw capture named after the folder |
| `"unidentified"` | nothing was identified at all, because the input was rejected -- see "A rejected input still gets a manifest" below. Only ever appears alongside `hosts: []` and `final_exit_status: 3` |

**Absent fields are absent, not null.** Several fields are omitted entirely rather than
serialized as `null` or `[]`, so their presence in a manifest is itself information -- the
example above shows each of them both ways round:

| Field | Omitted when |
|---|---|
| `archives` | the array would be empty -- omitted entirely, never `[]`. Usually that means directory input with no archives; but a directory-input run that *skipped* a collection still gets the array, holding that skip |
| `archives[].extracted_to` | nothing was extracted from that input (`notes.zip` above) |
| `archives[].skipped_reasons` | no individual entry inside the archive was skipped |
| `archives[].error` | that input reached no error (`Collection-DESKTOP-OA8SHHC.zip` above) |
| `hosts[].source_archive` | the host came from a directory rather than an archive |
| `hosts[].output_errors` | that collection's output-compat writes all succeeded -- omitted entirely, never `[]` (see below) |
| `hosts[].tools[].error` | the tool did not abort |
| `hosts[].external_tools[].error` | the external tool did not fail |

`sha256` is the deliberate exception: it stays in the record as an explicit `null`, because in
a chain-of-custody report "we did not hash this" and "there is no hash field" are different
claims. `sha256_skipped` says which kind of `null` it is.

**`output_paths` are paths, not tree-relative keys.** An in-process tool's entries are built
from whatever `--out` was given, so they are relative exactly when `--out` was relative (`out/...`
above, from `--out out`). External-tool entries are always absolute: they are derived from the
host's resolved collection directory, which is where the child process was pointed. Neither is
relative to the output root -- do not assume a common prefix when reading them back
programmatically.

`time_filter` is `null` when the run had no `--start`/`--end` at all, `"applied"` for a tool
that actually honors the range (only `EvtxTriage` today), and `"not_applicable"` for every
other tool on a filtered run -- deliberately distinct from `null` so a reader can tell "no
range this run" apart from "this tool has no filter" (see "Time-range filtering" above). The
run above passed no range, so every entry shows `null`.

Each tool entry reports:
- `files_matched`: Number of files selected by the tool's patterns and validation.
- `parsed`: Number of files successfully parsed.
- `failed`: Number of files that failed parsing.
- `records`: Total number of records output (rows in CSV or lines in NDJSON).
- `output_paths`: The destinations `OutputRouter` actually **published** for this tool
  (`FinishReport::published` -- one entry per staged file whose rename onto its final
  destination succeeded), followed by the merged category-root files the merge post-pass then
  wrote. `PerUser/` files and merged files alike. Published, not merely opened, and not merely
  present on disk: a tool whose router aborted after a write failure publishes nothing, and the
  merge then has no sources, so its `output_paths` is `[]` even though a previous run's files
  may be sitting at every one of those paths. Reproduced on a real capture (`STDC1`,
  `--only le`, pinned stamp) by planting a regular file where `FileSystem/PerUser/` had to be
  created: `records: 0`, `output_paths: []`, the abort in `error`, and the run exits `4`.
  Two consequences worth knowing before reading this array back programmatically. For a
  `Scope::UserElseSystem` tool that produced both system-scope and per-user output, the
  category-root path appears **twice** -- once as the published system-scope slice, once as the
  merged file that replaced it. And the reclaimed system-scope slice is listed under the
  category-root path it was published to, not under the `PerUser/` name the merge then renamed
  it to (see "PerUser reclaim" above): that filename is on disk but in no manifest.
- `time_filter`: `null` (no range given), `"applied"`, or `"not_applicable"`.
- `error`: Omitted unless the tool execution aborted; contains the error message.
- `sha256` / `sha256_skipped` (archive entries only): the source archive's hash, and whether
  `null` there means "hashing was skipped" (`--skip-hashes`) rather than "hashing failed".
  `sha256` is also `null` whenever the recorded path is not a regular file — a directory, or
  the FIFO or device node a rejected run refused — which is never hashed at all.
- `size_bytes` (archive entries only): `null`, not a false `0`, when the archive couldn't be
  stat'ed.

Each `external_tools` entry reports (see the External tools section above for full field
semantics): `tool`, `found`, `invoked`, `exit_code`, `output_paths`, `error`.

Each host entry also carries `output_errors`: the failures, if any, in that collection's
output-compat writes — the VeloResults copy, the source hash log, the SysInfo report, the
Timeline Explorer sessions, the output hash walk. It is **omitted entirely** on a healthy
run, so existing manifests are unchanged. It exists because those writes no longer abort the
process (see exit code `4` below): with the run continuing, `final_exit_status: 4` alone
would say that *a* collection's output is incomplete without saying which, and console output
is not a chain-of-custody record.

Copied from a real run whose `CaseInfo/<stamp>_OutputHashes.txt` had been replaced by a
directory of the same name (`<out>` stands in for the absolute output root):

```json
"output_errors": [
  "cannot write output hashes: output failure at <out>/Processed-GOODHOST-2026-03-13T192553Z/CaseInfo/2026-03-13T192553Z_OutputHashes.txt: Is a directory (os error 21)"
]
```

Exit codes are: `0` success (including unsupported-only discovery), `2` usage,
`3` missing input, `4` output/manifest failure, `5` mixed artifact success and failure,
and `6` when applicable artifacts existed but all failed.

### A rejected input still gets a manifest

An input that produced no host at all — a path that does not exist, a double-zipped archive,
a folder whose archives were every one of them unusable — exits `3` **and still writes
`run_manifest.json`**. A rejected run is still a run: exit `3` on a terminal is not a
chain-of-custody record, and with a reused `--out` the alternative is worse than nothing,
because the file left on disk would be the *previous* run's successful manifest — a success
record for a run that never happened.

The rejection is described entirely in fields the manifest already has. `hosts` is empty
because nothing was processed, `final_exit_status` is `3`, and `archives[]` carries one
entry per refused input with the reason in its `error`. The path you pointed `run` at gets
its own entry holding the run-level reason, unless it *is* one of the archives already
listed (a lone `.zip` input is both), in which case that entry keeps its own, more specific
reason. `capture_type` is `"unidentified"`: nothing was identified, and claiming
`"velociraptor"` would assert an identification that never happened. That value appears only
here, alongside an empty `hosts[]` and `final_exit_status: 3`.

Copied verbatim from a run over a folder (`bad/`) holding two files that are not archives at
all. The only edit is that `<input-folder>` stands in for the absolute path `run` was pointed
at:

```json
{
  "schema_version": 3,
  "run_id": "20260913232350096",
  "orchestrator_version": "0.2.0",
  "started_utc": "2026-09-13T23:23:50.0967150Z",
  "finished_utc": "2026-09-13T23:23:50.0988860Z",
  "capture_type": "unidentified",
  "final_exit_status": 3,
  "archives": [
    {
      "archive": "a.zip",
      "archive_path": "<input-folder>/a.zip",
      "size_bytes": 4,
      "status": "skipped",
      "files_written": 0,
      "bytes_written": 0,
      "skipped_entries": 0,
      "error": "not a valid zip archive (invalid Zip archive: Could not find EOCD)",
      "sha256": "ca3704aa0b06f5954c79ee837faa152d84d6b2d42838f0637a15eda8337dbdce",
      "sha256_skipped": false
    },
    {
      "archive": "b.zip",
      "archive_path": "<input-folder>/b.zip",
      "size_bytes": 9,
      "status": "skipped",
      "files_written": 0,
      "bytes_written": 0,
      "skipped_entries": 0,
      "error": "not a valid zip archive (invalid Zip archive: Could not find EOCD)",
      "sha256": "555dc6d96de93dc75049c0bb1bf93ea0b61d7266433dbd926942d1acac1aa470",
      "sha256_skipped": false
    },
    {
      "archive": "bad",
      "archive_path": "<input-folder>",
      "size_bytes": null,
      "status": "skipped",
      "files_written": 0,
      "bytes_written": 0,
      "skipped_entries": 0,
      "error": "no usable capture found in <input-folder> (2 archive(s) skipped)",
      "sha256": null,
      "sha256_skipped": false
    }
  ],
  "hosts": []
}
```

The same rule holds for a failure partway through a run: see exit code `4` below, where one
collection's output-compat write failing no longer aborts the process before the manifest is
written.

## Exit codes

- `0` success (including unsupported-only discovery)
- `2` usage (invalid flags, unknown `--only`/`--skip` key, malformed `--config` TOML, unknown
  `--profile` name, or the `takajo.enabled` / `hayabusa.json` validation conflict)
- `3` missing input (capture path not found or not detectable, including a `.zip` input where
  no archive yielded a usable capture and no other collection was present). A `run_manifest.json`
  recording the rejection is still written — see "A rejected input still gets a manifest" above
- `4` output/manifest failure. A failure in one collection's output-compat writes (the
  VeloResults copy, the source hash log, the SysInfo report, the Timeline Explorer sessions,
  the output hash walk) prints its error, makes the run's status `4`, and lets the remaining
  hosts finish — the run still reaches the manifest write, so `run_manifest.json` describes
  *this* run rather than being left as the previous one's, and the failing host's
  `output_errors` names what went wrong for that collection
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
