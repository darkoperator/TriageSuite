use crate::capture::HostCapture;
use crate::registry::{ToolEntry, ToolOptions};
use crate::MAX_REASON_SAMPLES;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Instant;
use triage_cli::progress::NullProgress;
use triage_core::error::RunExit;
use triage_core::output::layout::OutputLayoutMode;
use triage_core::output::router::{OutputRouter, RouterOptions};
use triage_core::tool::{ResourceClass, Validation};

/// Single recursive walk over `root` matching the union of all selected
/// tools' filename globs. One walk feeds every tool.
pub struct DiscoveryIndex {
    pub candidates: HashMap<String, Vec<PathBuf>>,
    pub inaccessible: u64,
}

pub fn build_index(root: &Path, tools: &[ToolEntry], exclude: &[PathBuf]) -> DiscoveryIndex {
    let plans: Vec<(&str, GlobSet)> = tools
        .iter()
        .map(|entry| (entry.key, glob_set(entry.tool.patterns())))
        .collect();
    let mut candidates: HashMap<String, Vec<PathBuf>> = tools
        .iter()
        .map(|entry| (entry.key.to_string(), Vec::new()))
        .collect();
    let inaccessible =
        triage_core::discovery::walk_files(root, exclude, &mut |_| {}, &mut |path| {
            let Some(name) = path.file_name() else { return };
            let lower = name.to_string_lossy().to_lowercase();
            for (key, set) in &plans {
                if set.is_match(&lower) {
                    if let Some(files) = candidates.get_mut(*key) {
                        files.push(path.to_path_buf());
                    }
                }
            }
        });
    for files in candidates.values_mut() {
        files.sort();
    }
    DiscoveryIndex {
        candidates,
        inaccessible,
    }
}

/// Case-insensitive filename matcher for one tool's patterns. The patterns are
/// static strings the tool ships with, so one that fails to compile is a
/// programming error, not a runtime condition.
fn glob_set(patterns: &[&'static str]) -> GlobSet {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        b.add(Glob::new(&p.to_lowercase()).expect("tool pattern must be a valid glob"));
    }
    b.build().expect("tool patterns must build a glob set")
}

/// Where a `run_tool_on_host` invocation should write its output, and
/// whether it may overwrite existing files.
pub struct OutputOpts {
    pub csv_root: Option<PathBuf>,
    pub json_root: Option<PathBuf>,
    pub overwrite: bool,
    pub run_id: String,
    /// Per-run switches that change how individual tools are constructed
    /// (`--hunt`, `--no-timeline`), carried here because the worker threads
    /// rebuild each tool themselves.
    pub tools: ToolOptions,
    /// `--layout`: `Velo` (default) builds `Processed-<HOST>-<stamp>/<Category>`
    /// roots; `Native` keeps the per-tool, per-identity tree.
    pub layout: crate::velo::Layout,
    /// The Velo run stamp (`yyyy-MM-ddTHHmmssZ`), computed once per run via
    /// `velo_run_stamp()` so every host/tool in one invocation shares it.
    /// Unused under `Layout::Native`.
    pub velo_stamp: String,
    /// `--skip-hashes`: opt out of SHA256-ing every generated output file.
    /// Hashing re-reads every output file, which is real wall-clock cost on
    /// an MFT-sized capture. Unused under `Layout::Native`.
    pub skip_hashes: bool,
    /// The honesty statement (`crate::time_range_notice`) computed once for
    /// the whole run when `--start`/`--end` was given, `None` otherwise.
    /// Written into every tool's process log so a filtered run cannot look
    /// uniformly scoped when only EvtxTriage/Hayabusa actually filtered.
    pub time_range_notice: Option<String>,
}

/// Structured outcome of running one tool over one host's file index.
pub struct ToolRunResult {
    pub key: String,
    pub binary_name: String,
    pub files_matched: u64,
    pub supported: u64,
    pub unsupported: u64,
    pub corrupt: u64,
    pub unreadable: u64,
    pub deduplicated: u64,
    pub reason_samples: Vec<String>,
    pub parsed: u64,
    pub failed: u64,
    pub records: u64,
    /// The files this tool's `OutputRouter` published this run
    /// (`OutputRouter::finish`'s `FinishReport::published`) -- destinations
    /// whose staged file was renamed into place, not destinations that were
    /// merely opened and not paths that merely exist. A failed run can
    /// legitimately leave this empty while files from an earlier run still
    /// sit at those paths, which is precisely why the Velo merge post-pass
    /// is given this list rather than testing the filesystem.
    pub output_paths: Vec<PathBuf>,
    /// Category-level files produced by the Velo per-user merge post-pass
    /// (`crate::velo::merge`), kept separate from `output_paths` (the
    /// router's own per-user files) so callers can tell merged output from
    /// primary output; the manifest reports both together.
    pub merged: Vec<PathBuf>,
    pub error: Option<String>,
    pub exit: Option<RunExit>,
}

impl ToolRunResult {
    /// An empty result for `key`: every count zero, nothing failed.
    pub fn new(key: impl Into<String>, binary_name: impl Into<String>) -> Self {
        ToolRunResult {
            key: key.into(),
            binary_name: binary_name.into(),
            files_matched: 0,
            supported: 0,
            unsupported: 0,
            corrupt: 0,
            unreadable: 0,
            deduplicated: 0,
            reason_samples: Vec::new(),
            parsed: 0,
            failed: 0,
            records: 0,
            output_paths: Vec::new(),
            merged: Vec::new(),
            error: None,
            exit: None,
        }
    }

    /// A result for a tool that never ran at all, carrying `message` as both
    /// its run-level error and its only reason sample.
    pub fn fatal(key: impl Into<String>, binary_name: impl Into<String>, message: String) -> Self {
        ToolRunResult {
            reason_samples: vec![message.clone()],
            error: Some(message),
            exit: Some(RunExit::Fatal),
            ..Self::new(key, binary_name)
        }
    }

    /// Record why one file was not parsed. Keeps the first few, so a run over
    /// thousands of unsupported files still produces a readable manifest.
    fn note(&mut self, path: &Path, reason: impl std::fmt::Display) {
        if self.reason_samples.len() < MAX_REASON_SAMPLES {
            self.reason_samples
                .push(format!("{}: {reason}", path.display()));
        }
    }

