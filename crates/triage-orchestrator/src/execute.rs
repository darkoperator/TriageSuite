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
    pub output_paths: Vec<PathBuf>,
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
/// Uses `OutputLayoutMode::Nested` (`<root>/<BinaryName>/<identity>/...`)
/// rather than the CLI's default Flat layout: the orchestrator already fans
/// output out per-host, so nesting per-tool underneath that keeps a multi-
/// tool, multi-host run's output tree legible (`<csv_root>/<host>/<Tool>/...`)
/// instead of dumping every tool's identity-stamped files into one shared
/// per-host directory.
///
/// Early-returns (no router built, no output directory created) when no
/// files matched — this keeps a no-op tool from creating an empty output
/// tree for every host.
pub fn run_tool_on_host(
    entry: &ToolEntry,
    host: &HostCapture,
    index: &DiscoveryIndex,
    out: &OutputOpts,
) -> ToolRunResult {
    let tool = entry.tool.as_ref();
    let candidates = index.candidates.get(entry.key).cloned().unwrap_or_default();
    let mut result = ToolRunResult::new(entry.key, tool.binary_name());
    result.files_matched = candidates.len() as u64;
    let mut files = Vec::new();
    for path in candidates {
        match tool.validate(&path) {
            Validation::Supported => {
                result.supported += 1;
                files.push(path);
            }
            Validation::Unsupported { reason } => {
                result.unsupported += 1;
                result.note(&path, reason);
            }
            Validation::Corrupt { reason } => {
                result.corrupt += 1;
                result.failed += 1;
                result.note(&path, reason);
            }
            Validation::Unreadable { error } => {
                result.unreadable += 1;
                result.failed += 1;
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
                false
            }
            Err(error) => {
                result.unreadable += 1;
                result.failed += 1;
                result.note(path, error);
                false
            }
        });
    }
    if files.is_empty() {
        return result;
    }

    let router_opts = RouterOptions {
        csv_root: out.csv_root.as_ref().map(|r| r.join(&host.output_id)),
        json_root: out.json_root.as_ref().map(|r| r.join(&host.output_id)),
        csvf: None,
        jsonf: None,
        pretty: false,
        overwrite: out.overwrite,
        run_stamp: Some(out.run_id.clone()),
        layout_mode: OutputLayoutMode::Nested,
    };
    let mut router = match OutputRouter::new(tool.binary_name(), tool.datasets(), router_opts) {
        Ok(r) => r,
        Err(e) => {
            result.exit = Some(e.run_exit());
            result.error = Some(e.to_string());
            return result;
        }
    };

    let mut progress = NullProgress;
    let outcome =
        triage_cli::runner::parse_validated(tool, &files, &mut router, true, &mut progress);
    result.parsed = outcome.parsed;
    result.failed += outcome.failed;
    // Snapshot the exact final destinations before finish consumes the router.
    result.output_paths = router.output_paths();
    match router.finish() {
        Ok(records) => result.records = records,
        Err(e) => {
            result.exit = Some(e.run_exit());
            result.error = Some(e.to_string());
            result.output_paths.retain(|path| path.exists());
        }
    }
    if let Some(e) = outcome.abort {
        result.exit.get_or_insert(e.run_exit());
        result.error.get_or_insert(e.to_string());
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
