# SrumETriage

SRUM (System Resource Usage Monitor) is a Windows subsystem that periodically records
per-application network, CPU, disk, energy, and notification activity into an ESE
database at `%SystemRoot%\System32\sru\SRUDB.dat`. SrumETriage parses that database
directly (no live ESE engine dependency) and emits one dataset per SRUM table: network
usage, network connections, application resource usage, push notifications, energy
usage, application timeline, and VPN/VFU provider activity. Internal integer app and
user references are resolved via the SRUDB's own `SruDbIdMapTable` and, where a
SOFTWARE hive is available, cross-referenced against `ProfileList` to attach human
usernames to SIDs.

## Target Windows versions

Windows 8 and later (SRUM was introduced in Windows 8), through Windows 11 and Server
2025. SrumETriage supports the modern ESE revision-300 page layout used by Windows 11
24H2 and Server 2025, in addition to the older ESE format revisions used by earlier
Windows releases. If a SRUDB uses an ESE format revision newer than any revision the
parser supports, SrumETriage prints a warning and skips the file rather than emitting
partial or incorrect records.

## Compatibility

Output is compatible with Eric Zimmerman's SrumECmd — compatible fixtures were verified,
including the modern ESE revision-300 layout. Field order in each output dataset
matches SrumECmd's declared column order, and SID-resolution logic (SID type
classification, username lookup) mirrors SrumECmd's behavior so that CSV/JSON output
lines up column-for-column with SrumECmd's own output.

## Flags

Input (exactly one required, via the shared TriageSuite CLI conventions):
```
-d, --directory <DIR>         Recursively discover SRUDB.dat files under this directory
-f, --file <FILE>             Explicit SRUDB.dat file (repeatable)
```

Output (at least one required):
```
--csv <DIR>                   Write CSV output beneath this directory
--json <DIR>                  Write NDJSON output beneath this directory
--csvf <NAME>                 Override the default CSV basename
--jsonf <NAME>                Override the default JSON basename
--pretty                      Pretty-print JSON (no effect on NDJSON-framed output)
--overwrite                   Replace existing output files
--nested-output               Preserve the legacy nested output layout under <root>/<ToolName>/<identity>/
```

SrumETriage-specific:
```
--software <PATH>             Path to a SOFTWARE hive for SID->username resolution
                               (overrides the auto-located hive in the SRUDB's capture
                               subtree)
```

Diagnostics:
```
--debug                       Emit debug-level diagnostics to stderr
```

When `--software` is not given, SrumETriage walks the ancestors of the SRUDB path
looking for a sibling `.../Windows/System32/config/SOFTWARE` (or `Software`) hive in
the same capture subtree. If no hive is found, or a hive is found but a particular SID
has no matching `ProfileList` entry, the `Sid` value is emitted as-is with an empty
`UserName`.

## Output layout

Each SRUM table becomes its own dataset file, so a single SRUDB.dat produces up to
seven output files. Because SrumETriage is declared `SystemWide` in scope, all output
lands under `system/` — there is no per-user `users/<name>/` split for this tool (the
SRUDB is a single machine-wide database, and per-record user attribution is carried in
the `UserName`/`Sid` columns of each row instead of the output path).

```
<out>/
  SrumETriage/
    system/
      SrumETriage_NetworkUsages_Output.csv
      SrumETriage_NetworkConnections_Output.csv
      SrumETriage_AppResourceUseInfo_Output.csv
      SrumETriage_PushNotifications_Output.csv
      SrumETriage_EnergyUsage_Output.csv
      SrumETriage_AppTimelineProvider_Output.csv
      SrumETriage_vfuprov_Output.csv
```

This directory/file layout follows the same `<out>/<ToolName>/system|users/<name>/` scheme
used by the rest of TriageSuite; the exact basenames above are read directly from the
`DatasetSpec` table in `crates/srume-triage/src/lib.rs` and are stable regardless of
`--csv`/`--json` root. Without `--nested-output`, the orchestrator's flat naming mode is
used instead, appending an identity suffix (e.g. `SrumETriage_NetworkUsages_Output_system.csv`)
directly under the chosen `--csv`/`--json` root rather than nesting per-tool folders.

## Output datasets and fields

Every dataset shares a common identity/app prefix resolved from the SRUDB's
`SruDbIdMapTable` plus (optionally) a SOFTWARE hive: `ExeInfo`/`ExeInfoDescription`/
`ExeTimestamp` from the app id, and `SidType`/`Sid`/`UserName` from the user id.
`SidType` is the well-known-SID classification (e.g. `LocalSystem`, `Administrator`,
`UnknownOrUserSid`); `UserName` is populated only when a SOFTWARE hive's `ProfileList`
resolves the SID.

### NetworkUsages (`SrumETriage_NetworkUsages_Output`) — ESE table `{973F5D5C-1D90-4944-BE8E-24B94231A174}`

- `Id`, `Timestamp`, `ExeInfo`, `ExeInfoDescription`, `ExeTimestamp`, `SidType`, `Sid`,
  `UserName`, `UserId`, `AppId`
- `BytesReceived`, `BytesSent`
- `InterfaceLuid`, `InterfaceType` (interface-type enum name decoded from bytes 6-7 of
  the LUID; unrecognized codes fall back to the raw numeric value), `L2ProfileFlags`,
  `L2ProfileId`, `ProfileName` (currently always empty — no network-profile-name map is
  resolved)