    /// Record that the Velo per-user merge post-pass failed for one dataset
    /// stem. Increments `failed` — a merge failure is not ancillary the way
    /// an external-tool failure or a skipped archive is (`docs/tools/
    /// TriageSuite.md`): it operates on this tool's own already-parsed
    /// primary output, using the same header-integrity check that makes a
    /// mismatch a hard error inside `merge_per_user` itself, so recording it
    /// here and then reporting success at the process-exit level would
    /// contradict that. This flows into `aggregate_exit`'s documented 5
    /// (mixed) / 6 (all failed) and flips `progress_ui::summary_line` to the
    /// failure line automatically.
    ///
    /// Deliberately does not touch `error`/`exit`: those are reserved for
    /// abort-worthy failures, and setting them here would break the
    /// per-dataset isolation `run_tool_on_host`'s merge loop relies on — one
    /// dataset's merge failure must not stop another dataset's merge from
    /// running or being reported.
    fn note_merge_failure(&mut self, stem: &str, reason: impl std::fmt::Display) {
        self.failed += 1;
        if self.reason_samples.len() < MAX_REASON_SAMPLES {
            self.reason_samples
                .push(format!("merge of {stem}: {reason}"));
        }
    }
}

/// Apply the workspace-wide aggregate exit semantics after all applicable
/// artifacts have run.
pub fn aggregate_exit(successful: u64, failed: u64, terminal: Option<RunExit>) -> RunExit {
    if let Some(exit @ (RunExit::Usage | RunExit::InputMissing | RunExit::OutputFailure)) = terminal
    {
        return exit;
    }
    if failed == 0 {
        RunExit::Success
    } else if successful > 0 {
        RunExit::Partial
    } else {
        RunExit::Fatal
    }
}

