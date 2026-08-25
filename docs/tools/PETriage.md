# PETriage

Windows Prefetch parser. Produces two datasets matching PECmd's output: a main record
per prefetch file and a timeline of loaded resources. Prefetch versions 17–31
(XP through Windows 11) are supported, including MAM-compressed files.

## Target Windows versions

Prefetch format version is read from the file header and maps to a Windows release as
follows:

| Version | Windows release |
|---|---|
| 17 | Windows XP or Windows Server 2003 |
| 23 | Windows Vista or Windows 7 |
| 26 | Windows 8.0, Windows 8.1, or Windows Server 2012(R2) |
| 30 | Windows 10 or Windows 11 |
| 31 | Windows 11 |

Any other value falls back to printing the raw numeric version. This mapping is a
straight decimal-to-decimal correspondence (format version numbers are not tied to a
specific OS build in a way that changed since Windows 8), so there is no ambiguity in
listing it as a fixed table.

## Compatibility

Output is column-compatible with Eric Zimmerman's PECmd (same field names and order,
verified against a PECmd fixture header). The one intentional difference: timestamps
are rendered as ISO 8601 UTC strings (`YYYY-MM-DDTHH:MM:SS.fffffffZ`, full
100-nanosecond FILETIME precision) rather than PECmd's local-time strings.

## Flags

Input (exactly one required):
```
-d, --directory <DIR>         Recursively discover prefetch files under this directory
-f, --file <FILE>             Explicit prefetch file (repeatable)
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

PETriage options:
```
-k, --keywords <kw,...>       Additional comma-separated keywords to flag in loaded-files
                               (temp and tmp are always included; PECmd semantics)
--dedupe <true|false>         Deduplicate identical prefetch files by SHA-1 (default: false)
```

Diagnostics:
```
-q, --quiet                   Suppress per-file informational messages
--debug                       Emit debug-level diagnostics to stderr
--trace                       Emit trace-level diagnostics to stderr (implies --debug)
```

## Output layout

```
<out>/
  PETriage/
    system/
      PETriage_Output.csv          # one row per prefetch file
      PETriage_Output_Timeline.csv # one row per loaded resource
```

User-attributed artifacts are placed under `PETriage/users/<username>/` instead of
`PETriage/system/`. The output directory is excluded from discovery when it falls inside
the input directory.

## Output fields

### Main record (`PETriage_Output`, CSV and JSON)

One row per prefetch file, 27 columns:

- `Note` — set only when the file references more than two volumes; otherwise omitted
  from JSON (null in CsvOut terms).
- `SourceFilename` — path to the prefetch file that was parsed.
- `SourceCreated` — creation time of the prefetch file itself (filesystem metadata).
- `SourceModified` — modification time of the prefetch file itself.
- `SourceAccessed` — access time of the prefetch file itself.
- `ExecutableName` — executable name recorded in the prefetch header.
- `Hash` — header hash, rendered as uppercase hex without zero-padding.
- `Size` — decompressed prefetch payload size from the header (not the on-disk
  MAM-compressed size).
- `Version` — human-readable OS description derived from the format version (see the
  version table above).
- `RunCount` — number of recorded executions.
- `LastRun` — timestamp of the most recent run; empty string if the file records no run
  times.
- `PreviousRun0` through `PreviousRun6` — up to seven earlier run timestamps, oldest
  fields omitted from JSON when the file records fewer runs.
- `Volume0Name` — device path of the first referenced volume; empty string if none.
- `Volume0Serial` — volume serial number, uppercase hex without zero-padding.
- `Volume0Created` — volume creation time; blanked to empty when the year is 1601
  (uninitialized FILETIME), matching PECmd's blanking rule.
- `Volume1Name` — device path of the second referenced volume; omitted from JSON when
  there are fewer than two volumes (volume 1's creation time is not year-1601 blanked).
- `Volume1Serial` — second volume's serial number.
- `Volume1Created` — second volume's creation time.
- `Directories` — directories referenced by all volumes, comma-space joined per volume
  and concatenated across volumes with no separator between volumes (a PECmd quirk
  reproduced exactly).
- `FilesLoaded` — comma-space joined list of every resource path referenced by the
  prefetch file.
- `ParsingError` — `"True"`/`"False"` string; always `"False"` in practice, since a parse
  failure aborts the record instead of emitting a partial one.

### Timeline record (`PETriage_Output_Timeline`, CSV only)

One row per recorded run time (i.e., up to `RunCount` rows per prefetch file), 2
columns:

- `RunTime` — a single run timestamp from the prefetch file's run-time array.
- `ExecutableName` — the volume-qualified path of the executable (the files-loaded entry
  ending with the header executable name), falling back to the bare executable name when
  no loaded-file entry matches.

## Examples

```
PETriage -f /mnt/triage/C/Windows/Prefetch/POWERSHELL.EXE-ABC12345.pf --csv ./out
PETriage -d /mnt/triage --csv ./out
PETriage -d /mnt/triage --csv ./out --json ./out -k appdata
PETriage -d /mnt/triage --json ./out --dedupe true
```
