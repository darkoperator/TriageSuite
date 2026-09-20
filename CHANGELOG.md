# Changelog

All notable changes to TriageSuite are recorded here. Versions follow SemVer,
with the caveat that in `0.y.z` the **minor** carries what the major will carry
after 1.0: a `0.y` bump may break compatibility.

`run_manifest.json` carries its own `schema_version`, versioned against what
consumers parse rather than against the binary. It is **3** as of 0.3.0.

## [0.3.0] — 2026-09-20

Two features dominate: `run` writes a VeloProcessor-compatible output tree **by
default**, and every run emits a typed DuckDB view layer beside
`run_manifest.json`. It also corrects four browser decode tables that were
silently producing wrong values in 0.2.0.

### ⚠️ Breaking

- **The default output tree has changed.** `TriageSuite run` now writes the
  VeloProcessor category tree: per-category directories, per-user output under
  `PerUser/<Dataset>/`, and one merged file per dataset at the category root
  carrying a trailing `TriageUser` column — the single permitted divergence from
  Zimmerman column compatibility, and only on merged files. Anything that
  globbed 0.2.0's per-tool, per-identity tree will find nothing.
  `--layout native` restores the old shape.

### Added

- **DuckDB view layer.** Every run writes `<out>/duckdb/` — a `datasets.json`
  inventory of what the run published, and a `views.sql` generated as a pure
  function of it. Each dataset gets a raw view and a typed view over it, so a
  timestamp can be compared and ordered as a timestamp instead of a string. A
  two-host run defines over 300 views. See [docs/DuckDB.md](docs/DuckDB.md).
- Typed columns keep their original cell text in a `__text` companion, which is
  what distinguishes a value that was *absent* from one that *failed to
  convert*: a NULL typed value with non-NULL text is a conversion failure; both
  NULL means the cell was blank.
- `TriageSuite duckdb regenerate --out <root>` re-derives scan paths after a
  collection moves. It refuses an inventory it did not produce: `sql_type` is
  interpolated unquoted into a `TRY_CAST`, so a tampered `datasets.json` is
  untrusted input, and the refusal rewrites neither file.
- Column types are declared per tool, including for datasets whose id is built
  at run time — EvtxTriage's per-channel `Individual/` exports and RETriage's
  per-plugin registry output, which between them are the registry and event-log
  sides of an investigation.
- **Query templates for incident response** in `docs/DuckDB.md`, ordered the way
  an investigation runs: what evidence survived, whether it is intact, then
  hunting. Every query was executed against a real collection before being
  documented.
- Chain-of-custody records alongside the new output tree: `OutputHashes.txt`, a
  source hash log, a per-collection SysInfo report, per-tool process logs naming
  the file and reason for every parse, merge and router failure, and Timeline
  Explorer session files.
- A pre-flight validation gate runs per collection, so a folder of collector
  ZIPs is accepted and a bad archive is skipped on its own with its reason
  recorded in the run manifest.
- `TRIAGE_REQUIRE_DUCKDB=1` makes the DuckDB view assertions fail rather than
  skip when no `duckdb` binary is present. CI sets it and installs DuckDB, so
  the generated SQL is proven to load on every run.
- `TRIAGE_RUN_STAMP` pins the output run stamp for tests whose runs must land on
  the same filename. Values are restricted to a short alphanumeric token, since
  the stamp becomes part of a path.

### Fixed

- **Four BrowserTriage decode tables named the wrong thing**, each with a green
  test that asserted the table against the literal written above it. Every
  corrected table is now pinned to its upstream constant by a test citing the
  source. **If you ran BrowserTriage on 0.2.0, these columns were wrong:**
  - *Chromium page transitions* were shifted one bit position against
    `ui/base/page_transition_types.h` — a server redirect decoded as Client
    Redirect, an address-bar navigation as Forward Back. Redirect-chain
    reconstruction was wrong on any row carrying qualifier bits.
  - *Extension disable reasons* were built by doubling, ignoring the gaps where
    `extensions/browser/disable_reason.h` retired reasons — Blocked By Policy
    read as Unknown, Corrupted as Remote Install.
  - *Firefox download states* used Chromium's table, which agrees only on `1` —
    Failed read as Cancelled, Canceled as Interrupted.
  - *Firefox visit sources* claimed syncing and importing; the real values are
    Organic, Sponsored, Bookmarked, Searched. The column was never emitted, so
    no shipped output was affected.
- BrowserTriage no longer discards evidence silently: an unreadable
  `downloads_url_chains` says so in the row's `Notes` instead of leaving a blank
  origin; a failure in the first Firefox keyword source no longer discards every
  row of the second; the modern path's INNER JOIN no longer drops a term whose
  metadata row was cleared — the survived-a-deletion evidence the dataset exists
  for; and a file whose tables all fail is recorded as failed rather than as a
  successful parse of zero rows.
- BrowserTriage profile identification no longer requires a Windows container
  segment, so Chrome, Edge and Brave on one macOS or Linux host stop collapsing
  into a single identity.
- `webkit_or_time_t` called `.abs()` on an integer read straight from an
  evidence cell; `i64::MIN.abs()` overflows.
- Output-path defects found across three rounds of independent review: a merge
  that could adopt a previous run's output; a per-user filename decoder that
  could publish one tool's schema under another tool's name; a rejected input
  that left no manifest at all; and session definitions that loaded a merged
  file and the slices it was built from at once.
- `scripts/build-release.sh` raises its own open-file limit. macOS gives a login
  shell a soft `NOFILE` of 256 and linking through `zig cc` needs more, so the
  build died with `ProcessFdQuotaExceeded` — which reads like a build error and
  is not one.

### Changed

- Every decoder is swept over the values that break its arithmetic
  (`triage_testkit::boundary`).
- The clean clippy set is denied workspace-wide, with every `#[allow]` carrying
  a reason that `scripts/check-allow-justifications.sh` enforces.
- Fixed-size decode sites across 12 crates use `as_chunks::<N>()` rather than
  `chunks_exact(N)`, putting the length in the type so the reads below them
  cannot index out of bounds even in principle.
- FILETIME decoding moved out of the RECmd plugins into `triage-core`.
- Testing is local, behind `scripts/check.sh`: a hosted runner can only ever
  execute the half of this suite that needs no evidence tree.
- `triage-orchestrator` had a cleanup pass with no behavior change — same flags,
  same output tree, same manifest JSON, same exit codes.

### Known limitations

- MFTriage parses `$MFT`, `$J` and `$Boot`. `$Secure:$SDS` is collected by
  Velociraptor but not yet parsed; `$LogFile` is not parsed and is not parsed by
  MFTECmd either. There is no MFTECmd comparison harness for MFTriage yet, so
  its column compatibility is documented rather than proven.
- The capture-gated tests skip on hosted CI, which has no evidence tree. A green
  `PR checks` run proves the tree builds clean and the generated views load; it
  does not exercise the parsers against real evidence. `scripts/check.sh`
  locally does.

## [0.2.0] — 2026-08-31

Added **BrowserTriage**, a first-party browser artifact parser for
Chromium-family and Firefox-family browsers, and restructured the orchestrator's
external-tool support. See the
[0.2.0 release notes](https://github.com/darkoperator/TriageSuite/releases/tag/v0.2.0).

## [0.1.0]

Initial release.
