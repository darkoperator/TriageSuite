# WxTTriage

WxTTriage parses `ActivitiesCache.db`, the SQLite database behind the Windows Timeline /
Activity History feature. It reads the `Activity`, `ActivityOperation`, and
`Activity_PackageId` tables and emits WxTCmd's three corresponding datasets. ("WxT" in the
tool name and CLI binary refers to **W**indows **T**imeline, not any "XTension" artifact —
the crate's own module doc and `--help` text both state this plainly, and the tables it reads
are exactly Timeline's tables.)

## Target Windows versions

Windows Timeline (and its backing `ActivitiesCache.db`, stored per-user under
`AppData\Local\ConnectedDevicesPlatform\<CDP-ID>\ActivitiesCache.db`) was introduced in the
Windows 10 April 2018 Update (version 1803) and remained present through later Windows 10
feature updates. Microsoft discontinued the Timeline UI starting with Windows 10 2004/20H2
and it is absent from Windows 11; on those later builds the database may still exist as a
leftover from an in-place upgrade, or from `ConnectedDevicesPlatform` activity tracking that
continued after the Timeline UI itself was removed, but WxTTriage's practical target window is
Windows 10 1803 through roughly 2004/20H2.

## Compatibility

Output is compatible with Eric Zimmerman's WxTCmd (compatible fixtures verified against
captures from multiple hosts/users, checked in `crates/wxt-triage/tests/compat_wxtcmd.rs`).

One accepted, understood difference: WxTTriage's timestamps render as ISO-8601 with 7-digit
fractional seconds via `WinTimestamp` (e.g. `...:49.0000000Z`), while WxTCmd renders whole
seconds; the compat test suite normalizes this away before comparing, since both denote the
same instant.

## Flags

Input (exactly one required):

```
-d, --directory <DIR>         Recursively discover ActivitiesCache.db files under this directory
-f, --file <FILE>              Explicit ActivitiesCache.db file (repeatable)
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

WxTTriage options:

```
--dt <FORMAT>                  Datetime format (accepted for WxTCmd CLI parity only; output
                                is always ISO-8601 via WinTimestamp regardless of this value)
```

Diagnostics:

```
-q, --quiet                   Suppress per-file informational messages
--debug                       Emit debug-level diagnostics to stderr
--trace                       Emit trace-level diagnostics to stderr (implies --debug)
```

## Output layout

WxTTriage's scope is `UserSpecific`: every `ActivitiesCache.db` is attributed to a user
identity derived from its capture path (it normally lives under a per-user
`ConnectedDevicesPlatform` folder), and unattributable captures fall back to `users/unknown`
rather than a system-wide bucket. Following the suite's general output convention (see
JLETriage for the same pattern), the default (non-`--nested-output`) layout is:

```
<out>/
  WxTTriage_Activity_Output.csv                    # one row per Activity row
  WxTTriage_Activity_Output.json
  WxTTriage_ActivityOperation_Output.csv            # one row per ActivityOperation row
  WxTTriage_ActivityOperation_Output.json
  WxTTriage_Activity_PackageId_Output.csv           # one row per Activity_PackageId row
  WxTTriage_Activity_PackageId_Output.json
```

or, with `--nested-output` (legacy layout, mirroring the per-user attribution):

```
<out>/
  WxTTriage/
    users/
      <username>/
        WxTTriage_Activity_Output.csv
        WxTTriage_Activity_Output.json
        WxTTriage_ActivityOperation_Output.csv
        WxTTriage_ActivityOperation_Output.json
        WxTTriage_Activity_PackageId_Output.csv
        WxTTriage_Activity_PackageId_Output.json
      unknown/
        ... (same files, for captures with no derivable user identity)
