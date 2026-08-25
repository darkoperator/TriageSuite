# SumETriage

SumETriage parses the Windows Server **System Usage Monitor (SUM)** database set: the
ESE-format `SystemIdentity.mdb` (server identity, the `RoleGuid`→`RoleName` map, and the
index of chained per-year databases) plus `Current.mdb` and its chained `.mdb` siblings,
which hold the per-role client-access detail (authenticated user, source IP, access counts
by day), DNS resolution history for those clients, first/last role-access timestamps, and
Automated VM Activation records for virtual machines that reported in. This is the role-
and license-usage tracking feature Microsoft ships on Windows Server for Automated VM
Activation and server-role usage auditing — not "Setup & Execution Monitor". The README's
tool-keys one-liner (`sum SumETriage (Setup & Execution Monitor parser)`) is an inaccurate
expansion of "SUM" and should be read as System Usage Monitor instead; the crate's own
module doc describes it as a "SUM / User Access Logging" parser, reflecting SUM's role in
the same license/usage-auditing feature family as Windows' User Access Logging (UAL).

## Target Windows versions

SUM databases are a Windows Server feature (Automated VM Activation / server-role usage
tracking), present from Windows Server 2012 onward — this is not a client-OS artifact.
SumETriage's ESE reader supports the classic page-format revisions used by older Server
builds as well as the modern **ESE revision-300** page layout introduced with Windows 11
24H2 and Windows Server 2025 (`triage-ese`'s header parser detects and accepts rev-300;
see `crates/triage-ese/src/header.rs` and `db.rs`). The tool's own compatibility fixtures
were captured from a rev-300 SUM database set (`SystemIdentity.mdb` + `Current.mdb` + two
chained per-GUID `.mdb` files, CleanShutdown), so both legacy and rev-300 SUM captures are
exercised in CI.

## Compatibility

Output is compatible with Eric Zimmerman's SumECmd (compatible fixtures verified, including
the modern ESE revision-300 layout).

## Flags

SumETriage takes only the common TriageSuite CLI arguments — the tool defines no
SUM-specific flags of its own:

```
Input (exactly one required):
  -d, --directory <DIR>   Velociraptor capture root or artifact directory to search
                           recursively for SystemIdentity.mdb
  -f, --file <FILE>       Explicit SystemIdentity.mdb path (repeatable)

Output (at least one required):
  --csv <DIR>              Write Zimmerman-compatible CSV output beneath this directory
  --json <DIR>             Write Zimmerman-compatible JSON (NDJSON) output beneath this directory
  --csvf <NAME>             Override the default CSV basename
  --jsonf <NAME>            Override the default JSON basename
  --pretty                  Pretty-print JSON (whitespace only; ignored for NDJSON framing)
  --overwrite                Allow replacement of existing output files
  --nested-output            Preserve the legacy nested output layout under <root>/<ToolName>/<identity>/

Diagnostics:
  -q, --quiet                Suppress per-file informational messages
  --debug                     Emit debug-level diagnostics to stderr
  --trace                     Emit trace-level diagnostics to stderr (implies --debug)
```