### NetworkConnections (`SrumETriage_NetworkConnections_Output`) — ESE table `{DD6636C4-8929-4683-974E-22C046A43763}`

- `Id`, `Timestamp`, `ExeInfo`, `ExeInfoDescription`, `ExeTimestamp`, `SidType`, `Sid`,
  `UserName`, `UserId`, `AppId`
- `ConnectedTime`, `ConnectStartTime` (decoded from a Windows FILETIME)
- `InterfaceLuid`, `InterfaceType`, `L2ProfileFlags`, `L2ProfileId`, `ProfileName`
  (always empty, same as NetworkUsages)

### AppResourceUseInfo (`SrumETriage_AppResourceUseInfo_Output`) — ESE table `{D10CA2FE-6FCF-4F6D-848E-B2E99266FA89}`

- `Id`, `Timestamp`, `ExeInfo`, `ExeInfoDescription`, `ExeTimestamp`, `SidType`, `Sid`,
  `UserName`, `UserId`, `AppId`
- `BackgroundBytesRead`, `BackgroundBytesWritten`, `BackgroundContextSwitches`,
  `BackgroundCycleTime`, `BackgroundNumberOfFlushes`, `BackgroundNumReadOperations`,
  `BackgroundNumWriteOperations`
- `FaceTime`
- `ForegroundBytesRead`, `ForegroundBytesWritten`, `ForegroundContextSwitches`,
  `ForegroundCycleTime`, `ForegroundNumberOfFlushes`, `ForegroundNumReadOperations`,
  `ForegroundNumWriteOperations`

### PushNotifications (`SrumETriage_PushNotifications_Output`) — ESE table `{D10CA2FE-6FCF-4F6D-848E-B2E99266FA86}`

- `Id`, `Timestamp`, `ExeInfo`, `ExeInfoDescription`, `ExeTimestamp`, `SidType`, `Sid`,
  `UserName`, `UserId`, `AppId`
- `NetworkType`, `NotificationType`, `PayloadSize`

### EnergyUsage (`SrumETriage_EnergyUsage_Output`) — ESE tables `{FEE4E14F-02A9-4550-B5CE-5FA2DA202E37}` (base) and `{FEE4E14F-02A9-4550-B5CE-5FA2DA202E37}LT` (long-term)

Rows from both the base and "LT" (long-term) energy tables are merged into a single
output dataset, distinguished by the `IsLt` column. Where a column doesn't exist in one
of the two source tables, it is emitted as `-1` (or an empty timestamp for
`EventTimestamp`) rather than being omitted, matching SrumECmd's handling. Row IDs from
the base table that collide with an already-emitted LT-table ID are renumbered to avoid
collisions, mirroring SrumECmd's merge logic.

- `Id`, `Timestamp`, `ExeInfo`, `ExeInfoDescription`, `ExeTimestamp`, `SidType`, `Sid`,
  `UserName`, `UserId`, `AppId`
- `IsLt` (`"True"`/`"False"`), `ConfigurationHash`, `EventTimestamp` (LT rows: empty),
  `StateTransition` (LT rows: `-1`), `ChargeLevel` (LT rows: `-1`), `CycleCount`,
  `DesignedCapacity`, `FullChargedCapacity`
- `ActiveAcTime`, `ActiveDcTime`, `ActiveDischargeTime`, `ActiveEnergy` (base-table rows: `-1`)
- `CsAcTime`, `CsDcTime`, `CsDischargeTime`, `CsEnergy` (base-table rows: `-1`)

### AppTimelineProvider (`SrumETriage_AppTimelineProvider_Output`) — ESE table `{5C8CF1C7-7257-4F13-B223-970EF5939312}`

- `Id`, `Timestamp`, `ExeInfo`, `ExeInfoDescription`, `ExeTimestamp`, `SidType`, `Sid`,
  `UserName`, `UserId`, `AppId`
- `EndTime` (decoded from a Windows FILETIME), `DurationMs`

### vfuprov (`SrumETriage_vfuprov_Output`) — ESE table `{7ACBBAA3-D029-4BE4-9A7A-0885927F1D8F}`

VFU provider activity (per-app foreground-usage/duration windows).

- `Id`, `Timestamp`, `UserId`, `AppId`, `ExeInfo`, `ExeInfoDescription`, `ExeTimestamp`,
  `SidType`, `Sid`, `UserName`
- `StartTime`, `EndTime` (both decoded from Windows FILETIME values)
- `Flags`
- `Duration` — computed as `EndTime - StartTime`, formatted as a .NET `TimeSpan`-style
  string (`[-]hh:mm:ss[.fffffff]`, or `[-]d.hh:mm:ss[.fffffff]` when the span spans one
  or more full days). The source notes this format is an approximation of CsvHelper's
  default `TimeSpan` serialization pending further fixture reconciliation.

Note: not every SRUM table SrumECmd can read is implemented here — for example, no
separate dataset for a "VpnConnection" or generic "Unknown" provider table was found
in `datasets.rs`. Only the seven datasets above are wired up in
`crates/srume-triage/src/lib.rs`'s dataset list and `SrumeTool::parse`.

## Examples

```
SrumETriage -f /mnt/triage/C/Windows/System32/sru/SRUDB.dat --csv ./out
SrumETriage -d /mnt/triage --csv ./out --json ./out
SrumETriage -d /mnt/triage --csv ./out --software /mnt/triage/C/Windows/System32/config/SOFTWARE
SrumETriage -d /mnt/triage --json ./out --pretty --overwrite
```