/// Run a single tool over a single host's shared discovery index. Filters
/// `index` down to this tool's files, confirms each with `tool.validate()`,
/// builds a per-host `OutputRouter` rooted at `<csv_root>/<output_id>` (and
/// likewise for `json_root`), drives parsing via
/// `triage_cli::runner::parse_validated` with a `NullProgress` (the
/// orchestrator owns its own progress rendering across hosts/tools, not
/// per-call), then flushes the router.
///
/// Under `--layout native` (`OutputOpts::layout == Layout::Native`), uses
/// `OutputLayoutMode::Nested` (`<root>/<BinaryName>/<identity>/...`) rather
/// than the CLI's default Flat layout: the orchestrator already fans output
/// out per-host, so nesting per-tool underneath that keeps a multi-tool,
/// multi-host run's output tree legible (`<csv_root>/<host>/<Tool>/...`)
/// instead of dumping every tool's identity-stamped files into one shared
/// per-host directory. Under the default `--layout velo`, uses
/// `OutputLayoutMode::Velo` rooted at `Processed-<HOST>-<stamp>/<Category>`
/// (`crate::velo`), matching VeloProcessor's output tree.
///
/// Early-returns (no router built, no output directory created — no process
/// log either, for the same reason) when no files matched — this keeps a
/// no-op tool from creating an empty output tree for every host.
pub fn run_tool_on_host(
    entry: &ToolEntry,
    host: &HostCapture,
    index: &DiscoveryIndex,
    out: &OutputOpts,
) -> ToolRunResult {
    let start = Instant::now();
    let tool = entry.tool.as_ref();
    let candidates = index.candidates.get(entry.key).cloned().unwrap_or_default();
    // Checked before the validation loop consumes `candidates`, and used
    // below instead of `files.is_empty()` after that loop: those are two
    // different conditions. Zero candidates means this tool found nothing
    // to say at all, and creating an output tree (even just a log) for that
    // is the noise the "no output directory for a no-op tool" contract
    // exists to prevent. Candidates that all fail validation is the
    // opposite case -- "found N files, every one corrupt/unsupported" is
    // exactly the diagnosis an analyst goes looking for a process log for --
    // so that case must still get one, even though `files` (the
    // *validated* set) ends up empty too.
    let no_candidates_at_all = candidates.is_empty();
    let mut result = ToolRunResult::new(entry.key, tool.binary_name());
    result.files_matched = candidates.len() as u64;

    // Where this tool's process log would live under `--layout velo`, or
    // `None` under `--layout native` (`crate::velo::collection_dir_for`,
    // shared with `main.rs` so the two call sites can't disagree on the
    // gating condition or the path).
    let collection_dir = out
        .csv_root
        .as_ref()
        .or(out.json_root.as_ref())
        .and_then(|root| {
            crate::velo::collection_dir_for(out.layout, root, &host.output_id, &out.velo_stamp)
        });

    // Discovery/validation lines are buffered rather than written straight
    // to a `ProcessLog`: opening one means `create_dir_all`-ing the
    // collection directory, and the `files.is_empty()` guard below promises
    // no output directory gets created for a no-op tool. Buffering keeps
    // that promise — the log (with everything gathered here) is only
    // actually opened once we know this tool is producing output at all.
    let mut log_lines: Option<Vec<String>> = collection_dir.is_some().then(Vec::new);
    let mut log_line = |line: String| {
        if let Some(lines) = log_lines.as_mut() {
            lines.push(line);
        }
    };
    // Written before anything else in the log: an analyst reading this
    // tool's own process log must see the same honesty statement as the run
    // summary and SysInfo report, not just infer it from `time_filter` in
    // the manifest.
    if let Some(notice) = &out.time_range_notice {
        log_line(notice.clone());
    }
    log_line(format!(
        "discovered {} candidate files",
        result.files_matched
    ));

    let mut files = Vec::new();
    for path in candidates {
        match tool.validate(&path) {
            Validation::Supported => {
                result.supported += 1;
                files.push(path);
            }
            Validation::Unsupported { reason } => {
                result.unsupported += 1;
                let reason = reason.to_string();
                log_line(format!("unsupported: {} — {reason}", path.display()));
                result.note(&path, reason);
            }
            Validation::Corrupt { reason } => {
                result.corrupt += 1;
                result.failed += 1;
                let reason = reason.to_string();
                log_line(format!("corrupt: {} — {reason}", path.display()));
                result.note(&path, reason);
            }
            Validation::Unreadable { error } => {
                result.unreadable += 1;
                result.failed += 1;
                let error = error.to_string();
                log_line(format!("unreadable: {} — {error}", path.display()));
                result.note(&path, error);
            }
        }
    }
    // Content dedupe is per-tool policy, not universal: a tool whose output
    // reports *where* an artifact was found needs both copies. See
    // `Tool::dedupe_by_content`.
    if tool.dedupe_by_content() {
        let mut dedupe = triage_core::dedupe::DedupeSet::new();
        files.retain(|path| match dedupe.insert(path) {
            Ok(true) => true,
            Ok(false) => {
                result.deduplicated += 1;
                log_line(format!("deduplicated (content match): {}", path.display()));
                false
            }
            Err(error) => {
                result.unreadable += 1;
                result.failed += 1;
                let error = error.to_string();
                log_line(format!(
                    "unreadable during dedupe: {} — {error}",
                    path.display()
                ));
                result.note(path, error);
                false
            }
        });
    }
    // Opens the process log (creating the collection directory as a side
    // effect) and flushes everything buffered above into it. Opening is
    // itself best-effort — a failure here (e.g. an unwritable output root)
    // must not stop the tool from running; it only means this run's process
    // log is missing, and the manifest remains the authoritative record
    // either way.
    let open_and_flush_log = || {
        let mut log = collection_dir.as_deref().and_then(|dir| {
            crate::velo::proclog::ProcessLog::open(dir, tool.binary_name(), out.overwrite).ok()
        })?;
        if let Some(lines) = log_lines.as_ref() {
            for line in lines {
                log.line(line);
            }
        }
        Some(log)
    };

    if files.is_empty() {
        // `candidates` was non-empty but every one failed validation (or was
        // deduplicated away) -- unlike the true zero-candidate case, this
        // one still gets a log: see the comment on `no_candidates_at_all`
        // above.
        if !no_candidates_at_all {
            if let Some(log) = open_and_flush_log() {
                log.finish_with_counts(
                    result.files_matched,
                    result.parsed,
                    result.failed,
                    result.records,
                    start.elapsed(),
                );
            }
        }
        return result;
    }

    let mut proc_log = open_and_flush_log();

    let (csv_root, json_root, layout_mode) = match out.layout {
        crate::velo::Layout::Velo => {
            let category = crate::velo::category_for_key(entry.key);
            (
                out.csv_root
                    .as_ref()
                    .map(|r| crate::velo::velo_root(r, &host.output_id, &out.velo_stamp, category)),
                out.json_root
                    .as_ref()
                    .map(|r| crate::velo::velo_root(r, &host.output_id, &out.velo_stamp, category)),
                OutputLayoutMode::Velo,
            )
        }
        crate::velo::Layout::Native => (
            out.csv_root.as_ref().map(|r| r.join(&host.output_id)),
            out.json_root.as_ref().map(|r| r.join(&host.output_id)),
            OutputLayoutMode::Nested,
        ),
    };

    // Cloned rather than moved: the merge post-pass below (after
    // router.finish()) needs the same category roots to find the PerUser/
    // files the router is about to write.
    let router_opts = RouterOptions {
        csv_root: csv_root.clone(),
        json_root: json_root.clone(),
        csvf: None,
        jsonf: None,
        pretty: false,
        overwrite: out.overwrite,
        run_stamp: Some(match out.layout {
            crate::velo::Layout::Velo => out.velo_stamp.clone(),
            crate::velo::Layout::Native => out.run_id.clone(),
        }),
        layout_mode,
    };
    let mut router = match OutputRouter::new(tool.binary_name(), tool.datasets(), router_opts) {
        Ok(r) => r,
        Err(e) => {
            result.exit = Some(e.run_exit());
            result.error = Some(e.to_string());
            if let Some(mut log) = proc_log {
                log.line(&format!("router failed to open: {e}"));
                log.finish_with_counts(
                    result.files_matched,
                    result.parsed,
                    result.failed,
                    result.records,
                    start.elapsed(),
                );
            }
            return result;
        }
    };

    let mut progress = NullProgress;
    let outcome =
        triage_cli::runner::parse_validated(tool, &files, &mut router, true, &mut progress);
    result.parsed = outcome.parsed;
    result.failed += outcome.failed;
    // Parse failures reach the log in full and the manifest as a capped
    // sample: the process log is the per-file record an analyst reads to
    // find out which artifact failed and why, while `reason_samples` is a
    // summary that must stay readable after a run over thousands of files.
    for failure in &outcome.failures {
        if let Some(log) = proc_log.as_mut() {
            log.line(&format!(
                "parse failed: {} — {}",
                failure.path.display(),
                failure.reason
            ));
        }
        result.note(&failure.path, &failure.reason);
    }
    // `finish()` reports the destinations it actually published, which is
    // what the merge post-pass below has to know: a destination this run
    // never published can still hold a *previous* run's file.
    let finished = router.finish();
    result.output_paths = finished.published;
    match finished.outcome {
        Ok(records) => result.records = records,
        Err(e) => {
            // Without this the log shows `records: 0` next to a non-zero
            // `parsed:` and never says why, the same unexplained-count shape
            // the parse failures above exist to close -- and the sibling
            // `OutputRouter::new` failure already logs its reason.
            if let Some(log) = proc_log.as_mut() {
                log.line(&format!("router failed to close: {e}"));
            }
            result.exit = Some(e.run_exit());
            result.error = Some(e.to_string());
        }
    }
    if let Some(e) = outcome.abort {
        // An abort stops the file loop, so the remaining artifacts are
        // neither parsed nor counted as failed. Naming the one in flight is
        // the only way the log can say where the tool stopped -- the error
        // itself carries the output path that failed, not the artifact.
        if let Some(log) = proc_log.as_mut() {
            match &outcome.aborted_on {
                Some(path) => log.line(&format!("aborted while parsing {}: {e}", path.display())),
                None => log.line(&format!("aborted: {e}")),
            }
        }
        result.exit.get_or_insert(e.run_exit());
        result.error.get_or_insert(e.to_string());
    }

    // Merged files are derived from the PerUser/ output, so this runs only
    // after the router above has published every per-user file for this
    // tool. Each dataset merges independently: one dataset's merge failure
    // is recorded and skipped rather than losing the other datasets' merged
    // output or aborting the run (`note_merge_failure`'s doc comment).
    if out.layout == crate::velo::Layout::Velo {
        for spec in tool.datasets() {
            let stem = format!(
                "{}_{}",
                out.velo_stamp,
                triage_core::output::router::velo_basename(tool.binary_name(), spec)
            );
            // The dataset's discriminator, which is the directory level the
            // router just wrote this dataset's per-user slices into. Derived
            // from the same `DatasetSpec` the router used, so the merge reads
            // exactly the directory the write side chose -- see
            // `OutputLayout::for_velo_dataset`.
            let dataset_dir =
                triage_core::output::router::velo_discriminator(tool.binary_name(), spec);
            // `result.output_paths` is what the router published this run
            // (`OutputRouter::finish`), passed through so the merge can tell
            // this run's own system-scope output at the category root apart
            // from a previous run's file sitting on the same path -- and,
            // defensively, a real account named "system" the router just
            // wrote apart from a stale leftover of a previous run's own
            // reclaim. It is also what the merge selects its per-user
            // sources from, so a slice left in `PerUser/` by a previous run
            // -- a profile since removed from the host, an artifact class
            // that stopped being collected -- cannot enter this run's merged
            // output (`velo::merge::published_per_user_sources`).
            if let Some(root) = csv_root.as_ref() {
                match crate::velo::merge::merge_per_user(
                    root,
                    &stem,
                    out.overwrite,
                    &result.output_paths,
                    dataset_dir.as_deref(),
                ) {
                    Ok(Some(report)) => result.merged.push(report.merged_path),
                    Ok(None) => {}
                    Err(e) => {
                        // Same reasoning as the parse failures above: a
                        // merge failure increments `failed`, so the log
                        // must say which dataset merge it was and why,
                        // rather than leaving the closing count
                        // unexplained.
                        if let Some(log) = proc_log.as_mut() {
                            log.line(&format!("merge failed: {stem} — {e}"));
                        }
                        result.note_merge_failure(&stem, e);
                    }
                }
            }
            if let Some(root) = json_root.as_ref() {
                match crate::velo::merge::merge_per_user_ndjson(
                    root,
                    &stem,
                    out.overwrite,
                    &result.output_paths,
                    dataset_dir.as_deref(),
                ) {
                    Ok(Some(report)) => result.merged.push(report.merged_path),
                    Ok(None) => {}
                    Err(e) => {
                        // Logged for the same reason as the CSV merge above.
                        if let Some(log) = proc_log.as_mut() {
                            log.line(&format!("merge failed: {stem} — {e}"));
                        }
                        result.note_merge_failure(&stem, e);
                    }
                }
            }
        }
    }
    if let Some(log) = proc_log {
        log.finish_with_counts(
            result.files_matched,
            result.parsed,
            result.failed,
            result.records,
            start.elapsed(),
        );
    }
    result
}

