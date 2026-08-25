# SBETriage

Windows shellbags parser. Walks the `BagMRU`/`Bags` registry tree in NTUSER.DAT and
UsrClass.dat, reconstructs each shellbag's absolute path, timestamps, MRU ordering, and
explored state, and emits SBECmd's 19-column record model as CSV and NDJSON. Output is
column-compatible with Eric Zimmerman's SBECmd. Hive parsing uses the M5 triage-registry
engine (transaction-log replay + deleted-record recovery via notatin). Shell items are
decoded by the triage-shellitems engine shared with LETriage and JLETriage.

## Target Windows versions

Shellbags exist from Windows XP onward, but the modern BagMRU/Bags structure documented
here (`Shell\BagMRU` and `ShellNoRoam\BagMRU` under NTUSER.DAT, and the UsrClass.dat
variant) is the Vista-and-later layout. SBETriage targets Windows Vista through Windows 11
/ Server. UsrClass.dat is a separate per-user hive introduced in Vista, distinct from
NTUSER.DAT, and is parsed with its own BagMRU root.

## Supported hive types

| Hive | BagMRU root(s) |
|---|---|
| NTUSER.DAT | `Software\Microsoft\Windows\Shell\BagMRU`, `Software\Microsoft\Windows\ShellNoRoam\BagMRU` |
| UsrClass.dat | `Local Settings\Software\Microsoft\Windows\Shell\BagMRU` |

Both hive types are auto-detected from the file name. Transaction logs (`.LOG1` / `.LOG2`
siblings) are replayed when present. Deleted records visible to notatin's allocator scan
are recovered.

## Compatibility

Output is column-compatible with Eric Zimmerman's SBECmd (19-column record model).

SBECmd is **closed-source** (no public repository). Compatibility fixtures are generated
by running the official **SBECmd net9 binary** on macOS via `scripts/gen-sbecmd-fixtures.sh`
with a user-local .NET 9 runtime, for use in comparison testing. This fixture-generation
approach differs from the other TriageSuite tools, where reference fixtures are built from
open-source binaries.

## Flags

```
Input (exactly one required):
  -d, --directory <DIR>         Recursively discover NTUSER.DAT / UsrClass.dat files
  -f, --file <FILE>             Explicit hive file (repeatable)

Output (at least one required):
  --csv <DIR>                   Write CSV output beneath this directory
  --json <DIR>                  Write NDJSON output beneath this directory
  --csvf <NAME>                 Override the default CSV basename
  --jsonf <NAME>                Override the default JSON basename
  --pretty                      Pretty-print JSON (no effect on NDJSON-framed output)
  --overwrite                   Replace existing output files

SBETriage options:
  --nl                          No-logs mode: process hives that have no transaction-log
                                siblings (allow dirty / unclean hives without logs)
  --dedupe                      Deduplicate identical records (same BagPath + Slot)
  --dt <FORMAT>                 Timestamp format string for output (default: ISO 8601 UTC)

Diagnostics:
  -q, --quiet                   Suppress per-file informational messages
  --debug                       Emit debug-level diagnostics to stderr
  --trace                       Emit trace-level diagnostics to stderr (implies --debug)
```

## Output layout

```
<out>/
  SBETriage/
    users/
      <username>/
        SBETriage_Shellbags_Output.csv   # one row per shellbag entry
        SBETriage_Shellbags_Output.json  # same records as NDJSON
    system/
      SBETriage_Shellbags_Output.csv
      SBETriage_Shellbags_Output.json
```

Hives whose path contains a recognisable user-profile segment are routed to
`SBETriage/users/<username>/`; all others go to `SBETriage/system/`. One record set per
hive is written to the user's directory.

## Output columns (19)

`BagPath`, `Slot`, `NodeSlot`, `MRUPosition`, `AbsolutePath`, `ShellType`, `Value`,
`ChildBags`, `CreatedOn`, `ModifiedOn`, `AccessedOn`, `LastWriteTime`, `MFTEntry`,
`MFTSequenceNumber`, `ExtensionBlockCount`, `FirstInteracted`, `LastInteracted`,
`HasExplored`, `Miscellaneous`.

## Accepted deltas

The following 11 documented divergences from SBECmd output are accepted in the compat test
suite (`crates/sbe-triage/tests/compat_sbecmd.rs`). All others fail CI.

1. **Parse-error row (whole-row delta)** — SBECmd emits a `"Type ID: 0x32"` placeholder
   row with empty fields for a structurally malformed shell item (BagMRU\1\2, slot 3).
   SBETriage parses the same bytes successfully; every column on that one row may diverge.

2. **ShellType: CLSID_SearchFolder → "Users property view"** — SBECmd renders class-0x1F
   items whose GUID is a CLSID_SearchFolder variant as `"Users property view"`. Our parser
   uses `"Root folder: GUID"` for all 0x1F items.

3. **ShellType: class-0x71 control-panel GUID → "GUID: Control panel"** — SBECmd maps some
   class-0x71 GUID-backed control-panel sub-items to `"GUID: Control panel"` via GUID
   lookup; we emit `"Users property view"` for all 0x71 items without GUID overrides.

4. **ShellType: zip-content items** — Zip content items use multiple class bytes (0x00,
   0x07, 0x7E etc.); not all variants are mapped to `"Zip file contents"` in our ShellType
   table.

5. **Value: zip-content items** — We do not parse filenames from all zip content item class
   variants; we emit `"Unknown-0x..."` placeholders. Accepted only when ours is a
   placeholder or very short fallback; a future correct parse will break this guard.

6. **AbsolutePath: zip-content items** — AbsolutePath depends on Value; accepted when ours
   contains the `"Unknown-0x"` placeholder token or the leaf segment is ≤ 2 characters.

7. **ShellType: class-0x00 → "Variable: Users property view"** — Some class-0x00 items that
   SBECmd renders as `"Variable: Users property view"` (via PropertyStore probe) lack the
   `0xAFBB00B5` signature we detect; we emit `"Variable"`.

8. **Value: class-0x00 unresolved PropertyStore → "Unknown-0x00"** — Class-0x00 items whose
   embedded PropertyStore display name we cannot decode emit `"Unknown-0x00"` instead of
   the resolved name.

9. **AbsolutePath: class-0x00 unresolved** — AbsolutePath for rows where Value is
   `"Unknown-0x00"` will diverge from SBECmd's resolved path.

10. **AbsolutePath: cascade from unresolved parent** — Child items of an unresolved class-0x00
    parent inherit `"Unknown-0x00"` as a path segment, propagating the divergence into all
    deeper entries.

11. **ExtensionBlockCount undercount for CLSID_SearchFolder** — Some class-0x1F items embed
    a full IDList in their body; our forward BEEF-block scanner finds blocks inside the IDList
    and terminates there, missing outer BEEF blocks. Undercount only (reference > ours); root
    fix requires class-specific layout parsing.

**Timestamp note:** timestamps are compared at whole-second precision (SBECmd renders
whole-second; SBETriage renders 7-digit FILETIME fractional seconds representing the same
instant). Our CSV is pre-normalized before comparison — no timestamp AcceptedDelta is needed.

## Examples

Parse a NTUSER.DAT from a Velociraptor capture:

```bash
SBETriage -f "/mnt/triage/C/Users/alice/NTUSER.DAT" --csv ./out
```

Parse all hives in a capture directory, emit both CSV and NDJSON:

```bash
SBETriage -d /mnt/triage --csv ./out --json ./out
```

Parse a dirty hive that has no transaction logs:

```bash
SBETriage -f "/mnt/triage/C/Users/alice/UsrClass.dat" --csv ./out --nl
```
