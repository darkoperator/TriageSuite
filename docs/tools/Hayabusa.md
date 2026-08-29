# Hayabusa

[Hayabusa](https://github.com/Yamato-Security/hayabusa) is an open-source, Rust-based Windows
Event Log (EVTX) fast forensics timeline generator and threat-hunting tool built by Yamato
Security. It scans a directory of `.evtx` files in a single pass against Sigma detection rules
— both native "Hayabusa rules" and community Sigma rules — and produces a scored timeline
(informational/low/medium/high/critical) plus a results summary of suspicious events.

TriageSuite integrates Hayabusa as an **optional external tool stage**: the orchestrator can
invoke the real `hayabusa` binary against a host's EVTX artifacts if it can find one (on `PATH`
or at a configured path), and folds its output paths and exit status into the run manifest.
Hayabusa is not a TriageSuite-authored parser — it is a separate, independently maintained
project that the orchestrator shells out to and chains into
[Takajo](https://github.com/Yamato-Security/takajo) (Hayabusa's companion results analyzer),
after a host's normal in-process TriageSuite tools have finished.

## Target Windows versions

Any Windows version that produces `.evtx` event logs — Windows Vista / Server 2008 and later
(the modern binary XML `.evtx` format). Hayabusa's own Sigma rule coverage may vary in relevance
by OS version and log source, but the tool itself operates on any standard `.evtx` file; it
imposes no additional version constraint of its own.

## Relationship to TriageSuite

Hayabusa is a third-party, independently developed tool (Yamato Security), not a TriageSuite
parser. TriageSuite orchestrates its execution — resolving the binary, building its argv,
running it, capturing its exit code and output paths into the run manifest — and nothing more.
It does not reimplement, wrap, or reinterpret Hayabusa's detection logic or output schema.

Unlike TriageSuite's own parsers (PETriage, RBTriage, LETriage, JLETriage, RETriage, etc.),
**Hayabusa has no Eric Zimmerman tool equivalent and no CSV/JSON compatibility claim applies to
it.** It is a category of tool (Sigma-rule EVTX threat hunting) that doesn't exist in the
Zimmerman toolset, so there is nothing to be compatible with — Hayabusa's own output format is
authoritative for its own output.

## Enabling it

No configuration is required by default. If a `hayabusa` binary is discoverable on `PATH`,
`TriageSuite run` auto-runs it for every host after that host's in-process tools finish. This
matches the rest of the orchestrator's default-on philosophy: a bare
`TriageSuite run <capture> --out <dir>` with no config file behaves exactly as if `[hayabusa]`
were present with every field at its default.

To customize behavior:

- Point at an explicit install location with the `bin` field (see below) if the binary isn't on
  `PATH`, or to pin a specific build.
- Use the `enabled`, `csv`, `json`, and `logon_summary` boolean fields to turn Hayabusa off
  entirely, or to select which of its three invocation modes run.
- Supply a TOML config file and, optionally, a named profile on the CLI:

```bash
TriageSuite run <capture> --out <dir> --config ./triage.toml --profile quick
```

`--config <path>` points at the TOML file containing `[hayabusa]` (and `[takajo]`) tables and
any `[profiles.<name>.*]` overlays. `--profile <name>` selects a named overlay to apply on top
of the base `[hayabusa]`/`[takajo]` tables; it requires `--config` to also be given (the
orchestrator CLI rejects `--profile` without `--config`). Precedence, lowest to highest: built-in
field defaults -> `[hayabusa]` base table -> the selected `[profiles.<name>.hayabusa]` overlay.
There is no separate CLI-flag override layer above the profile for individual Hayabusa fields —
`--config`/`--profile` is the whole mechanism.

## Configuration fields

All fields live under a `[hayabusa]` table and are optional. An unset field means "don't pass
that flag" — Hayabusa's own built-in default applies, not a TriageSuite-duplicated literal, so
the schema doesn't drift when Hayabusa's own defaults change across versions. Verified directly
against `crates/triage-orchestrator/src/external_config.rs::HayabusaConfig`.

| Field | Type | Default | Maps to |
|---|---|---|---|
| `bin` | string | `"hayabusa"` | binary name/path (PATH lookup if bare, i.e. no path separator) |
| `enabled` | bool | `true` | auto-run if found; `false` disables even when present |
| `csv` | bool | `true` | run `dfir-timeline --output-type csv` |
| `json` | bool | `true` | run `dfir-timeline --output-type jsonl` — required if `[takajo]` is enabled |
| `logon_summary` | bool | `true` | run `logon-summary` (successful/failed logon summary CSVs) |
| `rules` | string (optional) | unset | `--rules` (`dfir-timeline` only) |
| `rules_config` | string (optional) | unset | `--rules-config` (`dfir-timeline` only) |
| `min_level` | string (optional) | unset | `--min-level` (`dfir-timeline` only) |
| `profile` | string (optional) | unset | `--profile` (`dfir-timeline` only — Hayabusa's own output-profile flag, see note below) |
| `threads` | integer (`u32`, optional) | unset | `--threads` (`dfir-timeline` and `logon-summary`) |
| `clobber` | bool | `false` | `--clobber` (`dfir-timeline` and `logon-summary`) |
| `sort` | bool | `false` | `--sort` (`dfir-timeline` only) |
| `scan_all_evtx_files` | bool | `false` | `--scan-all-evtx-files` (`dfir-timeline` only) |
| `enable_all_rules` | bool | `false` | `--enable-all-rules` (`dfir-timeline` only) |
| `enable_noisy_rules` | bool | `false` | `--enable-noisy-rules` (`dfir-timeline` only) |
| `enable_deprecated_rules` | bool | `false` | `--enable-deprecated-rules` (`dfir-timeline` only) |
| `enable_unsupported_rules` | bool | `false` | `--enable-unsupported-rules` (`dfir-timeline` only) |
| `proven_rules` | bool | `false` | `--proven-rules` (`dfir-timeline` only) |
| `time_offset` | string (optional) | unset | `--time-offset` (`dfir-timeline` and `logon-summary`) |
| `timeline_start` | string (optional) | unset | `--timeline-start` (`dfir-timeline` and `logon-summary`) |
| `timeline_end` | string (optional) | unset | `--timeline-end` (`dfir-timeline` and `logon-summary`) |

`logon-summary` is not sigma-rule-based, so the `dfir-timeline`-only fields above are never
passed to it, regardless of whether they're set — confirmed against the real Hayabusa 4.0.0
binary's `logon-summary --help`, which doesn't list them (and errors on unrecognized flags).

All 21 fields above were checked field-by-field against the current
`HayabusaConfig` struct and its `Default` impl and match the design spec exactly — no
discrepancies found between the spec doc and the code.

**Note on `profile`:** this field name collides in spelling, but not in meaning, with the
orchestrator's own `--profile <name>` CLI flag. The `[hayabusa].profile` TOML field maps to
Hayabusa's *own* `--profile` flag (an output-formatting profile Hayabusa understands natively,
e.g. controlling which columns/fields its timeline includes). The orchestrator's `--profile`
flag selects a `[profiles.<name>.hayabusa]` *overlay* in the TriageSuite config file. They are
unrelated settings that happen to share a name at two different layers.

An empty string for any optional string field (e.g. `min_level = ""`) is treated as unset — no
flag is emitted for it.

## Always-on flags (not configurable)

Every `dfir-timeline` invocation always receives these flags, hardcoded in
`crates/triage-orchestrator/src/external_args.rs`'s `HARDCODED_FLAGS` constant — they are
never TOML fields and cannot be turned off:

- **`--no-wizard`** — Hayabusa's interactive rule-selection wizard would hang an automated,
  non-interactive run. `logon-summary` has no wizard mode and rejects this flag outright
  (`error: unexpected argument '--no-wizard' found`, confirmed against the real 4.0.0
  binary), so it's excluded there — see `LOGON_SUMMARY_HARDCODED_FLAGS`.
- **`--quiet`**, **`--no-color`** — clean output suitable for logs/CI, since the orchestrator
  never runs Hayabusa attached to a TTY. Both accepted by `logon-summary` too.
- **`--iso-8601`** — timestamps are always ISO-8601, and per Hayabusa's own `--help` text this
  also always implies UTC (`-U/--utc` is therefore redundant once `--iso-8601` is hardcoded, and
  is not exposed as a separate field). Accepted by `logon-summary` too. (Hayabusa >= 4.0
  lowercased all long option names; this was `--ISO-8601` before.)

In addition, `--output-type` selects the format on the shared `dfir-timeline` command (Hayabusa
>= 4.0 merged the old separate `csv-timeline`/`json-timeline` commands into one):

- **`--output-type csv`** on the `csv`-triggered invocation.
- **`--output-type jsonl`** on the `json`-triggered invocation — never plain `json`, because the
  downstream Takajo `automagic -t` step expects a `.jsonl` file, not a single JSON document. (The
  old `json-timeline -L/--JSONL-output` flag no longer exists.)

Input and output paths (`--directory` and `--output`) are likewise orchestrator-managed, not
configurable — the orchestrator points `--directory` at the host's artifact root and
`--output` at its own nested output tree for that host. For `logon-summary`, `--output` is a
filename *prefix*, not a single file (see Execution model below).

## Execution model

Per host, after that host's normal in-process TriageSuite tools finish, the orchestrator invokes
Hayabusa up to three times (`crates/triage-orchestrator/src/external.rs::run_external_tools_for_host`):

1. If `hayabusa.enabled` and the binary resolves, and `csv = true`: run
   `hayabusa dfir-timeline --directory <host artifact root> --output-type csv --no-wizard
   --quiet --no-color --iso-8601 [shared rule/filter flags] --output
   <out>/<host>/Hayabusa/timeline.csv`.
2. If `hayabusa.enabled` and the binary resolves, and `json = true`: run
   `hayabusa dfir-timeline --directory <host artifact root> --output-type jsonl --no-wizard
   --quiet --no-color --iso-8601 [shared rule/filter flags] --output
   <out>/<host>/Hayabusa/timeline.jsonl`.
3. If `hayabusa.enabled` and the binary resolves, and `logon_summary = true`: run
   `hayabusa logon-summary --directory <host artifact root> --quiet --no-color --iso-8601
   [--threads/--clobber/--time-offset/--timeline-start/--timeline-end if set] --output
   <out>/<host>/Hayabusa/logon-summary`.

The two `dfir-timeline` invocations share every rule-selection/filter flag (`rules`,
`rules_config`, `min_level`, `profile`, `threads`, `clobber`, `sort`, `scan_all_evtx_files`,
`enable_*`, `proven_rules`, `time_offset`, `timeline_start`, `timeline_end`), differing only in
`--output-type` and the output file extension. `logon-summary` is not sigma-rule-based, so it
only receives the small subset of flags it actually documents — see the Configuration fields
table above for exactly which.

**Binary resolution:** `bin` is resolved via `resolve_bin()` in
`crates/triage-orchestrator/src/external_bin.rs` — an explicit path (containing a path
separator) is checked directly as a file; a bare name (e.g. the default `"hayabusa"`) is looked
up across every directory in `PATH`, first match wins. If nothing resolves, the tool is reported
as not found and no invocation is attempted.

**Working directory requirement:** every external-tool invocation, Hayabusa included, is run
with its process working directory set to the resolved binary's own parent directory
(`cmd.current_dir(parent)`), not the orchestrator's own working directory or the host's output
directory. This was discovered as a real requirement during live testing against the actual
Takajo binary (2.16.1), which checks that its own executable exists relative to the process's
current working directory and refuses to run otherwise, regardless of the absolute paths passed
via its own flags. Applying the same cwd convention to every external tool (Hayabusa included)
is a safe general default even though Hayabusa itself was not observed to require it.

**Output paths:** `<out>/<host>/Hayabusa/timeline.csv` (csv), `<out>/<host>/Hayabusa/timeline.jsonl`
(json), and `<out>/<host>/Hayabusa/logon-summary-successful.csv` /
`<out>/<host>/Hayabusa/logon-summary-failed.csv` (logon-summary — confirmed exact suffixes
against the real 4.0.0 binary; neither file is written at all if no logon events are found in
the scanned logs), under the same per-host output root as every other TriageSuite tool's nested
layout. The orchestrator creates the `Hayabusa/` directory itself before each invocation, and for
`logon-summary` discovers the actual file(s) written by listing that directory for names starting
with the given prefix rather than assuming the exact suffixes (`files_with_prefix()` in
`external.rs`) — a deliberate hedge against Hayabusa changing them in a future release, the same
class of drift that broke `csv-timeline`/`json-timeline` in 4.0.

**Chaining to Takajo:** if `takajo.enabled` and Hayabusa's `json`-triggered `dfir-timeline`
invocation actually
produced a `.jsonl` file on disk (checked with a real filesystem existence check, not merely a
zero exit code), the orchestrator runs `takajo automagic -t <that .jsonl file> -o
<out>/<host>/Takajo/ [--level] [--displayTable]` immediately after. If `takajo.enabled` but
Hayabusa produced no JSONL for that host (disabled, not found, or failed), a report is still
recorded explaining the skip rather than silently omitting Takajo from the manifest. Config-time
validation additionally rejects `takajo.enabled = true` combined with an explicit
`hayabusa.json = false`, since that combination can never be satisfied.

## Manifest reporting

Each Hayabusa invocation attempted contributes one `ExternalToolReport` entry
(`crates/triage-orchestrator/src/external.rs`) to the run manifest, shaped as:

```json
{
  "tool": "hayabusa-csv",
  "found": true,
  "invoked": true,
  "exit_code": 0,
  "output_paths": ["<out>/<host>/Hayabusa/timeline.csv"],
  "error": null
}
```

- `tool` — `"hayabusa-csv"`, `"hayabusa-json"`, and `"hayabusa-logon-summary"` for the three
  invocation modes (a bare `"hayabusa"` entry is used only for the not-found case, before any
  invocation is attempted).
- `found` — whether `resolve_bin("hayabusa")`/the configured `bin` resolved to an executable at
  all.
- `invoked` — whether the subprocess was actually started (`false` if the OS failed to spawn it,
  e.g. permission denied).
- `exit_code` — the process's raw exit code, if it ran to completion.
- `output_paths` — populated only if the process exited successfully *and* the expected output
  file exists on disk; a zero exit code alone does not guarantee this and is not treated as
  sufficient.
- `error` — non-null only on failure: stderr (trimmed) if the process ran and failed, the exit
  status if stderr was empty, or the OS spawn error if the process never started. Omitted
  entirely from the JSON when null.

Takajo's chained `automagic` step reports under `"takajo-automagic"` in the same shape,
including the explicit `"skipped: hayabusa did not produce a JSONL timeline for this host"`
error case when Hayabusa didn't hand it usable input.

## Console reporting

Every external-tool invocation also prints a line to stderr as it finishes
(`ProgressUi::external_tool_finished`, always shown regardless of `--no-progress` — this is not
a decoration) instead of only landing silently in `run_manifest.json`:

```
✔ hayabusa-csv -> /out/HOSTX/Hayabusa/timeline.csv
✔ hayabusa-json -> /out/HOSTX/Hayabusa/timeline.jsonl
✔ hayabusa-logon-summary -> /out/HOSTX/Hayabusa/logon-summary-successful.csv, /out/HOSTX/Hayabusa/logon-summary-failed.csv
✔ takajo-automagic -> /out/HOSTX/Takajo
```

or, on failure or when the binary can't be found:

```
✘ hayabusa-json exited with status Some(1)
○ takajo not found on PATH, skipped
```

`✔`/`✘` match the same glyphs and coloring used for TriageSuite's own in-process tools; `○` is
specific to external tools, since "not found on PATH" is a state in-process `Tool`s never have.

## Example configuration

```toml
# triage.toml — optional; a bare `TriageSuite run <capture> --out <dir>` works with none of this.

[hayabusa]
bin = "hayabusa"
enabled = true
csv = true
json = true
logon_summary = true
rules = "./rules"
rules_config = "./rules/config"
min_level = "informational"
threads = 4
clobber = false
sort = false
scan_all_evtx_files = false
enable_all_rules = false
enable_noisy_rules = false
enable_deprecated_rules = false
enable_unsupported_rules = false
proven_rules = false

[takajo]
bin = "takajo"
enabled = true
display_table = false

# A fast pass: only high-confidence, high-severity hits, no Takajo post-processing.
[profiles.quick.hayabusa]
min_level = "high"
proven_rules = true

[profiles.quick.takajo]
enabled = false

# A slower, broader sweep for a deeper hunt: everything Hayabusa's rule set can flag.
[profiles.full-hunt.hayabusa]
enable_all_rules = true
enable_noisy_rules = true
# no timeline_start/timeline_end — full-hunt scans everything available;
# real compromises can predate an arbitrary cutoff by a year or more.
```

## Examples

Run a capture with Hayabusa auto-detected on `PATH`, no config file needed:

```bash
TriageSuite run /mnt/triage --out ./results
```

Run with an explicit config file, applying the `quick` profile for a fast, high-confidence pass:

```bash
TriageSuite run /mnt/triage --out ./results --config ./triage.toml --profile quick
```

Run a deep hunt against a folder of multi-host captures, chaining into Takajo automatically
since `hayabusa.json` and `takajo.enabled` both default to `true`:

```bash
TriageSuite run /evidence/captures --out ./results --config ./triage.toml --profile full-hunt --json
```