```

JSON output is NDJSON (one record per line) for all three datasets, per the suite convention
noted in `crates/wxt-triage/src/lib.rs`.

## Output datasets and fields

### Activity (22 columns)

One row per row of the `Activity` table.

| # | Field | Notes |
|---|-------|-------|
| 1 | `Id` | GUID identity column; decoded from a 16-byte BLOB (.NET `Guid.ToString("D")` byte order) or taken as-is if already TEXT |
| 2 | `ActivityTypeOrg` | Raw integer `ActivityType` value |
| 3 | `ActivityType` | Named form: `ToastNotification` (2), `ExecuteOpen` (5), `InFocus` (6), `CloudClipboard` (10), `CopyPaste` (16); unknown values render as the raw integer |
| 4 | `Executable` | Derived from the `AppId` JSON array: prefers `windows_win32`/`x_exe_path`, then `windows_universal`, else the first entry; a leading `{GUID}` path segment is resolved via the shared GUID-mapping table |
| 5 | `DisplayText` | From the decoded `Payload` JSON |
| 6 | `ContentInfo` | From the decoded `Payload` JSON, built from `description` and a percent-decoded `contentUri` (with GUID substitution when the URI embeds one) |
| 7 | `Payload` | ASCII-decoded payload text, or the literal `(Binary data)` when it is not a JSON object |
| 8 | `ClipboardPayload` | ASCII-decoded `ClipboardPayload` BLOB/TEXT, empty when the source is empty |
| 9 | `StartTime` | `WinTimestamp`, from Unix epoch-seconds; 0/negative renders as empty |
| 10 | `EndTime` | `WinTimestamp`, same rule |
| 11 | `Duration` | `.NET TimeSpan` constant form `[d.]hh:mm:ss`; empty when end is absent, equal to start, or end's year is <= 1970 |
| 12 | `LastModifiedTime` | `WinTimestamp` |
| 13 | `LastModifiedOnClient` | `WinTimestamp` |
| 14 | `OriginalLastModifiedOnClient` | `WinTimestamp` |
| 15 | `ExpirationTime` | `WinTimestamp` |
| 16 | `CreatedInCloud` | `WinTimestamp` |
| 17 | `IsLocalOnly` | `.NET`-style bool, rendered `"True"`/`"False"` |
| 18 | `ETag` | Raw integer |
| 19 | `PackageIdHash` | |
| 20 | `PlatformDeviceId` | |
| 21 | `DevicePlatform` | From the decoded `Payload` JSON |
| 22 | `TimeZone` | From the decoded `Payload` JSON (`userTimezone`) |

### ActivityOperation (23 columns)

One row per row of the `ActivityOperation` table.

| # | Field | Notes |
|---|-------|-------|
| 1 | `Id` | Same GUID decoding as `Activity.Id` |
| 2 | `ActivityTypeOrg` | Raw integer |
| 3 | `ActivityType` | Same named mapping as `Activity.ActivityType` |
| 4 | `Executable` | Derived from `AppId`: prefers `windows_win32`/`x_exe_path`, else the first entry; unlike the Activity-table rule, the GUID-segment mapping is applied only when the chosen `Application` string contains `.exe` — otherwise the raw string is kept as-is |
| 5 | `DisplayText` | From decoded `Payload` |
| 6 | `ContentInfo` | From decoded `Payload` |
| 7 | `Payload` | ASCII-decoded, or `(Binary data)` |
| 8 | `ClipboardPayload` | ASCII-decoded |
| 9 | `StartTime` | `WinTimestamp` |
| 10 | `EndTime` | `WinTimestamp` |
| 11 | `Duration` | Same rule as Activity |
| 12 | `LastModifiedTime` | `WinTimestamp` |
| 13 | `LastModifiedTimeOnClient` | `WinTimestamp` |
| 14 | `CreatedTime` | `WinTimestamp` |
| 15 | `ExpirationTime` | `WinTimestamp` |
| 16 | `OperationExpirationTime` | `WinTimestamp` |
| 17 | `OperationOrder` | Raw integer |
| 18 | `AppId` | Raw `AppId` JSON text (unlike Activity, the raw column is kept alongside the derived `Executable`) |
| 19 | `OperationType` | Raw integer |
| 20 | `Description` | Always empty in the current implementation |
| 21 | `PlatformDeviceId` | |
| 22 | `DevicePlatform` | From decoded `Payload` |
| 23 | `TimeZone` | From decoded `Payload` |

### Activity_PackageId (5 columns)

One row per row of the `Activity_PackageId` table (an Activity can have several PackageId
rows, e.g. one per platform).

| # | Field | Notes |
|---|-------|-------|
| 1 | `Id` | The associated `ActivityId`, GUID-decoded the same way as `Activity.Id` |
| 2 | `Platform` | Renamed from the raw column: `windows_win32` -> `Win32`, `x_exe_path` -> `ExecutablePath`, `packageId` -> `Package`; anything else passes through unchanged |
| 3 | `Name` | Raw `PackageName` |
| 4 | `AdditionalInformation` | Populated only when `Name` contains `.exe` and its leading `\`-separated segment is a `{GUID}` resolvable via the shared GUID-mapping table (the GUID segment is then replaced with its description); empty otherwise |
| 5 | `Expires` | `WinTimestamp`, from `ExpirationTime`; 0/negative renders as empty |

## Examples

```
WxTTriage -d /mnt/triage --csv ./out --json ./out
WxTTriage -f "/mnt/triage/C/Users/alice/AppData/Local/ConnectedDevicesPlatform/L.alice/ActivitiesCache.db" --csv ./out
WxTTriage -d /mnt/triage --csv ./out --overwrite --debug
WxTTriage -d /mnt/triage --json ./out --pretty --nested-output
```