SumETriage discovers its input by matching the filename pattern `SystemIdentity.mdb`, and
automatically opens `Current.mdb` plus any chained `.mdb` files listed in
`SystemIdentity.mdb`'s `CHAINED_DATABASES` table from the same directory — you point it at
(or let it discover) `SystemIdentity.mdb`; the sibling databases do not need to be named
individually. If a listed chained database is not present alongside `SystemIdentity.mdb`,
it is skipped silently (matching SumECmd's behavior). If any opened database was not
cleanly shut down, SumETriage still parses it but prints a "not cleanly shut down (dirty)"
warning to stderr, noting that emitted records may be incomplete.

## Output layout

TriageSuite's default output layout is **flat**: every dataset file is written directly
under the `--csv`/`--json` root, with a 14-digit `<yyyyMMddHHmmss>_` run-stamp prefix and
the record's identity folded into the filename. SumETriage's `scope()` is `SystemWide`
(this is host/server-level data, not a per-user artifact), so the identity label is
`system`. Inferring the convention from `crates/triage-core/src/output/layout.rs` and the
dataset basenames in `crates/sum-triage/src/datasets.rs`, a default run produces names of
the shape:

```
<out>/
  system_<yyyyMMddHHmmss>_SumETriage_SystemIdentInfo_Output.csv
  system_<yyyyMMddHHmmss>_SumETriage_RoleInfos_Output.csv
  system_<yyyyMMddHHmmss>_SumETriage_ChainedDbInfo_Output.csv
  system_<yyyyMMddHHmmss>_SumETriage_Clients_Output.csv
  system_<yyyyMMddHHmmss>_SumETriage_ClientsDetailed_Output.csv
  system_<yyyyMMddHHmmss>_SumETriage_DnsInfo_Output.csv
  system_<yyyyMMddHHmmss>_SumETriage_RoleAccesses_Output.csv
  system_<yyyyMMddHHmmss>_SumETriage_VmInfo_Output.csv
  ... (and the matching .json / NDJSON files if --json is used)
```

Pass `--nested-output` to instead get the legacy tree layout
(`<out>/SumETriage/system/SumETriage_<Dataset>_Output.csv`). `--csvf`/`--jsonf` override the
basename portion only; the run-stamp and identity folding still apply.

## Output datasets and fields

Eight datasets are emitted, three from `SystemIdentity.mdb` (the SUMMARY pass) and five
from `Current.mdb` / each chained `.mdb` (the DETAIL pass, run once per database and
tagged with that database's `SourceFile`).

### SystemIdentInfo (from `SYSTEM_IDENTITY`)

`CreationTime`, `OsMajor`, `OsMinor`, `OsBuild`

### RoleInfos (from `ROLE_IDS`)

`RoleGuid`, `RoleName`, `ProductName`

### ChainedDbInfo (from `CHAINED_DATABASES`)

`Year`, `FileName`

### Clients (from `CLIENTS`, one row per client per role)

`RoleGuid`, `RoleDescription`, `AuthenticatedUserName`, `TotalAccesses`, `InsertDate`,
`LastAccess`, `IpAddress`, `ClientName`, `TenantId`, `SourceFile`

### ClientsDetailed (from `CLIENTS`, day-column expansion)

Each `CLIENTS` row can carry sparse `Day1`..`Day366` tagged columns holding a per-day
access count for that calendar year; SumETriage expands each populated day column into
its own row: `Date`, `Count`, `DayNumber`, `RoleGuid`, `RoleDescription`,
`AuthenticatedUserName`, `TotalAccesses`, `InsertDate`, `LastAccess`, `IpAddress`,
`ClientName`, `TenantId`, `SourceFile`

### DnsInfo (from `DNS`)

`HostName`, `Address`, `LastSeen`, `SourceFile`

### RoleAccesses (from `ROLE_ACCESS`)

`RoleGuid`, `RoleDescription`, `FirstSeen`, `LastSeen`, `SourceFile`

### VmInfo (from `VIRTUALMACHINES`)

`SerialNumber`, `CreationTime`, `LastSeenActive`, `BiosGuid`, `VmGuid`, `SourceFile`

All timestamp fields are decoded from either an ESE `DateTime` column or a 100ns-tick
Int64 FILETIME (both forms are seen across SUM database revisions), with `<= 0`/unset
values rendered as an empty timestamp. GUID columns are rendered in canonical lowercase
`Guid.ToString("D")` form to match SumECmd. `RoleDescription` is resolved by joining
`RoleGuid` against the identity-pass `ROLE_IDS` map; a role GUID with no match renders as
`(Unknown Role Guid)`.

## Examples

Parse a `SystemIdentity.mdb` found by directory scan, emitting CSV only:

```bash
SumETriage -d /mnt/triage/C/Windows/System32/LogFiles/Sum --csv ./out
```

Parse an explicit `SystemIdentity.mdb`, emitting both CSV and NDJSON:

```bash
SumETriage -f "/mnt/triage/C/Windows/System32/LogFiles/Sum/SystemIdentity.mdb" \
  --csv ./out --json ./out
```

Recursively triage a full Velociraptor capture, pretty-printed JSON, allowing overwrite of
a prior run's output:

```bash
SumETriage -d /mnt/triage --json ./out --pretty --overwrite
```

Reproduce the legacy nested output tree with verbose diagnostics:

```bash
SumETriage -d /mnt/triage --csv ./out --nested-output --debug
```
