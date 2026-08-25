# JLETriage

Windows Jump List parser. Handles both Jump List types: **Automatic Destinations**
(`.automaticDestinations-ms`) — an OLE compound file containing a DestList stream of MRU
metadata entries plus numbered embedded-LNK streams — and **Custom Destinations**
(`.customDestinations-ms`) — a flat sequence of categorised embedded LNKs. The shell-items
and LNK engines are reused to parse all embedded LNK fields.

## Target Windows versions

Jump Lists were introduced in Windows 7. JLETriage applies to Windows 7 through Windows 11
and Windows Server 2008 R2 and later — any Windows version that maintains
`AutomaticDestinations` and `CustomDestinations` jump list files under
`AppData\Roaming\Microsoft\Windows\Recent\`.

## Compatibility

Output is column-compatible with Eric Zimmerman's JLECmd: `AutoCsvOut` (44 columns) and
`CustomCsvOut` (28 columns) with the same names and order, and the same intentional ISO 8601
UTC timestamp difference. JLETriage emits flattened NDJSON (one record per line) — the suite
convention — rather than JLECmd's nested per-file JSON dump.

One documented difference: JLECmd truncates embedded-LNK arguments longer than 260 characters
and omits those LNKs' extra blocks; JLETriage parses them fully.

## Flags

Input (exactly one required):

```
-d, --directory <DIR>         Recursively discover Jump List files under this directory
-f, --file <FILE>             Explicit Jump List file (repeatable)
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

JLETriage options:

```
--all                         Examine every file as a Jump List candidate (content
                               validation still applies; non-Jump-List files are skipped
                               with a warning)
--ld                          Extended embedded-LNK detail in console output
                               (does not affect CSV/JSON)
--fd                          Full embedded-LNK detail in console output
                               (does not affect CSV/JSON; implies --ld)
--withDir                     Include compound-file streams not referenced by the DestList
--appIds <FILE>                Load additional appid|description mappings; extends the
                               bundled AppId table
--cp <CODE_PAGE>               Code page for legacy (non-Unicode) string fields
                               (default: 1252)
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
  JLETriage/
    users/
      <username>/
        JLETriage_AutomaticDestinations_Output.csv   # one row per DestList entry
        JLETriage_AutomaticDestinations_Output.json
        JLETriage_CustomDestinations_Output.csv      # one row per embedded LNK
        JLETriage_CustomDestinations_Output.json
    system/
      JLETriage_AutomaticDestinations_Output.csv
      JLETriage_AutomaticDestinations_Output.json
      JLETriage_CustomDestinations_Output.csv
      JLETriage_CustomDestinations_Output.json
```

Jump Lists whose path contains a recognisable user-profile segment are routed to
`JLETriage/users/<username>/`; all others go to `JLETriage/system/`. AppId values are
resolved to descriptions via a bundled table; the `--appIds` flag extends that table with
site-specific or updated mappings.

## Output fields

### AutomaticDestinations (44 columns)

One row per DestList entry (MRU metadata plus its embedded-LNK columns, left empty when the
entry has no embedded LNK).

