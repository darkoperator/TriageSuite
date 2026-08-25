# Takajo

Takajo (鷹匠, "falconer" in Japanese) is an open-source, Nim-based results analyzer built by
Yamato Security for [Hayabusa](./Hayabusa.md) output. It ships roughly 30 subcommands — the
`stack-*` aggregation family, `timeline-*` views, `ttp-*` MITRE ATT&CK mapping, `html-report`,
VirusTotal lookups, and more — that all operate on a Hayabusa JSONL/CSV timeline, never on raw
evidence directly. TriageSuite does not drive any of those individually; it only invokes
Takajo's `automagic` subcommand, Takajo's own catch-all that runs as many of the relevant
analyses as it can in one pass and writes everything to a single output folder.

TriageSuite integrates Takajo as an **optional external tool** that the orchestrator chains
after a successful Hayabusa run, not as a TriageSuite-authored parser. It is invoked as a
subprocess, and its output is captured into the run manifest — nothing more.

## Relationship to TriageSuite

Takajo is a third-party, independently-developed tool maintained by Yamato Security. It is not
a TriageSuite parser: there is no `Tool` implementation for it in `crates/triage-core`, and it
never touches the `triage-orchestrator` per-file discovery/validate/parse loop that drives
in-process tools (`crates/triage-orchestrator/src/execute.rs`). Instead, TriageSuite orchestrates
Takajo as an external binary — resolving it, building its argv, running it as a subprocess, and
recording what happened — the same treatment given to Hayabusa itself.

Unlike every other document under `docs/tools/`, **no Eric Zimmerman compatibility claim applies
here.** Takajo has no Eric Zimmerman tool equivalent; it is a Hayabusa-ecosystem tool from a
different author (Yamato Security), and this document does not attempt to frame it as one.

## Why only `automagic`

Takajo ships around 30 subcommands covering individual analyses (per-field stacking, timeline
views, TTP/MITRE mapping, an HTML report server, VirusTotal lookups, etc.). Modeling each of
those as its own configurable invocation would require a large per-subcommand schema. Instead,
TriageSuite drives only `automagic` — Takajo's own built-in catch-all, described in its own
`--help` as running "as many commands as possible" against a Hayabusa results file and writing
all of it to one output folder. Driving `automagic` gets the bulk of Takajo's analytical value
with a config surface of exactly 4 fields (see below), instead of a bespoke schema per
subcommand. The other ~29 subcommands are out of scope; supporting them would need a separate,
later design.

## Dependency on Hayabusa

Takajo does not read raw evidence. `automagic -t` requires Hayabusa's JSONL timeline as input,
so Takajo can only run after a Hayabusa `json-timeline` invocation has produced one for that
host.

The orchestrator enforces this at two points:

- **Config-load time:** `takajo.enabled = true` requires `hayabusa.json = true`. Since both
  default to `true`, this holds with zero configuration. The only failure case is an *explicit*
  contradiction — `hayabusa.json = false` with `takajo.enabled = true` in the base table or an
  active profile — which is a hard config-load error (`ExternalConfig::resolve` returns
  `Err(ConfigError(...))` mentioning both `takajo` and `hayabusa.json`), not a silent skip.
- **Run time:** even with valid config, the orchestrator only chains Takajo if Hayabusa's
  `json-timeline` invocation actually produced a `.jsonl` file on disk for that host — checked
  with `out_file.is_file()`, not merely "the Hayabusa subprocess exited zero with no error." A
  Hayabusa run that reports success but, for whatever reason, leaves no JSONL file behind still
  causes Takajo to be skipped for that host, with a reported reason (see Manifest reporting
  below).

## Enabling it

Takajo needs no configuration to run: with no `[takajo]` table and no `--config` at all, the
orchestrator behaves as though `[takajo]` were present with every field at its default —
`enabled = true`. Concretely, per host, after Hayabusa's `json-timeline` invocation:

1. The orchestrator looks up the `takajo` binary — a bare name is resolved by scanning `PATH`
   for the first matching executable file; an explicit path (anything containing a path
   separator) is checked directly for existence (`crates/triage-orchestrator/src/external_bin.rs::resolve_bin`).
2. If the binary resolves *and* Hayabusa produced a JSONL file for that host (see above), Takajo
   `automagic` runs automatically — no flag is needed to opt in.
3. Set `bin` in the `[takajo]` table (or a profile overlay) to point at an explicit install
   location if `takajo` isn't on `PATH`.
4. Pass `--config <path.toml>` on `TriageSuite run` to supply the config file, and
   `--profile <name>` to select a named `[profiles.<name>.takajo]` overlay from it (`--profile`
   requires `--config` to also be given).

To disable Takajo outright, set `takajo.enabled = false` in the base `[takajo]` table or in an
active profile's `[profiles.<name>.takajo]` overlay.

## Configuration fields

The `[takajo]` table has exactly 4 fields, verified against the actual
`TakajoConfig` struct in `crates/triage-orchestrator/src/external_config.rs`:

| Field | Type | Default | Maps to |
|---|---|---|---|
| `bin` | string | `"takajo"` | binary name or path (bare name is looked up on `PATH`; a path containing a separator is checked directly) |
| `enabled` | bool | `true` | auto-run if the binary resolves **and** Hayabusa produced a JSONL timeline for the host; `false` disables even when both are true |
| `level` | string (optional) | unset | `--level` (omitted entirely from argv when unset) |
| `display_table` | bool | `false` | `--displayTable` (flag only appears in argv when `true`) |