/// Guards `run_tool_on_host` with `catch_unwind` so a parser panic (parsers
/// contain unwrap/expect/indexing that can panic on corrupt/adversarial
/// input) never unwinds past the worker thread and aborts the whole run.
/// Without this, a panicking `tool.parse()` would resume on the
/// `thread::scope` join in `run_tools_bounded` and kill the process before
/// the manifest is ever written — violating the "per-artifact/per-tool
/// failures never abort the run" contract. On panic, synthesizes a
/// `ToolRunResult` with `error` set so the failure surfaces in the manifest
/// exactly like any other per-tool error.
fn run_tool_on_host_guarded(
    entry: &ToolEntry,
    host: &HostCapture,
    index: &DiscoveryIndex,
    out: &OutputOpts,
) -> ToolRunResult {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_tool_on_host(entry, host, index, out)
    }))
    .unwrap_or_else(|_| {
        eprintln!(
            "Warning: tool '{}' panicked during parsing; recorded as a run-level error",
            entry.key
        );
        ToolRunResult::fatal(
            entry.key,
            entry.tool.binary_name(),
            "tool panicked during parsing".to_string(),
        )
    })
}

/// Counting semaphore for memory-heavy tools: at most `limit` hold a slot at
/// once, and a `HeavySlot` releases its slot on drop.
struct HeavyGate {
    limit: usize,
    active: Mutex<usize>,
    ready: Condvar,
}

impl HeavyGate {
    fn new(limit: usize) -> Self {
        HeavyGate {
            limit: limit.max(1),
            active: Mutex::new(0),
            ready: Condvar::new(),
        }
    }

    fn acquire(&self) -> HeavySlot<'_> {
        let mut active = self.active.lock().unwrap();
        while *active >= self.limit {
            active = self.ready.wait(active).unwrap();
        }
        *active += 1;
        HeavySlot(self)
    }
}

struct HeavySlot<'a>(&'a HeavyGate);

impl Drop for HeavySlot<'_> {
    fn drop(&mut self) {
        *self.0.active.lock().unwrap() -= 1;
        self.0.ready.notify_one();
    }
}

