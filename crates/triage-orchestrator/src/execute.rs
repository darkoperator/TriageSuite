// (filled in Tasks 6-7)

use crate::capture::HostCapture;
use crate::registry::ToolEntry;
use globset::{Glob, GlobSetBuilder};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use triage_cli::progress::NullProgress;
use triage_core::error::RunExit;
use triage_core::output::layout::OutputLayoutMode;
use triage_core::output::router::{OutputRouter, RouterOptions};
use triage_core::tool::Validation;

/// Single recursive walk over `root` matching the union of all selected
/// tools' filename globs. One walk feeds every tool via `files_for_tool`.
pub struct DiscoveryIndex {
    pub candidates: HashMap<String, Vec<PathBuf>>,
    pub inaccessible: u64,
}

pub fn build_index(root: &Path, tools: &[ToolEntry], exclude: &[PathBuf]) -> DiscoveryIndex {
    let plans: Vec<(&str, globset::GlobSet)> = tools
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
                    candidates
                        .entry((*key).to_string())
                        .or_default()
                        .push(path.to_path_buf());
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

fn glob_set(patterns: &[&'static str]) -> globset::GlobSet {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        // Case-insensitive filename match.
        if let Ok(g) = Glob::new(&p.to_lowercase()) {
            b.add(g);
        }
    }
    b.build()
        .unwrap_or_else(|_| GlobSetBuilder::new().build().unwrap())
}

/// Filter the shared discovery index down to one tool's own `patterns()`,
/// then confirm each candidate with `tool.validate()`. A single file in the
/// index may pass this for multiple tools (many-to-many dispatch).
/// Where a `run_tool_on_host` invocation should write its output, and
/// whether it may overwrite existing files.
pub struct OutputOpts {
    pub csv_root: Option<PathBuf>,
    pub json_root: Option<PathBuf>,
    pub overwrite: bool,
    pub run_id: String,
    pub hunt: bool,
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
/// `index` down to this tool's files (Task 6's `files_for_tool`), builds a
/// per-host `OutputRouter` rooted at `<csv_root>/<host.host>` (and likewise
/// for `json_root`), drives parsing via `triage_cli::runner::parse_validated`
/// with a `NullProgress` (the orchestrator owns its own progress rendering
/// across hosts/tools, not per-call), then flushes the router.
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
    let mut files = Vec::new();
    let mut result = ToolRunResult {
        key: entry.key.to_string(),
        binary_name: tool.binary_name().to_string(),
        files_matched: candidates.len() as u64,
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
    };
    for path in candidates {
        match tool.validate(&path) {
            Validation::Supported => {
                result.supported += 1;
                files.push(path);
            }
            Validation::Unsupported { reason } => {
                result.unsupported += 1;
                if result.reason_samples.len() < 10 {
                    result
                        .reason_samples
                        .push(format!("{}: {reason}", path.display()));
                }
            }
            Validation::Corrupt { reason } => {
                result.corrupt += 1;
                result.failed += 1;
                if result.reason_samples.len() < 10 {
                    result
                        .reason_samples
                        .push(format!("{}: {reason}", path.display()));
                }
            }
            Validation::Unreadable { error } => {
                result.unreadable += 1;
                result.failed += 1;
                if result.reason_samples.len() < 10 {
                    result
                        .reason_samples
                        .push(format!("{}: {error}", path.display()));
                }
            }
        }
    }
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
            if result.reason_samples.len() < 10 {
                result
                    .reason_samples
                    .push(format!("{}: {error}", path.display()));
            }
            false
        }
    });
    if files.is_empty() {
        return result;
    }

    let csv_root = out.csv_root.as_ref().map(|r| r.join(&host.output_id));
    let json_root = out.json_root.as_ref().map(|r| r.join(&host.output_id));
    let router_opts = RouterOptions {
        csv_root,
        json_root,
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
    let key = entry.key.to_string();
    let binary_name = entry.tool.binary_name().to_string();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_tool_on_host(entry, host, index, out)
    })) {
        Ok(r) => r,
        Err(_) => {
            eprintln!(
                "Warning: tool '{key}' panicked during parsing; recorded as a run-level error"
            );
            ToolRunResult {
                key,
                binary_name,
                files_matched: 0,
                supported: 0,
                unsupported: 0,
                corrupt: 0,
                unreadable: 0,
                deduplicated: 0,
                reason_samples: vec!["tool panicked during parsing".into()],
                parsed: 0,
                failed: 0,
                records: 0,
                output_paths: Vec::new(),
                error: Some("tool panicked during parsing".to_string()),
                exit: Some(RunExit::Fatal),
            }
        }
    }
}

