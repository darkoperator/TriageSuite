# MFTriage

NTFS artifact parser covering the three core filesystem-metadata sources on an NTFS
volume: the `$MFT` (Master File Table — one FILE record per file/directory entry, walked
with update-sequence-array fixup and 48-bit entry-number/attribute parsing), the `$J`
change journal (the `$UsnJrnl:$J` alternate data stream — a sparse, append-only log of
per-file change records), and the `$Boot` sector (volume geometry and cluster layout).
Both the `$MFT` and `$J` parsers are streaming: each artifact is read and its logical
rows are emitted to the output router as they're produced rather than being buffered
into memory as a whole parsed structure, so MFTriage stays bounded-memory on
multi-gigabyte `$MFT`/`$J` files from large volumes. `$J` parent-path resolution builds a
lightweight entry-number path index from an accompanying `$MFT` (either explicit via
`--mft` or auto-discovered as the sibling `$MFT` next to `$J`) in a first pass, then
streams `$J` records against that index in a second pass.

## Target Windows versions

NTFS is universal across all NT-based Windows versions still in use (2000 through 11 /
Server); `$MFT` and `$Boot` are always present on an NTFS volume, and the `$UsnJrnl:$J`
change journal is present whenever the volume has USN journaling enabled (the default
since Windows Vista/Server 2008, and typically present even further back). Nothing in
MFTriage's parsing is gated to a specific Windows release — the on-disk FILE record,
boot sector, and USN v2 record layouts it decodes have not changed across that range.
For multi-drive captures (a capture with C: and D: both collected, each contributing its
own `$MFT`/`$J`/`$Boot`), see [Multi-drive captures](#multi-drive-captures) below —
`SourceFile` is what keeps rows from different drives distinguishable.

## Compatibility

Output is compatible with Eric Zimmerman's MFTECmd — same column names, order, and
content for the `$MFT`, file-listing, `$J`, and `$Boot` datasets. MFTriage's parser is a
streaming design (bounded memory, two-pass `$J` parent-path resolution) rather than a
full in-memory structure walk.

## Flags

```
Input (exactly one required):
  -d, --directory <DIR>         Recursively discover $MFT/$Boot/$J files under this directory
  -f, --file <FILES>            Explicit artifact file (repeatable)

Output (at least one required):
  --csv <DIR>                   Write MFTECmd-compatible CSV output beneath this directory
  --json <DIR>                  Write MFTECmd-compatible NDJSON output beneath this directory
  --csvf <NAME>                 Override the default CSV basename
  --jsonf <NAME>                Override the default JSON basename
  --pretty                      Pretty-print JSON (no effect on NDJSON-framed output)
  --overwrite                   Replace existing output files
  --nested-output               Preserve the legacy nested output layout under <root>/MFTriage/<identity>/

Diagnostics:
  --debug                       Emit debug-level diagnostics to stderr
  --trace                       Emit trace-level diagnostics to stderr (implies --debug)
  -q, --quiet                   Suppress per-file informational messages
```

### MFTriage-specific flags

```
  --sn                          Include DOS (8.3) short names in $MFT output
  --at                          Include all $FILE_NAME (0x30) timestamps, not only when
                                they differ from the corresponding $STANDARD_INFORMATION
                                (0x10) timestamp
  --fl                          Also emit the $MFT file-listing dataset
  --mft <PATH>                  $MFT used to resolve $J parent paths (default: the
                                sibling $MFT next to $J)
```

`--sn` and `--at` only affect `$MFT` parsing. `--fl` derives one file-listing row per
non-ADS `$MFT` row into a second dataset. `--mft` only matters when parsing a `$J`
artifact; it is ignored for `$MFT`/`$Boot` input.

## Output layout

By default (flat mode), MFTriage is `SystemWide`-scoped — all rows attribute to the
`system` identity and the identity is folded into each output filename:

```
<out>/
  MFTriage_$MFT_Output_system.csv
  MFTriage_$MFT_Output_FileListing_system.csv   # only emitted with --fl
  MFTriage_$J_Output_system.csv
  MFTriage_$Boot_Output_system.csv
```

With `--nested-output`, the legacy tree is used instead:

```
<out>/
  MFTriage/
    system/
      MFTriage_$MFT_Output.csv
      MFTriage_$MFT_Output_FileListing.csv       # only emitted with --fl
      MFTriage_$J_Output.csv
      MFTriage_$Boot_Output.csv
```

Each dataset is a single output file per run — when a directory scan (`-d`) discovers
`$MFT`/`$J`/`$Boot` from multiple drives, all matching rows are appended into the same
per-dataset file, distinguished by the `SourceFile` column (see
[Multi-drive captures](#multi-drive-captures)).

## Output datasets and fields

### $MFT records (34 columns)

`EntryNumber, SequenceNumber, InUse, ParentEntryNumber, ParentSequenceNumber,
ParentPath, FileName, Extension, FileSize, ReferenceCount, ReparseTarget, IsDirectory,
HasAds, IsAds, SI<FN, uSecZeros, Copied, SiFlags, NameType, Created0x10, Created0x30,
LastModified0x10, LastModified0x30, LastRecordChange0x10, LastRecordChange0x30,
LastAccess0x10, LastAccess0x30, UpdateSequenceNumber, LogfileSequenceNumber,
SecurityId, ObjectIdFileDroid, LoggedUtilStream, ZoneIdContents, SourceFile`

Notes:
- One row is emitted per `$FILE_NAME` (0x30) attribute on a record (a file can carry a
  Win32 name and, with `--sn`, a separate DOS 8.3 name); one additional row is emitted
  per named `$DATA` attribute (alternate data stream), with `IsAds` set and `FileName`
  formatted as `name:stream`.
- `SI<FN` is true when the `$FILE_NAME` created timestamp predates
  `$STANDARD_INFORMATION` created (a timestomping indicator).
- The `…0x30` timestamp columns are left empty unless they differ from their `…0x10`
  counterpart, unless `--at` is passed, in which case all four are always populated.
- `ObjectIdFileDroid`, `LoggedUtilStream`, and `ZoneIdContents` are reserved output
  columns (present for MFTECmd column compatibility) and are not currently populated.

### File listing — `--fl` (7 columns)

`FullPath, Extension, IsDirectory, FileSize, Created0x10, LastModified0x10, SourceFile`

One row per non-ADS `$MFT` row (`FullPath` is `ParentPath` and `FileName` joined with a
backslash); alternate-data-stream rows are excluded from this dataset.

### $J / UsnJrnl records (13 columns)

`Name, Extension, EntryNumber, SequenceNumber, ParentEntryNumber,
ParentSequenceNumber, ParentPath, UpdateSequenceNumber, UpdateTimestamp,
UpdateReasons, FileAttributes, OffsetToData, SourceFile`

Notes:
- `UpdateReasons` and `FileAttributes` are `|`-joined flag names (e.g.
  `DataExtend|DataTruncation`, `Archive|NotContentIndexed`).
- `ParentPath` is resolved against the `$MFT` path index described above (`--mft` or
  the sibling `$MFT`); when no `$MFT` is available or the parent entry can't be
  resolved, `ParentPath` is left empty.
- Malformed or short USN records (below the minimum v2 record length, or with an
  unsupported major version) are skipped rather than aborting the parse; sparse
  zero-padding regions of the `$J` file are also skipped.

### $Boot record (17 columns)

`EntryPoint, Signature, BytesPerSector, SectorsPerCluster, ClusterSize,
ReservedSectors, TotalSectors, MftClusterBlockNumber, MftMirrClusterBlockNumber,
MftEntrySize, IndexEntrySize, VolumeSerialNumberRaw, VolumeSerialNumber,
VolumeSerialNumber32, VolumeSerialNumber32Reverse, SectorSignature, SourceFile`

One row per `$Boot` sector parsed. `VolumeSerialNumber` is the full 64-bit serial as an
uppercase hex string; `VolumeSerialNumber32`/`VolumeSerialNumber32Reverse` are the
low 32 bits, straight and byte-swapped, matching the two common ways Windows tooling
renders a volume serial. Parsing requires the `NTFS    ` OEM ID at offset 3 and the
`0xAA55` boot-sector signature at the end of the sector; either failing marks the
artifact corrupt.

## Multi-drive captures

Because `$MFT`, `$J`, and `$Boot` output rows from every matched input file are appended
into the same per-dataset output file, a capture containing both `C:\$MFT` and
`D:\$MFT` (two drives, both discovered under `-d`) produces one combined
`MFTriage_$MFT_Output` file rather than one per drive. The `SourceFile` column (present
on all four datasets, including the `--fl` file-listing dataset) carries the path of the
artifact each row came from, so rows from different drives — which otherwise may share
overlapping entry numbers, since `$MFT` entry numbers are only unique within a single
volume — stay distinguishable after the drives are combined.

## Examples

Parse a single `$MFT` and emit MFTECmd-compatible CSV:

```bash
MFTriage -f /mnt/triage/C/\$MFT --csv ./out
```

Parse a full capture directory (all `$MFT`/`$J`/`$Boot` files discovered automatically,
across all drives present) and also emit the file-listing dataset:

```bash
MFTriage -d /mnt/triage --csv ./out --fl
```

Parse `$MFT` including DOS short names and every `$FILE_NAME` timestamp, writing both
CSV and NDJSON:

```bash
MFTriage -f /mnt/triage/C/\$MFT --csv ./out --json ./out --sn --at
```

Parse a `$J` change journal, resolving parent paths against an explicit `$MFT` rather
than the auto-discovered sibling:

```bash
MFTriage -f "/mnt/triage/C/\$Extend/\$UsnJrnl%3A\$J" --mft /mnt/triage/C/\$MFT --csv ./out
```
