# LETriage

Windows Shell Link (`.lnk`) parser. Parses the full LNK structure: file header (flags,
target timestamps, file attributes), target IDs via the shell-items engine (rendering the
`TargetIDAbsolutePath`), LinkInfo (volume drive type/serial/label/local path, network
share path), StringData (relative path, working directory, arguments), and the distributed
link TrackerDataBlock (MachineID, MAC address + OUI vendor lookup, tracker creation time).
Produces CSV and NDJSON output. Adds NDJSON output that LECmd does not provide.

## Target Windows versions

The Shell Link binary format is largely unchanged since Windows 2000/XP — the fixed
header, LinkInfo, StringData, and extra-data block layouts parsed here are the same
structures Windows XP through Windows 11 all write. LETriage applies across that whole
range without version-specific branching; artifacts collected from older systems (XP,
Vista, 7) and current systems (10, 11) are parsed identically.

## Compatibility

Output is column-compatible with Eric Zimmerman's LECmd (27 columns, matching names and
order; the bundled MAC OUI table powers the `MACVendor` field). Adds NDJSON output that
LECmd does not provide. One documented incompatibility: Windows Search property-view
targets (`CLSID_SearchFolder`) and their MFT references are not yet extracted.

## Flags

Input (exactly one required):

```
-d, --directory <DIR>         Recursively discover .lnk files under this directory
-f, --file <FILE>             Explicit .lnk file (repeatable)
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

LETriage options:

```
-r, --removable-only          Only emit records whose target volume type is removable
--all                         Examine every file as an LNK candidate (content validation
                               still applies; non-LNK files are skipped with a warning)
--nid                         Suppress Target-ID detail block in console output
                               (does not affect CSV/JSON)
--neb                         Suppress Extra-Block detail in console output
                               (does not affect CSV/JSON)
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
  LETriage/
    users/
      <username>/
        LETriage_Output.csv     # one row per .lnk file under a user profile
        LETriage_Output.json    # same records as NDJSON
    system/
      LETriage_Output.csv       # one row per .lnk file not attributed to a user profile
      LETriage_Output.json
```

LNKs whose path contains a recognisable user-profile segment are routed to
`LETriage/users/<username>/`; all others go to `LETriage/system/`.

## Output fields

The 27-column record (`LnkRecord` in `crates/le-triage/src/lib.rs`), column names and
order pinned to match LECmd:

| # | Field | Notes |
|---|-------|-------|
| 1 | `SourceFile` | Path to the `.lnk` file itself |
| 2 | `SourceCreated` | Filesystem creation time of the `.lnk` file |
| 3 | `SourceModified` | Filesystem modified time of the `.lnk` file |
| 4 | `SourceAccessed` | Filesystem accessed time of the `.lnk` file |
| 5 | `TargetCreated` | Target creation timestamp from the LNK header |
| 6 | `TargetModified` | Target modified timestamp from the LNK header |
| 7 | `TargetAccessed` | Target accessed timestamp from the LNK header |
| 8 | `FileSize` | Target file size from the LNK header |
| 9 | `RelativePath` | StringData relative path |
| 10 | `WorkingDirectory` | StringData working directory |
| 11 | `FileAttributes` | Target file attributes, rendered as flag string |
| 12 | `HeaderFlags` | LNK header data-flags, rendered as flag string |
| 13 | `DriveType` | LinkInfo volume drive type (`(None)` sentinel when absent) |
| 14 | `VolumeSerialNumber` | LinkInfo volume serial number |
| 15 | `VolumeLabel` | LinkInfo volume label |
| 16 | `LocalPath` | LinkInfo local path |
| 17 | `NetworkPath` | LinkInfo network share path |
| 18 | `CommonPath` | LinkInfo common path suffix |
| 19 | `Arguments` | StringData command-line arguments |
| 20 | `TargetIDAbsolutePath` | Absolute path rendered from the LinkTargetIDList via the shell-items engine |
| 21 | `TargetMFTEntryNumber` | MFT entry number from the last target ID's Beef0004 extension block (hex) |
| 22 | `TargetMFTSequenceNumber` | MFT sequence number from the same extension block (hex) |
| 23 | `MachineID` | TrackerDataBlock machine identifier |
| 24 | `MachineMACAddress` | TrackerDataBlock MAC address |
| 25 | `MACVendor` | OUI vendor lookup on `MachineMACAddress` (only resolved when a MAC is present) |
| 26 | `TrackerCreatedOn` | TrackerDataBlock creation timestamp |
| 27 | `ExtraBlocksPresent` | Comma-joined list of extra-data block types present in the LNK |

Optional fields serialize as a JSON `null` (omitted by the null-nuking output sink) when
absent, but render as an empty CSV cell — matching LECmd's `ToJson` behavior. `FileSize`,
`TargetIDAbsolutePath`, and `ExtraBlocksPresent` are fields LECmd always emits, though the
latter two still use `Option` internally so an empty value nukes to `null` in JSON while
staying an empty (not absent) CSV cell.

## Examples

```
LETriage -d /mnt/triage --csv ./out --json ./out
LETriage -f "/mnt/triage/C/Users/alice/AppData/Roaming/Microsoft/Windows/Recent/report.lnk" --csv ./out
LETriage -d /mnt/triage --csv ./out --json ./out --cp 932
```

Restrict to removable-media targets only (e.g. USB exfiltration triage):

```
LETriage -d /mnt/triage --csv ./out -r
```