An unset `level` means "don't pass `--level` at all," deferring to Takajo's own built-in
default rather than TriageSuite hard-coding a literal that could drift from Takajo's defaults
across versions. A `[profiles.<name>.takajo]` overlay (`TakajoOverlay`) may set any subset of
these 4 fields; unset overlay fields fall through to the base `[takajo]` table's values
(`TakajoConfig::merge_overlay`).

## Execution requirements discovered via live testing

Two real requirements were found by running the orchestrator against the actual `takajo`
binary (Takajo 2.16.1) — neither is documented in Takajo's own `--help` text in an obvious way,
and neither is captured in the earlier design spec (`docs/superpowers/specs/2026-08-22-hayabusa-takajo-config-design.md`),
which predates this testing. Both are handled automatically by the orchestrator; they matter to
you only if you plan to invoke Takajo by hand outside of TriageSuite.

1. **Working directory must be Takajo's own install directory.** Takajo checks that its own
   executable exists relative to the process's current working directory and refuses to run
   otherwise — regardless of the absolute paths passed via `-t`/`-o`. The orchestrator handles
   this by setting the subprocess's `current_dir` to the resolved binary's parent directory for
   *every* external tool it runs, not just Takajo (`invoke()` in
   `crates/triage-orchestrator/src/external.rs`: `if let Some(parent) = bin.parent() { cmd.current_dir(parent); }`).
   If you run `takajo automagic` manually, you must `cd` into Takajo's install directory first,
   or it will fail even with fully qualified `-t`/`-o` paths.

2. **`automagic -o` refuses to write into a directory that already exists**, failing with
   "Please specify a new folder name." Takajo creates its own leaf output directory and expects
   only the *parent* to exist beforehand. The orchestrator never pre-creates Takajo's own output
   folder (`<out>/<host>/Takajo/`) — it only `create_dir_all`s the host directory
   (`<out>/<host>/`) that folder will live under, and lets Takajo create the `Takajo/` leaf
   itself. If you run `automagic -o` manually, make sure the target directory does not already
   exist.

## Output layout

```
<out>/<host>/Takajo/
```

This directory is created by Takajo itself (see above), not pre-created by the orchestrator.
Its exact contents depend on what `automagic` finds relevant in the Hayabusa results for that
host — typically some combination of `stack-*.csv` aggregation files, `Timeline*.csv` views,
`TTPSummary.csv`, `Metrics*.csv`, and similar analysis outputs. The orchestrator does not
enumerate or validate individual files inside this directory; it treats the directory itself as
the output artifact and records it in the manifest only if it exists on disk after the
subprocess exits successfully.

## Manifest reporting

Each Takajo invocation attempt produces one `ExternalToolReport` entry
(`crates/triage-orchestrator/src/external.rs`):

```json
{
  "tool": "takajo-automagic",
  "found": true,
  "invoked": true,
  "exit_code": 0,
  "output_paths": ["<out>/<host>/Takajo"],
  "error": null
}
```

Field semantics:

- `tool` — always the literal `"takajo-automagic"`.
- `found` — whether the configured `bin` resolved to an executable file.
- `invoked` — whether the subprocess was actually spawned (`false` only if spawning itself
  failed, e.g. permission denied).
- `exit_code` — the process exit code, if the subprocess ran.
- `output_paths` — contains `<out>/<host>/Takajo` only if the subprocess exited successfully
  *and* that path exists on disk afterward; a zero exit code alone is not sufficient to claim
  the output.
- `error` — populated on failure with the subprocess's trimmed stderr (or a fallback
  `"exited with status ..."` message if stderr was empty), or with a spawn error message.

If `takajo.enabled = true` but Hayabusa did not produce a JSONL timeline for that host (binary
missing, `hayabusa.enabled = false`, `hayabusa.json = false` — which config validation normally
prevents when paired with `takajo.enabled = true` — or the JSONL file was simply absent after a
Hayabusa run), the reported entry looks like this instead:

```json
{
  "tool": "takajo-automagic",
  "found": true,
  "invoked": false,
  "exit_code": null,
  "output_paths": [],
  "error": "skipped: hayabusa did not produce a JSONL timeline for this host"
}
```

If the `takajo` binary itself cannot be resolved (not on `PATH`, and `bin` isn't a valid
explicit path), the entry instead reports `"tool": "takajo"`, `"found": false`,
`"invoked": false`, with no error message — the same `not_found()` shape used for Hayabusa.

## Example configuration

```toml
# triage.toml

[hayabusa]
bin = "hayabusa"
enabled = true
csv = true
json = true          # required: takajo.enabled = true needs hayabusa.json = true
rules = "./rules"
min_level = "informational"

[takajo]
bin = "takajo"
enabled = true
level = ""            # unset — defer to Takajo's own default
display_table = false

[profiles.quick.hayabusa]
min_level = "high"
proven_rules = true

[profiles.quick.takajo]
enabled = false        # skip Takajo entirely for a fast pass
```

The `quick` profile only overrides `takajo.enabled`; every other `[takajo]` field (`bin`,
`level`, `display_table`) is inherited unchanged from the base table via the additive
per-field overlay merge.

## Examples

```sh
# Auto-run Hayabusa + Takajo automagic with no config file at all
# (works if both `hayabusa` and `takajo` are on PATH)
TriageSuite run /path/to/capture --out ./triage-results

# Use a config file with explicit rule paths and Takajo settings
TriageSuite run /path/to/capture --out ./triage-results --config ./triage.toml

# Use the "quick" profile from that config, which disables Takajo
TriageSuite run /path/to/capture --out ./triage-results --config ./triage.toml --profile quick
```
