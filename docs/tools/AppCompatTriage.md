# AppCompatTriage

The Application Compatibility Cache (ShimCache) is a Windows mechanism that tracks
metadata — path, last-modified time, and (on newer Windows versions) whether a program
was executed — for binaries the OS has evaluated for compatibility shimming. The data is
stored as a single opaque binary blob inside a registry value named `AppCompatCache`,
nested under each control set of the `SYSTEM` hive. AppCompatTriage opens a `SYSTEM` hive,
locates that blob in every present control set, decodes its internal binary record
structure, and emits one row per cache entry as AppCompatCacheParser-compatible CSV and
NDJSON.

## Target Windows versions

AppCompatTriage's decoder (`triage-appcompat`, shared with RETriage) implements only the
**Windows 10/11 ShimCache binary layout**, identified internally by its per-entry `"10ts"`
signature (module doc: *"Windows ShimCache (AppCompatCache) Windows 10/11 ('10ts') binary
parser"*). Each entry is `sig(4) + unknown(4) + data_size(4) + path_size(2) + path(UTF-16LE)
+ FILETIME(8) + entry_data_size(4) + entry_data`, with the executed flag read from the last
4 bytes of the entry's data block.

The much older XP `0xdeadbeef`/`0xbadc0ffe`-family layouts, the Vista/7 layout, and the
Windows 8/8.1 layout are **not** implemented — there is no signature-detection branch,
version dispatcher, or format-version enum in the source for any format other than
`"10ts"`. If a blob's entry stream does not begin with the `10ts` signature at its parsed
entry offset, parsing simply stops and yields zero entries for that control set. State
this precisely: confirmed coverage is Windows 10/11 only, not "all ShimCache versions."

## Compatibility

Output is compatible with Eric Zimmerman's AppCompatCacheParser: identical 7-column header
(`ControlSet,CacheEntryPosition,Path,LastModifiedTimeUTC,Executed,Duplicate,SourceFile`),
`\??\` prefix stripping on paths, timestamp nulling for zero/epoch-1601 FILETIMEs, and
duplicate detection using the same key (FILETIME + uppercased path) tracked in emission
order across all control sets — mirroring AppCompatCacheParser's shared cache-key set. A
dedicated compatibility test suite (`crates/acc-triage/tests/compat_appcompatcacheparser.rs`)
diffs AppCompatTriage's output row-for-row against AppCompatCacheParser CSV fixtures.

RETriage also ships a generic `AppCompatCache` entry in its registry-plugin table for the
`SYSTEM` hive. That is expected and not a duplication to worry about: RETriage's plugin
surfaces the raw registry value the way any other registry-plugin row would, while
AppCompatTriage is a dedicated parser that understands the binary blob's internal
`10ts`-entry structure and reconstructs individual cache records from it. Both read the
same underlying value from different angles.

## Flags

Input (exactly one required):
```
-d, --directory <DIR>         Recursively discover SYSTEM hives under this directory
-f, --file <FILE>             Explicit SYSTEM hive file (repeatable)
```

Output (at least one required):
```
--csv <DIR>                   Write AppCompatCacheParser-compatible CSV output beneath this directory
--json <DIR>                  Write NDJSON output beneath this directory
--csvf <NAME>                 Override the default CSV basename
--jsonf <NAME>                Override the default JSON basename
--pretty                      Pretty-print JSON (no effect on NDJSON-framed output)
--overwrite                   Replace existing output files
--nested-output               Preserve the legacy nested output layout under <root>/AppCompatTriage/<identity>/
```

AppCompatTriage options:
```
--nl                          Skip pairing the SYSTEM hive with its .LOG1/.LOG2 transaction-log siblings
```

Diagnostics:
```
-q, --quiet                   Suppress per-file informational messages
--debug                       Emit debug-level diagnostics to stderr
--trace                       Emit trace-level diagnostics to stderr (implies --debug)
```

Input discovery validates content, not just filename: in directory mode the filename glob
is `SYSTEM`, but a candidate is only accepted if it is not a `.LOG`/`.LOG1`/`.LOG2` sibling
and its first 4 bytes match the `regf` hive magic. Files that match the glob but fail the
magic check are treated as corrupt (`invalid_content_is_corrupt` is set), not silently
skipped.

## Output layout

TriageSuite's default output layout is **flat**: every dataset file is written directly
under the `--csv`/`--json` root, with a 14-digit `<yyyyMMddHHmmss>_` run-stamp prefix and
the record's identity folded into the filename. AppCompatTriage's `scope()` is
`SystemWide` (AppCompatCache is host-level data keyed to the SYSTEM hive, not a per-user
artifact), so the identity label is `system`. Inferring the convention from
`crates/triage-core/src/output/layout.rs` and the single dataset basename defined in
`crates/acc-triage/src/lib.rs` (`AppCompatTriage_AppCompatCache_Output`), a default run
produces:

```
<out>/
  system_<yyyyMMddHHmmss>_AppCompatTriage_AppCompatCache_Output.csv
  system_<yyyyMMddHHmmss>_AppCompatTriage_AppCompatCache_Output.json   # NDJSON, if --json used
```

Pass `--nested-output` to instead get the legacy tree layout:

```
<out>/
  AppCompatTriage/
    system/
      AppCompatTriage_AppCompatCache_Output.csv
      AppCompatTriage_AppCompatCache_Output.json
```

`--csvf`/`--jsonf` override the basename portion only; the run-stamp and identity folding
still apply in flat mode. All rows from every present `ControlSet000`..`ControlSet009` in
one SYSTEM hive are written to a single output file for that hive, ordered by control set
then by cache position within the control set.

## Output fields

Confirmed against the `AppCompatRecord` struct in `crates/acc-triage/src/record.rs`. Field
order is the CSV column order; all 7 fields are strings (empty string = absent), each
serialized with a PascalCase `#[serde(rename = ...)]`.

| Field | Notes |
|---|---|
| `ControlSet` | Control set number the entry was read from (e.g. `"1"` for `ControlSet001`), not necessarily the active `CurrentControlSet` — every present `ControlSet000`..`ControlSet009` is parsed |
| `CacheEntryPosition` | 0-based sequential position of the entry within its control set's cache blob |
| `Path` | Program path as stored in the cache, with a leading `\??\` device prefix stripped (matches AppCompatCacheParser) |
| `LastModifiedTimeUTC` | ISO 8601 UTC timestamp decoded from the entry's FILETIME; empty when the FILETIME is 0 or decodes to year 1601 (nulled, matching AppCompatCacheParser) |
| `Executed` | `"Yes"`/`"No"` — read from the last 4 bytes of the entry's data block (`== 1` as a little-endian i32 means executed) |
| `Duplicate` | `"True"`/`"False"` — `"True"` when the entry's key (FILETIME, or 0 if the timestamp was nulled, concatenated with the uppercased path) was already seen earlier in emission order, across all control sets |
| `SourceFile` | Full path to the SYSTEM hive the entry was read from |

## Examples

Parse a SYSTEM hive found by directory scan, emitting CSV only:

```bash
AppCompatTriage -d /mnt/triage --csv ./out
```

Parse an explicit SYSTEM hive, emitting both CSV and NDJSON:

```bash
AppCompatTriage -f "/mnt/triage/C/Windows/System32/config/SYSTEM" --csv ./out --json ./out
```

Parse a SYSTEM hive that has no `.LOG1`/`.LOG2` transaction-log siblings available:

```bash
AppCompatTriage -f "/mnt/triage/C/Windows/System32/config/SYSTEM" --csv ./out --nl
```

Recursively triage a full Velociraptor capture, pretty-printed JSON, allowing overwrite of
a prior run's output:

```bash
AppCompatTriage -d /mnt/triage --json ./out --pretty --overwrite
```
