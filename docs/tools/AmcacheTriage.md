# AmcacheTriage

Amcache.hve is a Windows registry hive at `C:\Windows\AppCompat\Programs\Amcache.hve` that
tracks an inventory of installed programs, executed binaries, drivers, and paired devices,
each keyed by identifiers such as SHA1 file hashes and program/product GUIDs. AmcacheTriage
parses the **new-format** Amcache schema only — the `Root\Inventory*` key tree (
`InventoryApplication`, `InventoryApplicationFile`, `InventoryApplicationShortcut`,
`InventoryDriverBinary`, `InventoryDeviceContainer`, `InventoryDriverPackage`,
`InventoryDevicePnp`) used from roughly Windows 10 1607 onward. It does not read the legacy
Windows 8/8.1 `Root\File` key schema; a hive lacking `Root\InventoryApplicationFile` is
detected as not-new-format and skipped with a warning, emitting zero records.

## Target Windows versions

Amcache.hve was introduced in Windows 8, replacing the Windows 7 `RecentFileCache.bcf`. Its
schema changed again with the "new format" `Root\Inventory*` key tree, which AmcacheTriage
targets exclusively — this covers Windows 10 1607 (Anniversary Update) and later, including
Windows 11 and Windows Server releases built on that hive layout. Windows 8 / 8.1 captures
using the legacy `Root\File` key schema (no `InventoryApplicationFile` key) are **out of
scope** for this tool: AmcacheTriage detects the absence of `Root\InventoryApplicationFile`,
prints a warning to stderr, and produces no output rows for that hive rather than attempting
a legacy-format parse.

## Compatibility

Output is compatible with Eric Zimmerman's AmcacheParser (new-format / `-i` mode). Row
decode fidelity is checked in `crates/amc-triage/tests/compat_amcacheparser.rs` against a
captured AmcacheParser oracle; every row AmcacheTriage emits is required to match an oracle
row across all columns (timestamps compared at whole-second precision, since AmcacheParser's
default `--dt` is `yyyy-MM-dd HH:mm:ss` while AmcacheTriage renders 7-digit ISO-8601 `…Z`).
Coverage is a documented strict subset for high-churn datasets: notatin (the suite's hive
engine) recovers fewer deleted Amcache subkeys than AmcacheParser's Eric-Zimmerman Registry
library, so AmcacheTriage's output can be a subset of the oracle for `InventoryApplication`,
`InventoryApplicationFile`, `InventoryApplicationShortcut`, and `InventoryDriverBinary`
entries, while matching exactly for `InventoryDeviceContainer` and `InventoryDevicePnp`
(datasets observed with no recoverable deleted entries).

## Flags

```
Input (exactly one required):
  -d, --directory <DIR>         Recursively discover Amcache.hve files
  -f, --file <FILE>             Explicit hive file (repeatable)

Output (at least one required):
  --csv <DIR>                   Write Zimmerman-compatible CSV output beneath this directory
  --json <DIR>                  Write Zimmerman-compatible JSON output beneath this directory
  --csvf <NAME>                 Override the default CSV basename
  --jsonf <NAME>                Override the default JSON basename
  --pretty                      Pretty-print JSON (whitespace only; ignored for NDJSON output)
  --overwrite                   Allow replacement of existing output files
  --nested-output               Preserve the legacy nested output layout under <root>/<ToolName>/<identity>/

AmcacheTriage options:
  --nl                          Skip pairing the hive with its .LOG1/.LOG2 transaction log siblings

Diagnostics:
  -q, --quiet                   Suppress per-file informational messages (progress/summary remain)
  --debug                       Emit debug-level diagnostics to stderr
  --trace                       Emit trace-level diagnostics to stderr (implies --debug)
```

Amcache.hve is validated by its `regf` magic header before parsing; files named
`*.LOG`, `*.LOG1`, or `*.LOG2` are rejected as primary inputs (they are only consumed as
transaction-log siblings of a matched `Amcache.hve`). AmcacheTriage is a machine-wide
(system-scoped) tool — it does not attribute output to a per-user identity.

## Output layout

AmcacheTriage follows the suite's standard output-router layout for a system-scoped tool:

