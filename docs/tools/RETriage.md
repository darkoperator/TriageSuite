# RETriage

Windows Registry parser. Reads any registry hive file (SYSTEM, SOFTWARE, NTUSER.DAT,
UsrClass.dat, SAM, SECURITY, DEFAULT, and RegBack copies) and produces a batch CSV and
per-plugin detail CSVs that are column-compatible with RECmd's output. Hives are parsed
by [notatin](https://github.com/strozfriedberg/notatin); only cells reachable via the
active allocated-cell graph are processed (orphaned slack-space cells visible to RECmd's
NuGet library are omitted by design).

## Target Windows versions

Registry hive parsing itself is universal across all NT-based Windows versions (2000
through 11 / Server) — the hive binary format and cell/bin structure that notatin walks
has not changed across that range. Individual plugin applicability, however, varies by
the Windows version that introduced the key path a plugin targets: RADAR (Resource
Exhaustion Detection and Resolution) is a Windows 10+ feature, and Known networks
(network profile / managed-network history) likewise targets keys introduced in Windows
10. Beyond these, plugin-level version specificity is not tracked per-plugin in this
document — a plugin will simply return no rows on a hive from a Windows version that
predates the key path it targets.

## Compatibility

Output is column-compatible with Eric Zimmerman's RECmd — same 15 batch columns, same
order, matching RECmd's `DFIRBatch.reb` profile. Per-plugin detail CSVs follow RECmd's
plugin model (34 plugins). The [Accepted deltas](#accepted-deltas) section below documents
known, understood divergences from RECmd output — not defects — each covered by a named
`AcceptedDelta` in the compat test suite.

## Flags

```
Input (exactly one required):
  -d, --directory <DIR>         Recursively discover hive files under this directory
  -f, --file <FILES>            Explicit hive file (repeatable)

Output (at least one required):
  --csv <DIR>                   Write CSV output beneath this directory
  --json <DIR>                  Write NDJSON output beneath this directory
  --csvf <NAME>                 Override the default CSV basename
  --jsonf <NAME>                Override the default JSON basename
  --pretty                      Pretty-print JSON (no effect on NDJSON-framed output)
  --overwrite                   Replace existing output files

Diagnostics:
  --debug                       Emit debug-level diagnostics to stderr
```

### Search flags

RETriage supports key-path and value-name search (mirrors RECmd's `--sk`/`--sv`/`--sd`
flags):

```
  --sk <PATTERN>                Search for keys matching the given pattern (case-insensitive
                                substring or glob)
  --sv <PATTERN>                Search for values matching the given pattern
  --sd                          Include deleted/orphaned keys and values in search results
```

## Output layout

```
<out>/
  RETriage/
    system/
      RETriage_Batch_Output.csv         # one row per registry entry (SYSTEM/SOFTWARE/etc.)
      <PluginName>_<HiveBasename>.csv   # per-plugin detail CSV (one per active plugin)
    users/
      <username>/
        RETriage_Batch_Output.csv       # one row per entry from that user's NTUSER.DAT / UsrClass.dat
        <PluginName>_<HiveBasename>.csv
```

## Plugins (34)

| Plugin name | Hive | Description |
|---|---|---|
| AppCompatCache | SYSTEM | Application Compatibility Cache (ShimCache) execution artefacts |
| AppCompatFlags2 | NTUSER | AppCompat compatibility layer flags per executable |
| AppPaths | SOFTWARE | Per-application default launch paths (`App Paths` key) |
| BamDam | SYSTEM | Background Activity Moderator execution times |
| ComDlg32 CIDSizeMRU | NTUSER | Common dialog box size MRU for each program |
| ComDlg32 LastVisitedPidlMRU | NTUSER | Common dialog "last visited" folder per extension |
| ComDlg32 OpenSavePidlMRU | NTUSER | Common dialog open/save folder MRU |
| DeviceClasses | SYSTEM | Device class GUIDs and associated device entries |
| ETW | SOFTWARE | Event Tracing for Windows registered providers |
| File Extensions | NTUSER | Registered file extension handlers |
| FirewallRules | SYSTEM | Windows Firewall inbound and outbound rules |
| First folder | NTUSER | First folder opened per program (ComDlg32) |
| IconLayouts | NTUSER | Desktop icon position layout |
| JumplistData | NTUSER | Program execution timestamps from Jump List data key |
| Known networks | SOFTWARE | Network connection history (profiles, managed networks, MAC) |
| NetworkAdapters | SYSTEM | Network adapter hardware configuration |
| NetworkSetup2 | SYSTEM | Network setup configuration |
| Office MRU | NTUSER | Microsoft Office most-recently-used file lists |
| Products | SOFTWARE | Windows Installer product registration |
| ProfileList | SOFTWARE | User profile SID-to-path mappings |
| RADAR | SOFTWARE | Resource Exhaustion Detection and Resolution data |
| Recent documents | NTUSER | Shell recent-documents MRU (RecentDocs key) |
| SCSI | SYSTEM | SCSI/storage controller enumeration |
| Services | SYSTEM | Windows Services and drivers |
| Taskband | NTUSER | Pinned taskbar items |
| TaskCache | SOFTWARE | Task Scheduler task cache |
| TrustedDocuments | NTUSER | Microsoft Office trusted-document records |
| TypedURLs | NTUSER | Internet Explorer / Edge typed URL history |
| UnInstall | SOFTWARE / NTUSER | Add/Remove Programs (Uninstall) entries |
| UserAssist | NTUSER | GUI program execution tracking (ROT-13 encoded) |
| VolumeInfoCache | SOFTWARE | Volume label and file-system cache |
| Windows App | UsrClass | Windows Store application registrations |
| WordWheelQuery | NTUSER | Windows Search typed query history |
| TimeZoneInfo | SYSTEM | Active time zone configuration |

## Output fields (batch CSV)

The batch CSV has 15 columns, matching RECmd's `DFIRBatch.reb` profile in name, order,
and content:

| Column | Description |
|---|---|
| `HivePath` | Path to the source hive file |
| `HiveType` | Hive type enum member (NtUser, Software, UsrClass, System, ...) |
| `Description` | Human-readable description of the entry |
| `Category` | Plugin category/grouping |
| `KeyPath` | Full registry key path |
| `ValueName` | Name of the registry value |
| `ValueType` | Registry value type (REG_SZ, REG_DWORD, etc.) |
| `ValueData` | Primary decoded/rendered value data |
| `ValueData2` | Secondary decoded/rendered value data |
| `ValueData3` | Tertiary decoded/rendered value data |
| `Comment` | Plugin-supplied annotation |
| `Recursive` | Whether the entry was produced by recursive key traversal |
| `Deleted` | Whether the entry is a deleted/orphaned record |
| `LastWriteTimestamp` | Key's last-write timestamp |
| `PluginDetailFile` | Basename of the corresponding per-plugin detail CSV, if any |

## Accepted deltas

The following documented divergences exist between RETriage and RECmd output; each is
covered by a named `AcceptedDelta` in the compat test suite:

- **HivePath**: RETriage emits the path as provided on the command line; RECmd uses its own
  temp-directory copy. Compared by basename only.
- **PluginDetailFile**: RECmd names detail CSVs using its internal class name (e.g.
  `AppCompat`, `OfficeMRU`, `RecentDocs`, `FileExts`, `FirstFolder`); RETriage uses
  `plugin_name()` (e.g. `AppCompatCache`, `Office MRU`, `Recent documents`,
  `File Extensions`, `First folder`). Compared by stripping the known prefix differences.
- **TypedURLs ValueData3**: Slack bytes beyond the value's null terminator may differ between
  RECmd and notatin (heap-state dependent). Content is informational only.
- **OpenSave/LastVisited ValueData and ValueData3**: Shell-item path rendering may differ for
  well-known folder GUIDs (`PROGRA~1` vs `@shell32.dll,-21781`, etc.). Same divergence as in
  the detail-CSV tests.
- **Recent documents ValueData/ValueData2/ValueData3**: LnkName extraction from beef0004
  blocks and ExtensionLastOpened (requires cross-key hive lookup, left empty in RETriage)
  may differ. Same divergence as in the detail-CSV test.
- **TrustedDocuments ValueData2**: RECmd's underlying `DateTime.ToString` uses a NARROW
  NO-BREAK SPACE (U+202F) before AM/PM on Windows; RETriage uses a regular space.
- **OfficeMRU and RecentDocs duplicate rows**: RECmd's plugin-activation logic adds
  OfficeMRU twice for its overlapping key paths and adds RecentDocs rows twice (plugin +
  engine recursion). RETriage deduplicates — reference fixture occurrences #2+ are absent
  from RETriage's output by design.
- **Orphaned records**: RECmd's NuGet library reads VK/NK bytes from raw hive slack space not
  reachable via the active allocated-cell graph. notatin follows the graph strictly; those
  records are omitted.
- **Empty-key rows from unported plugins**: RECmd's `DFIRBatch.reb` references a small
  number of plugins not included in RETriage's 34 (BTHPORT, WindowsPortableDevices, RunMRU,
  TerminalServerClient). When a corresponding key exists in the hive but has no values,
  RECmd fires the plugin (returns 0 rows) and emits nothing; RETriage emits one empty default
  row. This is a conservative choice: RETriage surfaces the key's existence even without a
  dedicated plugin.

## Examples

Parse a SYSTEM hive and emit batch CSV:

```bash
RETriage -f /mnt/triage/C/Windows/System32/config/SYSTEM --csv ./out
```

Parse a full Velociraptor capture directory (all hives discovered automatically):

```bash
RETriage -d /mnt/triage --csv ./out
```

Parse a NTUSER.DAT and emit both CSV and NDJSON:

```bash
RETriage -f "/mnt/triage/C/Users/alice/NTUSER.DAT" --csv ./out --json ./out
```

Search for a key path pattern across all hives in a capture:

```bash
RETriage -d /mnt/triage --csv ./out --sk "Run"
```