| # | Field | Notes |
|---|-------|-------|
| 1 | `SourceFile` | |
| 2 | `SourceCreated` | Filesystem timestamp of the source `.automaticDestinations-ms` file |
| 3 | `SourceModified` | |
| 4 | `SourceAccessed` | |
| 5 | `AppId` | |
| 6 | `AppIdDescription` | Resolved via the bundled/`--appIds`-extended AppId table |
| 7 | `HasSps` | `.NET`-style bool, rendered `"True"`/`"False"` |
| 8 | `DestListVersion` | |
| 9 | `LastUsedEntryNumber` | |
| 10 | `MRU` | |
| 11 | `EntryNumber` | |
| 12 | `CreationTime` | DestList entry creation time |
| 13 | `LastModified` | DestList entry last-modified time |
| 14 | `Hostname` | |
| 15 | `MacAddress` | All-zero MAC (`00:00:00:00:00:00`) renders as empty |
| 16 | `Path` | |
| 17 | `InteractionCount` | |
| 18 | `PinStatus` | `.NET`-style bool, rendered `"True"`/`"False"` |
| 19 | `FileBirthDroid` | All-zero GUID renders as empty |
| 20 | `FileDroid` | All-zero GUID renders as empty |
| 21 | `VolumeBirthDroid` | All-zero GUID renders as empty |
| 22 | `VolumeDroid` | All-zero GUID renders as empty |
| 23 | `TargetCreated` | From embedded LNK |
| 24 | `TargetModified` | From embedded LNK |
| 25 | `TargetAccessed` | From embedded LNK |
| 26 | `FileSize` | From embedded LNK |
| 27 | `RelativePath` | From embedded LNK |
| 28 | `WorkingDirectory` | From embedded LNK |
| 29 | `FileAttributes` | From embedded LNK |
| 30 | `HeaderFlags` | From embedded LNK |
| 31 | `DriveType` | From embedded LNK |
| 32 | `VolumeSerialNumber` | From embedded LNK |
| 33 | `VolumeLabel` | From embedded LNK |
| 34 | `LocalPath` | From embedded LNK |
| 35 | `CommonPath` | From embedded LNK |
| 36 | `TargetIDAbsolutePath` | From embedded LNK shell items |
| 37 | `TargetMFTEntryNumber` | From embedded LNK shell items |
| 38 | `TargetMFTSequenceNumber` | From embedded LNK shell items |
| 39 | `MachineID` | From embedded LNK tracker block |
| 40 | `MachineMACAddress` | From embedded LNK tracker block |
| 41 | `TrackerCreatedOn` | From embedded LNK tracker block |
| 42 | `ExtraBlocksPresent` | From embedded LNK |
| 43 | `Arguments` | From embedded LNK; parsed fully regardless of length |
| 44 | `Notes` | |

### CustomDestinations (28 columns)

One row per embedded LNK, tagged with its category.

| # | Field | Notes |
|---|-------|-------|
| 1 | `SourceFile` | |
| 2 | `SourceCreated` | Filesystem timestamp of the source `.customDestinations-ms` file |
| 3 | `SourceModified` | |
| 4 | `SourceAccessed` | |
| 5 | `AppId` | |
| 6 | `AppIdDescription` | Resolved via the bundled/`--appIds`-extended AppId table |
| 7 | `EntryName` | Category/entry name |
| 8 | `TargetCreated` | From embedded LNK |
| 9 | `TargetModified` | From embedded LNK |
| 10 | `TargetAccessed` | From embedded LNK |
| 11 | `FileSize` | From embedded LNK |
| 12 | `RelativePath` | From embedded LNK |
| 13 | `WorkingDirectory` | From embedded LNK |
| 14 | `FileAttributes` | From embedded LNK |
| 15 | `HeaderFlags` | From embedded LNK |
| 16 | `DriveType` | From embedded LNK |
| 17 | `VolumeSerialNumber` | From embedded LNK |
| 18 | `VolumeLabel` | From embedded LNK |
| 19 | `LocalPath` | From embedded LNK |
| 20 | `CommonPath` | From embedded LNK |
| 21 | `TargetIDAbsolutePath` | From embedded LNK shell items |
| 22 | `TargetMFTEntryNumber` | From embedded LNK shell items |
| 23 | `TargetMFTSequenceNumber` | From embedded LNK shell items |
| 24 | `MachineID` | From embedded LNK tracker block |
| 25 | `MachineMACAddress` | From embedded LNK tracker block |
| 26 | `TrackerCreatedOn` | From embedded LNK tracker block |
| 27 | `ExtraBlocksPresent` | From embedded LNK |
| 28 | `Arguments` | From embedded LNK; parsed fully regardless of length |

## Examples

```
JLETriage -d /mnt/triage --csv ./out --json ./out
JLETriage -f "/mnt/triage/C/Users/alice/AppData/Roaming/Microsoft/Windows/Recent/AutomaticDestinations/5f7b5f1e01b83767.automaticDestinations-ms" --csv ./out
JLETriage -d /mnt/triage --csv ./out --json ./out --appIds /ref/custom_appids.txt
JLETriage -d /mnt/triage --csv ./out --fd --debug
```
