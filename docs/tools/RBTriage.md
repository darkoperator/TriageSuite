# RBTriage

RBTriage parses Windows Recycle Bin artifacts: `$I` metadata records in both the v1
format (pre-Windows 10) and the v2 format (Windows 10+), plus legacy `INFO2` files
from older Windows versions. It produces one output record per deleted file,
adding JSON output that RBCmd does not provide.

## Target Windows versions

- **v1 `$I` format**: Windows XP through Windows 8.1 (pre-Windows 10)
- **v2 `$I` format**: Windows 10 and Windows 11
- **Legacy `INFO2`**: Windows 95 through Windows XP/2003

## Compatibility

Output is column-compatible with Eric Zimmerman's RBCmd (same field names, same
intentional ISO 8601 UTC timestamp difference vs RBCmd's local-time strings). Adds
JSON output that RBCmd does not provide.

## Flags

Input (exactly one required):
```
-d, --directory <DIR>         Recursively discover Recycle Bin artifacts under this directory
-f, --file <FILE>             Explicit $I or INFO2 file (repeatable)
```

Output (at least one required):
```
--csv <DIR>                   Write CSV output beneath this directory
--json <DIR>                  Write NDJSON output beneath this directory
--csvf <NAME>                 Override the default CSV basename
--jsonf <NAME>                Override the default JSON basename
--pretty                      Pretty-print JSON (no effect on NDJSON-framed output)
--overwrite                   Replace existing output files
```

Diagnostics:
```
-q, --quiet                   Suppress per-file informational messages
--debug                       Emit debug-level diagnostics to stderr
--trace                       Emit trace-level diagnostics to stderr (implies --debug)
```

RBTriage has no tool-specific flags beyond the common set above.

## Output layout

```
<out>/
  RBTriage/
    users/
      <SID>/
        RBTriage_Output.csv     # one row per deleted file
        RBTriage_Output.json    # same records as NDJSON
```

Identity is the Recycle Bin SID found in the `$I` file path. Resolving the SID to a
username via a captured SOFTWARE hive's ProfileList is planned for a later milestone.

## Output fields

Confirmed against the `RecycleRecord` struct in `crates/rb-triage/src/lib.rs`. All
five fields are serialized with PascalCase names via `#[serde(rename = ...)]`; no
additional fields exist in the record.

| Field | Type | Notes |
|---|---|---|
| `SourceName` | string | Full path to the source `$I` or `INFO2` file |
| `FileType` | string | `"$I"` or `"INFO2"`, identifying which artifact format produced the record |
| `FileName` | string | Original path/name of the deleted file, as recorded by the Recycle Bin |
| `FileSize` | integer (i64) | Size of the deleted file in bytes |
| `DeletedOn` | timestamp | Deletion time, rendered as ISO 8601 UTC (RBTriage's intentional timestamp-format difference from RBCmd's local-time strings) |

## Examples

```
RBTriage -d /mnt/triage --csv ./out --json ./out
RBTriage -f "/mnt/triage/C/\$Recycle.Bin/S-1-5-21-1234/\$IABC123.docx" --csv ./out
RBTriage -d /mnt/triage --csv ./out -q
```

## Known limitations

- **v1 `$R` companion directories not expanded**: RBCmd additionally expands a
  deleted folder's `$R` companion directory into extra `DirectoryFiles` rows.
  RBTriage does not yet do this — only the `$I` metadata record is emitted for
  folder entries. This is deliberate: v1 deleted-folder expansion is
  filesystem-coupled behavior (reaching into sibling `$R` trees) that is
  designed for a later milestone rather than implemented now.
- **SID not resolved to username**: Identity is reported as the raw Recycle Bin
  SID found in the artifact path. Resolving the SID to a username via a
  captured SOFTWARE hive's ProfileList is planned for a later milestone.