/// Run several tools (named by their `--only`/`--skip` keys) over one host's
/// shared discovery index, at most `jobs` running concurrently, preserving
/// result order to match `keys`.
///
/// `Box<dyn Tool>` is not `Sync` (`Tool` carries no `Send + Sync` bound), so
/// a `&ToolEntry` cannot cross a `std::thread::scope` spawn boundary — the
/// registry only hands out owned `Box<dyn Tool>` values, never `Sync`
/// references to them. Instead each worker thread pulls the next key by
/// index from an atomic counter and calls `registry::tool_for_key` itself,
/// building a fresh `ToolEntry` *inside* the thread rather than sharing one
/// constructed on the caller's thread. `host`, `index`, and `out` are plain
/// `&` data (no interior `dyn Tool`), so they are `Send + Sync` and can be
/// shared across the scoped threads directly.
pub fn run_tools_bounded(
    keys: &[String],
    host: &HostCapture,
    index: &DiscoveryIndex,
    out: &OutputOpts,
    jobs: usize,
    heavy_jobs: usize,
    ui: Option<&crate::progress_ui::ProgressUi>,
) -> Vec<ToolRunResult> {
    let jobs = jobs.max(1);
    let heavy_jobs = heavy_jobs.max(1);
    let total = keys.len();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<Option<ToolRunResult>>> = (0..keys.len())
        .map(|_| std::sync::Mutex::new(None))
        .collect();
    let host_start = std::time::Instant::now();
    let heavy_state = (std::sync::Mutex::new(0usize), std::sync::Condvar::new());

    std::thread::scope(|scope| {
        for _ in 0..jobs.min(keys.len().max(1)) {
            let next = &next;
            let done = &done;
            let slots = &slots;
            let heavy_state = &heavy_state;
            scope.spawn(move || loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= keys.len() {
                    break;
                }
                let t0 = std::time::Instant::now();
                let result = match crate::registry::tool_for_key_with_hunt(&keys[i], out.hunt) {
                    Some(entry) => {
                        let name = entry.tool.binary_name().to_string();
                        if let Some(u) = ui {
                            u.tool_started(&name);
                        }
                        let heavy =
                            entry.tool.resource_class() == triage_core::tool::ResourceClass::Heavy;
                        if heavy {
                            let (lock, ready) = heavy_state;
                            let mut active = lock.lock().unwrap();
                            while *active >= heavy_jobs {
                                active = ready.wait(active).unwrap();
                            }
                            *active += 1;
                            drop(active);
                        }
                        let result = run_tool_on_host_guarded(&entry, host, index, out);
                        if heavy {
                            let (lock, ready) = heavy_state;
                            let mut active = lock.lock().unwrap();
                            *active -= 1;
                            ready.notify_one();
                        }
                        result
                    }
                    None => ToolRunResult {
                        key: keys[i].clone(),
                        binary_name: keys[i].clone(),
                        files_matched: 0,
                        supported: 0,
                        unsupported: 0,
                        corrupt: 0,
                        unreadable: 0,
                        deduplicated: 0,
                        reason_samples: vec![format!("unknown tool key: {}", keys[i])],
                        parsed: 0,
                        failed: 0,
                        records: 0,
                        output_paths: Vec::new(),
                        error: Some(format!("unknown tool key: {}", keys[i])),
                        exit: Some(RunExit::Fatal),
                    },
                };
                let dur = t0.elapsed();
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if let Some(u) = ui {
                    u.tool_finished(n, total, &result, dur);
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
    use triage_core::tool::Tool;

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
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn build_index_finds_union_of_patterns() {
        let td = TempDir::new().unwrap();
        fs::create_dir_all(td.path().join("Windows/Prefetch")).unwrap();
        fs::write(td.path().join("Windows/Prefetch/A.pf"), b"x").unwrap();
        fs::write(td.path().join("Windows/SYSTEM"), b"regf").unwrap();
        let tools = vec![
            crate::registry::ToolEntry {
                key: "pe",
                tool: Box::new(pe_triage::PeTool::default()),
            },
            crate::registry::ToolEntry {
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
        let host = crate::capture::HostCapture {
            host: "H".into(),
            output_id: "H".into(),
            os: "x".into(),
            collection_dir: root.clone(),
            artifact_root: root.clone(),
            source_archive: None,
        };
        // Empty index: nothing for files_for_tool to match.
        let idx = DiscoveryIndex {
            candidates: HashMap::new(),
            inaccessible: 0,
        };
        let entry = crate::registry::ToolEntry {
            key: "pe",
            tool: Box::new(pe_triage::PeTool::default()),
        };
        let out = OutputOpts {
            csv_root: Some(td.path().join("out")),
            json_root: None,
            overwrite: true,
            run_id: "20260710120000000".into(),
            hunt: false,
        };
        let res = run_tool_on_host(&entry, &host, &idx, &out);
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
        let host = crate::capture::HostCapture {
            host: "H".into(),
            output_id: "H".into(),
            os: "x".into(),
            collection_dir: root.clone(),
            artifact_root: root.clone(),
            source_archive: None,
        };
        let idx = DiscoveryIndex {
            candidates: HashMap::new(),
            inaccessible: 0,
        };
        let out = OutputOpts {
            csv_root: Some(td.path().join("out")),
            json_root: None,
            overwrite: true,
            run_id: "20260710120000000".into(),
            hunt: false,
        };
        let keys: Vec<String> = vec!["mft".into(), "pe".into(), "evtx".into(), "sum".into()];

        let sequential: Vec<ToolRunResult> = keys
            .iter()
            .map(|k| {
                let entry = crate::registry::tool_for_key(k).unwrap();
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
            fn datasets(&self) -> &'static [triage_core::output::dataset::DatasetSpec] {
                const DS: &[triage_core::output::dataset::DatasetSpec] =
                    &[triage_core::output::dataset::DatasetSpec {
                        id: "main",
                        default_basename: "PanicTool_Output",
                        framing: triage_core::output::dataset::JsonFraming::Ndjson,
                        csv_only: false,
                        override_suffix: None,
                    }];
                DS
            }
            fn scope(&self) -> triage_core::tool::Scope {
                triage_core::tool::Scope::SystemWide
            }
            fn parse(
                &self,
                _path: &Path,
                _out: &mut triage_core::output::router::OutputRouter,
            ) -> Result<u64, triage_core::error::TriageError> {
                panic!("simulated parser panic on corrupt input");
            }
        }

        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("evil.panic"), b"corrupt").unwrap();
        let host = crate::capture::HostCapture {
            host: "H".into(),
            output_id: "H".into(),
            os: "x".into(),
            collection_dir: root.clone(),
            artifact_root: root.clone(),
            source_archive: None,
        };
        let entry = crate::registry::ToolEntry {
            key: "panic_tool",
            tool: Box::new(PanicTool),
        };
        let idx = build_index(&root, std::slice::from_ref(&entry), &[]);
        assert_eq!(
            idx.candidates["panic_tool"].len(),
            1,
            "fixture file must be discovered"
        );
        let out = OutputOpts {
            csv_root: Some(td.path().join("out")),
            json_root: None,
            overwrite: true,
            run_id: "20260710120000000".into(),
            hunt: false,
        };

        let res = run_tool_on_host_guarded(&entry, &host, &idx, &out);
        assert!(
            res.error.is_some(),
            "panicking parse must surface as a per-tool error, not abort the run"
        );
        assert_eq!(res.key, "panic_tool");
        assert_eq!(res.binary_name, "PanicTool");
        assert_eq!(res.parsed, 0);
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
        let entry = crate::registry::ToolEntry {
            key: "pe",
            tool: Box::new(pe_triage::PeTool::default()),
        };
        let idx = build_index(&host.artifact_root, std::slice::from_ref(&entry), &[]);
        let out = OutputOpts {
            csv_root: Some(td.path().join("out")),
            json_root: None,
            overwrite: true,
            run_id: "20260710120000000".into(),
            hunt: false,
        };
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
