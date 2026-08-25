# LolTriage

Cross-references output already produced by `AmcacheTriage`, `AppCompatTriage`,
`MFTriage`, `PETriage`, and `RBTriage` against two checked-in reference snapshots:
[LOLDrivers](https://loldrivers.io) (vulnerable/malicious signed drivers, hash- and
filename-matched) and [LOLRMM](https://lolrmm.io) (abused remote-management tools,
filename-matched). Unlike every other tool in the suite, LolTriage's input is other
tools' CSV output, not raw forensic artifacts — run it as a second pass after a
`TriageSuite run` (or after running the individual tools it consumes) has produced
output.

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
--nested-output               Preserve the legacy nested output layout under <root>/LolTriage/<identity>/
```

LolTriage options:
```
--refs <DIR>                  Scan mode: load loldrivers_refs.json/lolrmm_refs.json from
                               this directory instead of the copies embedded in the binary.
                               --update-refs mode: write the freshly fetched snapshots here
                               instead of this source checkout's refdata/ directory.
--update-refs                 Fetch the latest LOLDrivers/LOLRMM data, reduce it, and write
                               loldrivers_refs.json/lolrmm_refs.json, then exit. Requires
                               network access. Cannot be combined with -d/-f/--csv/--json.
```

The reference snapshots are compiled into the `LolTriage` binary at build time, so the
tool works with no `--refs` flag on any host you copy it to. Pass `--refs <DIR>` in a
normal scan only to override that with different or more recent reference data on disk;
the directory must contain both `loldrivers_refs.json` and `lolrmm_refs.json` in the
reduced schema described below. See [Refreshing the reference
data](#refreshing-the-reference-data) for `--update-refs`.

Diagnostics:
```
-q, --quiet                   Suppress per-file informational messages
--debug                       Emit debug-level diagnostics to stderr
--trace                       Emit trace-level diagnostics to stderr (implies --debug)
```

## Output layout

TriageSuite's default output layout is **flat**: every dataset file is written directly
under the `--csv`/`--json` root, with a 14-digit `<yyyyMMddHHmmss>_` run-stamp prefix and
the record's identity folded into the filename. LolTriage's `scope()` is `SystemWide`
(findings are host-level, not per-user), so the identity label is `system`. Inferring the
convention from `crates/triage-core/src/output/layout.rs` and the single dataset basename
defined in `crates/lol-triage/src/lib.rs` (`LolTriage_Output`), a default run produces:

```
<out>/
  system_<yyyyMMddHHmmss>_LolTriage_Output.csv
  system_<yyyyMMddHHmmss>_LolTriage_Output.json   # NDJSON, if --json used
```

Pass `--nested-output` to instead get the legacy tree layout:

```
<out>/
  LolTriage/
    system/
      LolTriage_Output.csv
      LolTriage_Output.json
```

`--csvf`/`--jsonf` override the basename portion only; the run-stamp and identity folding
still apply in flat mode. All findings from every input CSV file are written to a single
output file (in flat mode) or dataset directory (in nested mode).

## What it reads

A `.csv` file is accepted only if its header line matches one of six known exact
schemas (content-gated, never by filename): Amcache `FileEntries`, Amcache
`DriveBinaries`, AppCompatCache, `$MFT`, Prefetch, Recycle Bin. Anything else under `-d`
is silently skipped.

AmcacheTriage writes its Associated and Unassociated file entries through the identical
header and schema, so LolTriage cannot tell which of the two files a row came from; both
are reported as `SourceDataset` = `FileEntries`.

## Output fields

| Field | Notes |
|---|---|
| `SourceTool` | Which TriageSuite tool produced the matched row |
| `SourceDataset` | Which dataset within that tool |
| `SourceFile` | Path to the CSV LolTriage read the row from |
| `EvidencePath` | The path/filename value from the source row |
| `EvidenceSha1` | Empty unless the source row carried a hash (Amcache only) |
| `EvidenceTimestamp` | Best-effort per-source timestamp |
| `MatchType` | `Hash` or `Filename` |
| `ReferenceList` | `LOLDrivers` or `LOLRMM` |
| `ReferenceName` | Matched entry's driver filename tag / RMM tool name |
| `ReferenceCategory` | e.g. `vulnerable driver`, `RAT` |
| `MitreAttackId` | LOLDrivers' MITRE ATT&CK technique ID; empty for LOLRMM |
| `Confidence` | `High` (hash match) or `Medium` (filename-only match) |

## Examples

Cross-reference output from a prior `TriageSuite run`, emitting CSV only:

```bash
LolTriage -d /path/to/prior-triagesuite-output --csv ./out
```

Cross-reference output from a prior `TriageSuite run`, emitting both CSV and NDJSON with
custom reference data:

```bash
LolTriage -d /path/to/prior-triagesuite-output --csv ./out --json ./out --refs ./custom-refdata
```

Cross-reference specific CSV files, emitting CSV with quiet mode:

```bash
LolTriage -f ./AmcacheTriage_UnassociatedFileEntries_Output.csv -f ./AppCompatTriage_AppCompatCache_Output.csv --csv ./out -q
```

## Refreshing the reference data

```bash
LolTriage --update-refs
```

Fetches `loldrivers.io`/`lolrmm.io`, reduces each to the schema above, and overwrites
`crates/lol-triage/refdata/loldrivers_refs.json`/`lolrmm_refs.json` in this source
checkout (the same files `include_str!`'d into the binary at build time). Run it
manually from a checkout with network access and commit the result — it is never
invoked automatically, and rebuilding `LolTriage` afterward is required to pick up the
refreshed data into the embedded default.

Pass `--refs <DIR>` alongside `--update-refs` to write elsewhere instead of this source
checkout (e.g. to inspect the fetched data before deciding whether to commit it):

```bash
LolTriage --update-refs --refs /tmp/lol-refs-preview
```

## Known limitations

- Filename matching is basename-only (no directory-path or glob matching), so it is
  a `Medium`-confidence signal, not a hard positive.
- LOLRMM's code-signing SHA-256 hashes are vendored but never compared: none of the
  five source datasets record a SHA-256, only Amcache's SHA-1.
- Not wired into `TriageSuite run` automatically — run it as an explicit second pass.