```
<out>/
  AmcacheTriage/
    system/
      AmcacheTriage_UnassociatedFileEntries_Output.csv
      AmcacheTriage_UnassociatedFileEntries_Output.json
      AmcacheTriage_UnassociatedFileEntries_Output_AssociatedFileEntries.csv
      AmcacheTriage_UnassociatedFileEntries_Output_AssociatedFileEntries.json
      AmcacheTriage_UnassociatedFileEntries_Output_ProgramEntries.csv
      AmcacheTriage_UnassociatedFileEntries_Output_ProgramEntries.json
      AmcacheTriage_UnassociatedFileEntries_Output_ShortCuts.csv
      AmcacheTriage_UnassociatedFileEntries_Output_ShortCuts.json
      AmcacheTriage_UnassociatedFileEntries_Output_DriveBinaries.csv
      AmcacheTriage_UnassociatedFileEntries_Output_DriveBinaries.json
      AmcacheTriage_UnassociatedFileEntries_Output_DeviceContainers.csv
      AmcacheTriage_UnassociatedFileEntries_Output_DeviceContainers.json
      AmcacheTriage_UnassociatedFileEntries_Output_DriverPackages.csv
      AmcacheTriage_UnassociatedFileEntries_Output_DriverPackages.json
      AmcacheTriage_UnassociatedFileEntries_Output_DevicePnps.csv
      AmcacheTriage_UnassociatedFileEntries_Output_DevicePnps.json
```