/// Run several tools (named by their `--only`/`--skip` keys) over one host's
/// shared discovery index, at most `jobs` running concurrently and at most
/// `heavy_jobs` of them memory-heavy, preserving result order to match `keys`.
///
/// `Box<dyn Tool>` is not `Sync` (`Tool` carries no `Send + Sync` bound), so
/// a `&ToolEntry` cannot cross a `std::thread::scope` spawn boundary — the
/// registry only hands out owned `Box<dyn Tool>` values, never `Sync`
/// references to them. Instead each worker thread pulls the next key by
/// index from an atomic counter and calls `registry::tool_for_key_with`
/// itself, building a fresh `ToolEntry` *inside* the thread rather than
/// sharing one constructed on the caller's thread. `host`, `index`, and `out`
/// are plain `&` data (no interior `dyn Tool`), so they are `Send + Sync` and
/// can be shared across the scoped threads directly.
// The parameter list is deliberately plain `&` data with no `dyn Tool` in it:
// that is what makes every argument Send + Sync and lets the scoped worker
// threads share them, as the doc comment above explains. Boxing them into a
// context struct would reintroduce the auto-trait question this signature
// exists to answer.
#[allow(clippy::too_many_arguments)]
pub fn run_tools_bounded(
    keys: &[String],
    host: &HostCapture,
    index: &DiscoveryIndex,
    out: &OutputOpts,
    jobs: usize,
    heavy_jobs: usize,
    ui: Option<&crate::progress_ui::ProgressUi>,
) -> Vec<ToolRunResult> {
    let total = keys.len();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<ToolRunResult>>> = (0..total).map(|_| Mutex::new(None)).collect();
    let heavy = HeavyGate::new(heavy_jobs);
    let host_start = Instant::now();

    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, total.max(1)) {
            let (next, done, slots, heavy) = (&next, &done, &slots, &heavy);
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= total {
                    break;
                }
                let t0 = Instant::now();
                let result = match crate::registry::tool_for_key_with(&keys[i], out.tools) {
                    Some(entry) => {
                        if let Some(u) = ui {
                            u.tool_started(entry.tool.binary_name());
                        }
                        let _slot = (entry.tool.resource_class() == ResourceClass::Heavy)
                            .then(|| heavy.acquire());
                        run_tool_on_host_guarded(&entry, host, index, out)
                    }
                    None => ToolRunResult::fatal(
                        &keys[i],
                        &keys[i],
                        format!("unknown tool key: {}", keys[i]),
                    ),
                };
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if let Some(u) = ui {
                    u.tool_finished(n, total, &result, t0.elapsed());
                }
                *slots[i].lock().unwrap() = Some(result);
            });
        }
    });

    if let Some(u) = ui {
        u.host_done(total, host_start.elapsed());
    }

    slots
        .into_iter()
        .map(|m| m.into_inner().unwrap().unwrap())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use triage_core::output::dataset::{DatasetSpec, JsonFraming};
    use triage_core::tool::{Scope, Tool};

    use crate::capture::test_host as host_at;

    fn csv_opts(csv_root: PathBuf) -> OutputOpts {
        OutputOpts {
            csv_root: Some(csv_root),
            json_root: None,
            overwrite: true,
            run_id: "20260710120000000".into(),
            tools: ToolOptions::default(),
            // These unit tests exercise routing/dedupe/panic-safety, not the
            // Velo tree itself (that's `crate::velo`'s and
            // `tests/velo_layout.rs`'s job) -- Native keeps the existing
            // `<out>/<host>/<Tool>/...` paths these tests already assert on.
            layout: crate::velo::Layout::Native,
            velo_stamp: String::new(),
            skip_hashes: true,
            time_range_notice: None,
        }
    }

    fn pe_entry() -> ToolEntry {
        ToolEntry {
            key: "pe",
            tool: Box::new(pe_triage::PeTool::default()),
        }
    }

    const ONE_DATASET: &[DatasetSpec] = &[DatasetSpec {
        id: "main",
        default_basename: "Test_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    }];

    #[test]
    fn aggregate_exit_distinguishes_partial_and_all_failed() {
        assert_eq!(aggregate_exit(2, 0, None), RunExit::Success);
        assert_eq!(aggregate_exit(2, 1, None), RunExit::Partial);
        assert_eq!(aggregate_exit(0, 1, None), RunExit::Fatal);
        assert_eq!(
            aggregate_exit(2, 1, Some(RunExit::OutputFailure)),
            RunExit::OutputFailure
        );
    }

    #[test]
    fn build_index_finds_union_of_patterns() {
        let td = TempDir::new().unwrap();
        fs::create_dir_all(td.path().join("Windows/Prefetch")).unwrap();
        fs::write(td.path().join("Windows/Prefetch/A.pf"), b"x").unwrap();
        fs::write(td.path().join("Windows/SYSTEM"), b"regf").unwrap();
        let tools = vec![
            pe_entry(),
            ToolEntry {
                key: "re",
                tool: Box::new(re_triage::RegistryTool::default()),
            },
        ];
        let idx = build_index(td.path(), &tools, &[]);
        assert_eq!(idx.candidates.values().map(Vec::len).sum::<usize>(), 2);
    }

    #[test]
    fn structured_validation_marks_matching_corrupt_artifact() {
        // Use an existing tool (PeTool) whose validate() checks prefetch magic.
        let td = TempDir::new().unwrap();
        fs::write(td.path().join("bogus.pf"), b"not a prefetch").unwrap();
        let tool = pe_triage::PeTool::default();
        assert!(matches!(
            tool.validate(&td.path().join("bogus.pf")),
            Validation::Corrupt { .. }
        ));
    }

    /// No fixture needed: an empty index must short-circuit before any
    /// router/output directory is built. This nails down the early-return
    /// contract independent of the gitignored `test captures/` fixtures.
    #[test]
    fn run_tool_on_host_with_no_matches_short_circuits() {
        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let idx = DiscoveryIndex {
            candidates: HashMap::new(),
            inaccessible: 0,
        };
        let out = csv_opts(td.path().join("out"));
        let res = run_tool_on_host(&pe_entry(), &host_at(&root), &idx, &out);
        assert_eq!(res.files_matched, 0);
        assert_eq!(res.parsed, 0);
        assert_eq!(res.error, None);
        assert!(res.output_paths.is_empty());
        assert!(
            !td.path().join("out").exists(),
            "no output directory should be created when no files matched"
        );
    }

    /// `run_tools_bounded` must return the same (key, parsed, records,
    /// error) tuples as calling `run_tool_on_host` sequentially per tool,
    /// in the same order — with jobs=2 (i.e. actually running concurrently,
    /// not falling back to jobs=1). An empty index (files_matched=0 for
    /// every tool) is enough to prove ordering + parity; no fixtures needed
    /// since no tool actually parses anything.
    #[test]
    fn bounded_matches_sequential_results() {
        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let host = host_at(&root);
        let idx = DiscoveryIndex {
            candidates: HashMap::new(),
            inaccessible: 0,
        };
        let out = csv_opts(td.path().join("out"));
        let keys: Vec<String> = vec!["mft".into(), "pe".into(), "evtx".into(), "sum".into()];

        let sequential: Vec<ToolRunResult> = keys
            .iter()
            .map(|k| {
                let entry = crate::registry::tool_for_key_with(k, ToolOptions::default()).unwrap();
                run_tool_on_host(&entry, &host, &idx, &out)
            })
            .collect();

        let bounded = run_tools_bounded(&keys, &host, &idx, &out, 2, 1, None);

        assert_eq!(bounded.len(), sequential.len());
        for (seq, par) in sequential.iter().zip(bounded.iter()) {
            assert_eq!(seq.key, par.key, "order must match keys order");
            assert_eq!(seq.binary_name, par.binary_name);
            assert_eq!(seq.files_matched, par.files_matched);
            assert_eq!(seq.parsed, par.parsed);
            assert_eq!(seq.failed, par.failed);
            assert_eq!(seq.records, par.records);
            assert_eq!(seq.error, par.error);
        }
        // Explicitly confirm order equals the original keys order, not just
        // pairwise-equal-to-sequential (which could coincidentally match if
        // sequential itself were reordered).
        let bounded_keys: Vec<&str> = bounded.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(
            bounded_keys,
            keys.iter().map(|k| k.as_str()).collect::<Vec<_>>()
        );
    }

    /// Proves the `catch_unwind` guard in `run_tool_on_host_guarded` actually
    /// isolates a panicking parser: a fake `Tool` whose `validate()` always
    /// passes and whose `parse()` unconditionally panics (simulating the
    /// unwrap/expect/indexing panics real parsers can hit on corrupt input).
    /// Without the guard this panic would unwind straight through this test
    /// function (failing it with "test panicked", not a normal assertion
    /// failure) instead of coming back as a `ToolRunResult` with `error`
    /// set — so this test only passes because the guard is in place.
    #[test]
    fn run_tool_on_host_guarded_survives_a_panicking_parser() {
        struct PanicTool;
        impl Tool for PanicTool {
            fn binary_name(&self) -> &'static str {
                "PanicTool"
            }
            fn patterns(&self) -> &[&'static str] {
                &["*.panic"]
            }
            fn validate_legacy(&self, _path: &Path) -> bool {
                true
            }
            fn datasets(&self) -> &'static [DatasetSpec] {
                ONE_DATASET
            }
            fn scope(&self) -> Scope {
                Scope::SystemWide
            }
            fn parse(
                &self,
                _path: &Path,
                _out: &mut OutputRouter,
            ) -> Result<u64, triage_core::error::TriageError> {
                panic!("simulated parser panic on corrupt input");
            }
        }

        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("evil.panic"), b"corrupt").unwrap();
        let entry = ToolEntry {
            key: "panic_tool",
            tool: Box::new(PanicTool),
        };
        let idx = build_index(&root, std::slice::from_ref(&entry), &[]);
        assert_eq!(
            idx.candidates["panic_tool"].len(),
            1,
            "fixture file must be discovered"
        );
        let out = csv_opts(td.path().join("out"));

        let res = run_tool_on_host_guarded(&entry, &host_at(&root), &idx, &out);
        assert!(
            res.error.is_some(),
            "panicking parse must surface as a per-tool error, not abort the run"
        );
        assert_eq!(res.key, "panic_tool");
        assert_eq!(res.binary_name, "PanicTool");
        assert_eq!(res.parsed, 0);
    }

    /// Content dedupe is per-tool policy. Two byte-identical artifacts at
    /// different paths are one parse for a tool that opts in (the default) and
    /// two for a tool that opts out.
    ///
    /// BrowserTriage opts out because it emits a `Profile` column derived from
    /// the path: a browser update leaves `Snapshots/<version>` copies that are
    /// byte-identical to the live profile, and collapsing them keeps the rows
    /// but attributes them all to whichever copy was walked first. This drives
    /// the real `run_tool_on_host` rather than asserting on the trait method,
    /// because the method is only worth anything if `execute` honours it.
    #[test]
    fn content_dedupe_is_per_tool_policy() {
        struct CountTool(bool);
        impl Tool for CountTool {
            fn binary_name(&self) -> &'static str {
                "CountTool"
            }
            fn patterns(&self) -> &[&'static str] {
                &["*.count"]
            }
            fn validate_legacy(&self, _path: &Path) -> bool {
                true
            }
            fn dedupe_by_content(&self) -> bool {
                self.0
            }
            fn datasets(&self) -> &'static [DatasetSpec] {
                ONE_DATASET
            }
            fn scope(&self) -> Scope {
                Scope::SystemWide
            }
            fn parse(
                &self,
                _path: &Path,
                _out: &mut OutputRouter,
            ) -> Result<u64, triage_core::error::TriageError> {
                Ok(0)
            }
        }

        // Same bytes, two paths — exactly the Snapshots/<version> shape.
        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(root.join("a/x.count"), b"identical").unwrap();
        fs::write(root.join("b/x.count"), b"identical").unwrap();
        let host = host_at(&root);

        for (dedupe, want_parsed, want_skipped) in [(true, 1, 1), (false, 2, 0)] {
            let entry = ToolEntry {
                key: "count_tool",
                tool: Box::new(CountTool(dedupe)),
            };
            let idx = build_index(&root, std::slice::from_ref(&entry), &[]);
            assert_eq!(idx.candidates["count_tool"].len(), 2, "both copies found");
            let out = csv_opts(td.path().join(format!("out-{dedupe}")));
            let res = run_tool_on_host(&entry, &host, &idx, &out);
            assert_eq!(res.supported, 2, "dedupe={dedupe}: both validate");
            assert_eq!(res.parsed, want_parsed, "dedupe={dedupe}: parsed");
            assert_eq!(res.deduplicated, want_skipped, "dedupe={dedupe}: skipped");
        }
    }

    /// Pins the fix for the review finding that a merge failure was
    /// invisible outside the manifest: recording it via
    /// `ToolRunResult::note_merge_failure` must increment `failed` (which
    /// flows into `aggregate_exit`'s non-zero exit and
    /// `progress_ui::summary_line`'s failure line), while still leaving
    /// `error` untouched so the per-dataset merge loop in
    /// `run_tool_on_host` keeps isolating one dataset's merge failure from
    /// another's.
    ///
    /// Drives a real `run_tool_on_host` call under `Layout::Velo` with a
    /// fake `Scope::UserElseSystem` tool that writes a different-shaped row
    /// depending on which user's file it's parsing — two per-user CSVs with
    /// mismatched headers is exactly the condition `merge_per_user` already
    /// treats as a hard error, so the merge post-pass genuinely fails here
    /// rather than being forced to fail by calling `merge_per_user`
    /// directly.
    #[test]
    fn a_merge_failure_increments_failed_without_setting_a_run_level_error() {
        #[derive(serde::Serialize)]
        struct RowA {
            x: u32,
            y: u32,
        }
        #[derive(serde::Serialize)]
        struct RowB {
            y: u32,
            x: u32,
            z: u32,
        }

        struct MismatchedHeaderTool;
        impl Tool for MismatchedHeaderTool {
            fn binary_name(&self) -> &'static str {
                "MismatchTool"
            }
            fn patterns(&self) -> &[&'static str] {
                &["*.hdr"]
            }
            fn validate_legacy(&self, _path: &Path) -> bool {
                true
            }
            fn datasets(&self) -> &'static [DatasetSpec] {
                ONE_DATASET
            }
            fn scope(&self) -> Scope {
                Scope::UserElseSystem
            }
            fn parse(
                &self,
                path: &Path,
                out: &mut OutputRouter,
            ) -> Result<u64, triage_core::error::TriageError> {
                if path.to_string_lossy().contains("alice") {
                    out.write("main", &RowA { x: 1, y: 2 })?;
                } else {
                    out.write("main", &RowB { y: 3, x: 4, z: 5 })?;
                }
                Ok(1)
            }
        }

        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        fs::create_dir_all(root.join("Users/alice")).unwrap();
        fs::create_dir_all(root.join("Users/bob")).unwrap();
        // Distinct content: `dedupe_by_content` defaults to true, and
        // identical bytes would collapse the two files into one parse,
        // hiding the header mismatch this test needs.
        fs::write(root.join("Users/alice/a.hdr"), b"alice").unwrap();
        fs::write(root.join("Users/bob/b.hdr"), b"bob").unwrap();

        // Key "le" borrowed only so `velo::category_for_key` resolves to a
        // real category ("FileSystem") -- this test's `Tool` is otherwise
        // unrelated to LETriage.
        let entry = ToolEntry {
            key: "le",
            tool: Box::new(MismatchedHeaderTool),
        };
        let host = host_at(&root);
        let idx = build_index(&root, std::slice::from_ref(&entry), &[]);
        assert_eq!(idx.candidates["le"].len(), 2, "both per-user files found");

        let mut out = csv_opts(td.path().join("out"));
        out.layout = crate::velo::Layout::Velo;
        out.velo_stamp = "20260101000000".into();

        let res = run_tool_on_host(&entry, &host, &idx, &out);
        assert_eq!(res.parsed, 2, "both files parse fine on their own");
        assert_eq!(
            res.error, None,
            "a merge failure must not set a run-level error: that would break \
             per-dataset isolation"
        );
        assert!(
            res.failed >= 1,
            "a merge failure must increment failed so it flows into the exit \
             code and progress summary, not just a manifest reason sample"
        );
        assert!(
            res.merged.is_empty(),
            "the failed merge must not report a merged path"
        );
    }

    /// Proves `run_tool_on_host` actually wires up `velo::proclog::ProcessLog`
    /// under `--layout velo`, not just that `ProcessLog` works in isolation --
    /// exactly the gap a previous task in this plan shipped (a function with
    /// unit tests but no caller). Drives a real run with one supported and
    /// one unsupported file through a fake tool, then reads
    /// `process_logs/<Tool>.log` back off disk and checks it against the
    /// counts `run_tool_on_host` itself returned.
    #[test]
    fn run_tool_on_host_writes_a_process_log_under_velo_layout() {
        struct LogTestTool;
        impl Tool for LogTestTool {
            fn binary_name(&self) -> &'static str {
                "LogTestTool"
            }
            fn patterns(&self) -> &[&'static str] {
                &["*.logtest"]
            }
            fn validate(&self, path: &Path) -> Validation {
                if path.to_string_lossy().contains("bad") {
                    Validation::Unsupported {
                        reason: "deliberately unsupported fixture".to_string(),
                    }
                } else {
                    Validation::Supported
                }
            }
            fn validate_legacy(&self, _path: &Path) -> bool {
                true
            }
            fn datasets(&self) -> &'static [DatasetSpec] {
                ONE_DATASET
            }
            fn scope(&self) -> Scope {
                Scope::SystemWide
            }
            fn parse(
                &self,
                _path: &Path,
                out: &mut OutputRouter,
            ) -> Result<u64, triage_core::error::TriageError> {
                #[derive(serde::Serialize)]
                struct Row {
                    n: u32,
                }
                out.write("main", &Row { n: 1 })?;
                Ok(1)
            }
        }

        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("good.logtest"), b"ok").unwrap();
        fs::write(root.join("bad.logtest"), b"nope").unwrap();

        // Key "mft" borrowed only so `velo::category_for_key` resolves to a
        // real category ("FileSystem") -- this test's `Tool` is otherwise
        // unrelated to MFTTriage.
        let entry = ToolEntry {
            key: "mft",
            tool: Box::new(LogTestTool),
        };
        let host = host_at(&root);
        let idx = build_index(&root, std::slice::from_ref(&entry), &[]);
        assert_eq!(idx.candidates["mft"].len(), 2, "both fixtures found");

        let out_root = td.path().join("out");
        let mut out = csv_opts(out_root.clone());
        out.layout = crate::velo::Layout::Velo;
        out.velo_stamp = "20260101000000".into();

        let res = run_tool_on_host(&entry, &host, &idx, &out);
        assert_eq!(res.parsed, 1);
        assert_eq!(res.unsupported, 1);
        assert_eq!(res.records, 1);

        let collection = out_root.join(crate::velo::collection_dir(
            &host.output_id,
            &out.velo_stamp,
        ));
        let log_path = collection.join("process_logs/LogTestTool.log");
        let body = std::fs::read_to_string(&log_path)
            .unwrap_or_else(|e| panic!("expected {} to exist: {e}", log_path.display()));

        assert!(body.contains("discovered 2 candidate files"), "got {body}");
        assert!(
            body.contains("unsupported") && body.contains("deliberately unsupported fixture"),
            "got {body}"
        );
        assert!(body.contains("parsed: 1"), "got {body}");
        assert!(body.contains("failed: 0"), "got {body}");
        assert!(body.contains("records: 1"), "got {body}");
        assert!(body.contains("duration:"), "got {body}");
    }

    /// Under `--layout native` there is no `Processed-<HOST>-<stamp>`
    /// directory of the shape process logs need, matching how
    /// `write_output_hashes` is gated on the same `Layout::Velo` condition
    /// (`main.rs`) -- so no process log is written at all, and none of the
    /// default-layout tests above should see a `process_logs/` directory
    /// appear anywhere under their output root.
    #[test]
    fn no_process_log_is_written_under_native_layout() {
        struct NativeTool;
        impl Tool for NativeTool {
            fn binary_name(&self) -> &'static str {
                "NativeTool"
            }
            fn patterns(&self) -> &[&'static str] {
                &["*.nativetest"]
            }
            fn validate_legacy(&self, _path: &Path) -> bool {
                true
            }
            fn datasets(&self) -> &'static [DatasetSpec] {
                ONE_DATASET
            }
            fn scope(&self) -> Scope {
                Scope::SystemWide
            }
            fn parse(
                &self,
                _path: &Path,
                _out: &mut OutputRouter,
            ) -> Result<u64, triage_core::error::TriageError> {
                Ok(0)
            }
        }

        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("x.nativetest"), b"ok").unwrap();
        let entry = ToolEntry {
            key: "native_tool",
            tool: Box::new(NativeTool),
        };
        let host = host_at(&root);
        let idx = build_index(&root, std::slice::from_ref(&entry), &[]);
        let out_root = td.path().join("out");
        let out = csv_opts(out_root.clone());
        assert_eq!(out.layout, crate::velo::Layout::Native);

        let res = run_tool_on_host(&entry, &host, &idx, &out);
        assert_eq!(res.parsed, 1);

        let found_any_process_log = walkdir_has_process_logs(&out_root);
        assert!(
            !found_any_process_log,
            "no process_logs/ directory should exist under --layout native"
        );
    }

    /// A tool with zero matching candidates must create nothing at all under
    /// `--layout velo`, not even a process log: the function's own doc
    /// comment promises "no output directory created" for a no-op tool, and
    /// that promise exists so a many-tool run over a sparse capture doesn't
    /// litter one empty output tree per unmatched tool per host. Opening a
    /// `ProcessLog` unconditionally would break that -- `ProcessLog::open`
    /// itself `create_dir_all`s the collection directory -- so this pins both
    /// halves: no `process_logs/<Tool>.log` file, and no
    /// `Processed-<HOST>-<stamp>` directory at all.
    #[test]
    fn a_zero_match_tool_creates_no_process_log_and_no_collection_directory() {
        struct NeverMatchesTool;
        impl Tool for NeverMatchesTool {
            fn binary_name(&self) -> &'static str {
                "NeverMatchesTool"
            }
            fn patterns(&self) -> &[&'static str] {
                &["*.nevermatch"]
            }
            fn validate_legacy(&self, _path: &Path) -> bool {
                true
            }
            fn datasets(&self) -> &'static [DatasetSpec] {
                ONE_DATASET
            }
            fn scope(&self) -> Scope {
                Scope::SystemWide
            }
            fn parse(
                &self,
                _path: &Path,
                _out: &mut OutputRouter,
            ) -> Result<u64, triage_core::error::TriageError> {
                Ok(0)
            }
        }

        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        fs::create_dir_all(&root).unwrap();
        // No `.nevermatch` file anywhere -- the discovery index will be empty
        // for this tool.
        fs::write(root.join("unrelated.txt"), b"ok").unwrap();
        let entry = ToolEntry {
            key: "mft", // borrowed only so `velo::category_for_key` resolves.
            tool: Box::new(NeverMatchesTool),
        };
        let host = host_at(&root);
        let idx = build_index(&root, std::slice::from_ref(&entry), &[]);
        assert_eq!(idx.candidates["mft"].len(), 0, "no fixture should match");

        let out_root = td.path().join("out");
        let mut out = csv_opts(out_root.clone());
        out.layout = crate::velo::Layout::Velo;
        out.velo_stamp = "20260101000000".into();

        let res = run_tool_on_host(&entry, &host, &idx, &out);
        assert_eq!(res.files_matched, 0);
        assert_eq!(res.parsed, 0);

        let collection = out_root.join(crate::velo::collection_dir(
            &host.output_id,
            &out.velo_stamp,
        ));
        assert!(
            !collection.exists(),
            "a zero-match tool must not create the collection directory at all: \
             found {collection:?}"
        );
        assert!(
            !walkdir_has_process_logs(&out_root),
            "a zero-match tool must not create a process_logs/ directory"
        );
    }

    /// The other half of the zero-candidates case above: candidates *were*
    /// discovered, but every single one failed validation, so `files` (the
    /// validated set) ends up empty too and takes the same early-return path
    /// as a genuine no-op tool. Unlike that case, this one must still get a
    /// process log -- "found N files, every one corrupt" is exactly the
    /// diagnosis a process log exists to carry, and the doc contract only
    /// promises no output directory when no files *matched*, not when files
    /// matched but none validated. Pins both the log's existence and its
    /// content (the per-file reasons and a non-zero `failed` count), so a
    /// regression back to gating on `files.is_empty()` alone would fail this
    /// test even though it would still pass the zero-candidates test above.
    #[test]
    fn a_tool_where_every_candidate_fails_validation_still_gets_a_process_log() {
        struct AllCorruptTool;
        impl Tool for AllCorruptTool {
            fn binary_name(&self) -> &'static str {
                "AllCorruptTool"
            }
            fn patterns(&self) -> &[&'static str] {
                &["*.allcorrupt"]
            }
            fn validate(&self, _path: &Path) -> Validation {
                Validation::Corrupt {
                    reason: "deliberately corrupt fixture".to_string(),
                }
            }
            fn validate_legacy(&self, _path: &Path) -> bool {
                true
            }
            fn datasets(&self) -> &'static [DatasetSpec] {
                ONE_DATASET
            }
            fn scope(&self) -> Scope {
                Scope::SystemWide
            }
            fn parse(
                &self,
                _path: &Path,
                _out: &mut OutputRouter,
            ) -> Result<u64, triage_core::error::TriageError> {
                unreachable!("no file should ever reach parse: every candidate is corrupt")
            }
        }

        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.allcorrupt"), b"one").unwrap();
        fs::write(root.join("b.allcorrupt"), b"two").unwrap();

        // Key "mft" borrowed only so `velo::category_for_key` resolves.
        let entry = ToolEntry {
            key: "mft",
            tool: Box::new(AllCorruptTool),
        };
        let host = host_at(&root);
        let idx = build_index(&root, std::slice::from_ref(&entry), &[]);
        assert_eq!(idx.candidates["mft"].len(), 2, "both fixtures discovered");

        let out_root = td.path().join("out");
        let mut out = csv_opts(out_root.clone());
        out.layout = crate::velo::Layout::Velo;
        out.velo_stamp = "20260101000000".into();

        let res = run_tool_on_host(&entry, &host, &idx, &out);
        assert_eq!(res.files_matched, 2);
        assert_eq!(res.parsed, 0);
        assert_eq!(res.corrupt, 2);
        assert_eq!(res.failed, 2);

        let collection = out_root.join(crate::velo::collection_dir(
            &host.output_id,
            &out.velo_stamp,
        ));
        let log_path = collection.join("process_logs/AllCorruptTool.log");
        let body = std::fs::read_to_string(&log_path)
            .unwrap_or_else(|e| panic!("expected {} to exist: {e}", log_path.display()));

        assert!(body.contains("discovered 2 candidate files"), "got {body}");
        assert!(
            body.contains("corrupt") && body.contains("deliberately corrupt fixture"),
            "the per-file reasons must survive into the log: got {body}"
        );
        assert!(body.contains("a.allcorrupt"), "got {body}");
        assert!(body.contains("b.allcorrupt"), "got {body}");
        assert!(body.contains("parsed: 0"), "got {body}");
        assert!(body.contains("failed: 2"), "got {body}");
        assert!(body.contains("records: 0"), "got {body}");
        assert!(body.contains("duration:"), "got {body}");
    }

    /// Small recursive scan for a directory named `process_logs` anywhere
    /// under `root`, used only by the native-layout negative test above.
    fn walkdir_has_process_logs(root: &Path) -> bool {
        if !root.exists() {
            return false;
        }
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().and_then(|n| n.to_str()) == Some("process_logs") {
                        return true;
                    }
                    stack.push(path);
                }
            }
        }
        false
    }

    /// Data-gated: mirrors pe-triage's own capture-driven tests. Uses the
    /// orchestrator's own `capture::enumerate` + `build_index` to drive a
    /// real host through `run_tool_on_host` and confirm it parses and
    /// writes output. Skips (via `triage_testkit::skip_if_missing`) when the
    /// gitignored `test captures/` directory isn't present.
    #[test]
    fn run_tool_on_host_parses_and_reports() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
        if triage_testkit::skip_if_missing(&root, "test captures") {
            return;
        }
        let (_, hosts) = crate::capture::enumerate(&root).expect("captures present but unreadable");
        let host = hosts
            .first()
            .expect("at least one host in test captures")
            .clone();
        let td = TempDir::new().unwrap();
        let entry = pe_entry();
        let idx = build_index(&host.artifact_root, std::slice::from_ref(&entry), &[]);
        let out = csv_opts(td.path().join("out"));
        let res = run_tool_on_host(&entry, &host, &idx, &out);
        assert_eq!(res.error, None);
        assert!(res.parsed >= 1, "expected at least one parsed .pf file");
        assert!(td
            .path()
            .join("out")
            .join(&host.output_id)
            .join("PETriage")
            .exists());
    }
}
