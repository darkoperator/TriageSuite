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
  --nested-output               Legacy nested layout under <root>/RETriage/<identity>/

Diagnostics:
  --debug                       Emit debug-level diagnostics to stderr
```

### Search flags

RETriage supports key-path and value-name search (mirrors RECmd's `--sk`/`--sv`/`--sd`
flags):

```
  --sk <TERM>                   Search for <term> in key names
  --sv <TERM>                   Search for <term> in value names
  --sd <TERM>                   Search for <term> in value data
  --regex                       Treat search terms as regular expressions
  --literal                     Force literal (substring) matching even if --regex is set
  --minSize <BYTES>             Minimum value data size in bytes (for --sd) [default: 0]
```

All three take a term; `--sd` is a search over value *data*, not a deleted-records toggle.
Search mode is mutually exclusive with batch mode: passing any of the three switches RETriage
to the `search` dataset described under [Velo layout](#velo-layout).

## Output layout

Standalone RETriage's **default is flat**: every file lands directly under the `--csv`/`--json`
root with the identity folded into the name. The batch CSV carries a 14-digit
`<yyyyMMddHHmmss>_` run stamp; the per-plugin detail CSVs are dynamic side-cars and carry none.

```
<out>/
  system_<yyyyMMddHHmmss>_RETriage_Batch_Output.csv   # one row per entry from SYSTEM/SOFTWARE/etc.
  <username>_<yyyyMMddHHmmss>_RETriage_Batch_Output.csv  # one per user hive set
  <PluginName>_<HiveBasename>_system.csv              # per-plugin detail CSV (one per active plugin)
  <PluginName>_<HiveBasename>_<username>.csv
```

`--nested-output` selects the legacy per-identity tree instead:

```
<out>/
  RETriage/
    system/
      <yyyyMMddHHmmss>_RETriage_Batch_Output.csv
      <PluginName>_<HiveBasename>.csv   # no stamp: a dynamic side-car keeps its runtime name
    users/
      <username>/
        <yyyyMMddHHmmss>_RETriage_Batch_Output.csv
        <PluginName>_<HiveBasename>.csv
```

Under `TriageSuite run` neither of these applies -- see [Velo layout](#velo-layout) below.

### Velo layout

Under the default `--layout velo`, RETriage's output lands in
`Processed-<HOST>-<stamp>/Registry/` (verified against a real run):

| File | Contents |
|---|---|
| `<stamp>_RETriage_results_Batch.csv` | every hive (system and user) merged, plus a trailing `TriageUser` column |
| `<PluginName>_<HiveBasename>.csv` | per-plugin detail CSV, system-scope hives only (dynamic runtime basename, no run stamp) |
| `PerUser/Batch/<stamp>_RETriage_results_Batch_<user>.csv` | one user's batch rows, columns exactly as documented above |
| `PerUser/<PluginName>_<HiveBasename>_<user>.csv` | that user's per-plugin detail CSV |

Where the same plugin runs over two hives whose filenames alone would collide in that one
directory, the stem is qualified with the hive's parent directory --
`AppCompatCache_SYSTEM.csv` for `System32/config/SYSTEM` but `AppCompatCache_SYSTEM_RegBack.csv`
for `System32/config/RegBack/SYSTEM`, and `TypedURLs_NTUSER.DAT_Default.csv` /
`_LocalService.csv` / `_NetworkService.csv` for the three service and default profile hives.
See "`--layout velo` (default)" in `docs/tools/TriageSuite.md`, which explains why a dynamic
side-car needs that qualifier and a stamped dataset file does not.

**`RETriage_Search_Output` is not in this table on purpose.** RETriage declares a second dataset,
`search` (Velo discriminator `Search`, so `<stamp>_RETriage_results_Search.csv` with per-user
slices in `PerUser/Search/`), but it is produced only in search mode -- `--sk` / `--sv` / `--sd`
on the standalone `RETriage` CLI. `TriageSuite run` never sets those, so no orchestrator run of
any layout emits it, and the standalone CLI that does has no `--layout velo` of its own: its
flat layout names the files `<identity>_<stamp>_RETriage_Search_Output.csv` (verified on a real
capture: one per identity, e.g. `system_`, `Administrator_`, `cperez_`). The Velo names above
are what the dataset *would* be routed to; nothing ships that routes it there today.

RETriage is `Scope::UserElseSystem`: the batch CSV can be system-scoped (SYSTEM/SOFTWARE/etc.,
written straight to the category-root filename, no `TriageUser`) or per-user (`PerUser/`).
When a run produces both, the merge post-pass needs `--overwrite` to replace the system-scope
batch file with the merged, `TriageUser`-tagged one -- without it, the merge is skipped
(recorded as a non-fatal failure) and `<stamp>_RETriage_results_Batch.csv` is the system-scope
rows only. The per-plugin detail CSVs are a separate, dynamic-basename mechanism
(`OutputRouter::write_dynamic_*`): they are **never merged** regardless of `--overwrite`, since
the merge post-pass only walks a tool's static `DatasetSpec` list and RETriage's per-plugin
output isn't one of those -- a user-scope `<PluginName>_<HiveBasename>_<user>.csv` in
`PerUser/` has no merged, `TriageUser`-tagged counterpart at the category root, ever. See "The
`TriageUser` rule, stated precisely" in `docs/tools/TriageSuite.md`.

`CaseInfo/<stamp>_SysInfo.txt` (see `docs/tools/TriageSuite.md`, "New outputs") is a second
pass over this batch CSV -- it reads `<stamp>_RETriage_results_Batch*.csv` back out rather than
re-parsing the hives, so it reflects whatever RETriage actually wrote for this run.

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
| `PluginDetailFile` | The corresponding per-plugin detail CSV, if any, as a `/`-separated path relative to this tool's output root (see "Following `PluginDetailFile`" below) |

### Following `PluginDetailFile`

`PluginDetailFile` is a cross-reference from a batch row to the per-plugin detail CSV that
row's plugin wrote. It holds that file's routed destination as a `/`-separated path relative
to **this tool's output root**, so it names the file that is actually on disk -- including
the profile name the output layout folds into a per-user side-car's filename
(`TypedURLs_NTUSER.DAT_alice.csv`) and, in the default Velo layout, the `PerUser/`
directory that side-car lives in.

The output root is:

| Layout | Root the reference is relative to |
|---|---|
| Velo (orchestrator default) | the category directory, e.g. `Processed-<HOST>-<stamp>/Registry/` |
| Flat (standalone CLI default) | the `--csv`/`--json` output directory |
| Nested | the tool's per-identity directory, e.g. `<out>/RETriage/users/alice/` |

Under Velo the reference resolves relative to the merged batch CSV's own directory, because
the merged file sits at that root. It does **not** resolve relative to a `PerUser/Batch/`
per-user slice: the same row text is published to both files, they sit at different depths,
and no single relative path can resolve against both. From a slice, resolve against the
category root two levels up.

## Accepted deltas

The following documented divergences exist between RETriage and RECmd output; each is
covered by a named `AcceptedDelta` in the compat test suite:

- **HivePath**: RETriage emits the path as provided on the command line; RECmd uses its own
  temp-directory copy. Compared by basename only.
- **PluginDetailFile**: RECmd emits an absolute path into its own per-run temp directory;
  RETriage emits a path relative to its output root, and appends qualifiers RECmd has no
  need for (see below). Compared by basename, allowing an appended qualifier.
- **PluginDetailFile naming**: RECmd names detail CSVs using its internal class name (e.g.
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