`unassociated_file_entries` is the tool's one **primary** dataset (default basename
`AmcacheTriage_UnassociatedFileEntries_Output`); it receives `--csvf`/`--jsonf` verbatim when
those overrides are passed. The other seven datasets are **derived**: each has a fixed
suffix appended to the primary's basename stem (`_AssociatedFileEntries`,
`_ProgramEntries`, `_ShortCuts`, `_DriveBinaries`, `_DeviceContainers`, `_DriverPackages`,
`_DevicePnps`), following the same stem+suffix convention used elsewhere in TriageSuite
(e.g. PECmd's `_Timeline` file). This naming is read directly from the `DatasetSpec` table
in `crates/amc-triage/src/lib.rs`; the directory nesting (`<out>/AmcacheTriage/system/...`)
is inferred from the shared `OutputRouter`/`OutputLayout` machinery
(`crates/triage-core/src/output/layout.rs`), which places `Scope::SystemWide` tools under
`<root>/<BinaryName>/system/` rather than a per-user path. Pass `--nested-output` to keep
the legacy `<root>/<ToolName>/<identity>/` nesting instead of the current layout.

## Output datasets and fields

AmcacheTriage emits eight datasets, one per `Root\Inventory*` key it walks. All eight are
implemented in `crates/amc-triage/src/records.rs`; no other Amcache sub-artifact types are
covered by this crate.

### ProgramEntries (from `Root\InventoryApplication`)

One row per installed-program subkey. 26 columns:

`ProgramId`, `KeyLastWriteTimestamp`, `Name`, `Version`, `Publisher`,
`InstallDateArpLastModified`, `InstallDate`, `InstallDateMsi`, `OSVersionAtInstallTime`,
`InstallDateFromLinkFile`, `BundleManifestPath`, `HiddenArp`, `InboxModernApp`, `Language`,
`ManifestPath`, `MsiPackageCode`, `MsiProductCode`, `PackageFullName`, `ProgramInstanceId`,
`RegistryKeyPath`, `RootDirPath`, `Type`, `Source`, `StoreAppType`, `UninstallString`,
`Manufacturer`.

`ProgramId` → `Name` pairs collected here are used to resolve `ApplicationName` on file
entries (see below).

### UnassociatedFileEntries / AssociatedFileEntries (from `Root\InventoryApplicationFile`)

Both share one 21-column record schema (`FileEntryRecord`); rows are routed to
`associated_file_entries` when the entry's `ProgramId` matches a known `InventoryApplication`
program, otherwise to `unassociated_file_entries` (with `ApplicationName` set to
`"Unassociated"`). Columns:

`ApplicationName`, `ProgramId`, `FileKeyLastWriteTimestamp`, `SHA1`, `IsOsComponent`,
`FullPath`, `Name`, `FileExtension`, `LinkDate`, `ProductName`, `Size`, `Version`,
`ProductVersion`, `LongPathHash`, `BinaryType`, `IsPeFile`, `BinFileVersion`,
`BinProductVersion`, `Usn`, `Language`, `Description`.

Notable decoding, per `crates/amc-triage/src/values.rs`:
- `SHA1` is derived from the raw `FileId` value by dropping its 4-character type-prefix and
  lowercasing the remainder (e.g. `FileId` `0006<40 hex chars>` → 40-char lowercase SHA1).
- `FileExtension` reproduces .NET's `Path.GetExtension` semantics against `FullPath`
  (falling back to `Name` if `FullPath` has none), including its handling of interior dots
  in directory segments (e.g. `john.doe` in a path does not count as an extension).
- `Size` accepts either `0x`-prefixed hex or decimal in the source value.
- `LinkDate` reproduces .NET's `DateTimeOffset.MinValue` literal
  (`0001-01-01T00:00:00.0000000Z`) when a non-empty `LinkDate` value fails to parse.
- `IsOsComponent` / `IsPeFile` are rendered as .NET-style `"True"`/`"False"` booleans
  (source value `"1"` = true).

### ShortCuts (from `Root\InventoryApplicationShortcut`)

3 columns: `KeyName`, `LnkName`, `KeyLastWriteTimestamp`. `LnkName` is the raw value data of
the subkey's first value (not matched by value name).

### DriveBinaries (from `Root\InventoryDriverBinary`)

20 columns: `KeyName`, `KeyLastWriteTimestamp`, `DriverTimeStamp`, `DriverLastWriteTime`,
`DriverName`, `DriverInBox`, `DriverIsKernelMode`, `DriverSigned`, `DriverCheckSum`,
`DriverCompany`, `DriverId`, `DriverPackageStrongName`, `DriverType`, `DriverVersion`,
`ImageSize`, `Inf`, `Product`, `ProductVersion`, `Service`, `WdfVersion`.

`DriverTimeStamp` is decoded from a Unix epoch-seconds value (values ≤ 0 render empty).
`DriverId` has its 4-character type prefix stripped (without lowercasing) — unlike the
`DriverId` on the DevicePnps dataset below, which is left raw.

### DeviceContainers (from `Root\InventoryDeviceContainer`)

17 columns: `KeyName`, `KeyLastWriteTimestamp`, `Categories`, `DiscoveryMethod`,
`FriendlyName`, `Icon`, `IsActive`, `IsConnected`, `IsMachineContainer`, `IsNetworked`,
`IsPaired`, `Manufacturer`, `ModelId`, `ModelName`, `ModelNumber`, `PrimaryCategory`, `State`.

### DriverPackages (from `Root\InventoryDriverPackage`)

12 columns: `KeyName`, `KeyLastWriteTimestamp`, `Date`, `Class`, `Directory`, `DriverInBox`,
`Hwids`, `Inf`, `Provider`, `SubmissionId`, `SYSFILE`, `Version`.

### DevicePnps (from `Root\InventoryDevicePnp`)

25 columns: `KeyName`, `KeyLastWriteTimestamp`, `BusReportedDescription`, `Class`,
`ClassGuid`, `Compid`, `ContainerId`, `Description`, `DriverId`, `DriverPackageStrongName`,
`DriverName`, `DriverVerDate`, `DriverVerVersion`, `Enumerator`, `HWID`, `Inf`,
`InstallState`, `Manufacturer`, `MatchingId`, `Model`, `ParentId`, `ProblemCode`, `Provider`,
`Service`, `Stackid`. Here `DriverId` is emitted raw (not prefix-stripped), unlike
DriveBinaries' `DriverId`.

## Examples

Parse a single Amcache.hve from a Velociraptor capture, writing CSV:

```bash
AmcacheTriage -f "/mnt/triage/C/Windows/AppCompat/Programs/Amcache.hve" --csv ./out
```

Recursively discover and parse every Amcache.hve under a capture root, writing both CSV and
NDJSON:

```bash
AmcacheTriage -d /mnt/triage --csv ./out --json ./out
```

Parse a hive without pairing it against `.LOG1`/`.LOG2` transaction logs:

```bash
AmcacheTriage -f /mnt/triage/C/Windows/AppCompat/Programs/Amcache.hve --csv ./out --nl
```

Parse with pretty-printed JSON and an overridden primary output basename:

```bash
AmcacheTriage -d /mnt/triage --json ./out --pretty --jsonf Host01_Amcache
```
