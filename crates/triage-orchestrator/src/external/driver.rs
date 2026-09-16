use super::config::{HayabusaConfig, ResolvedConfig};
use super::invoke::{files_with_prefix, invoke, path_if_exists, resolve_bin, CapturedOutput};
use super::registry;
use super::report::{not_found, skipped, ExternalToolReport};
use super::tool::{Artifacts, HostContext, Invocation, OutputDirPolicy, OutputSpec};
use crate::capture::HostCapture;
use crate::velo::proclog::ProcessLog;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Lexically collapse `.` and `..` components out of `path`, without
/// touching the filesystem (so it works on a path that doesn't exist yet,
/// unlike `Path::canonicalize`). `PathBuf::join` never does this on its
/// own: joining a tool's directory onto a literal `"./rules/config"` leaves
/// a `./` sitting in the middle of the result. That is harmless to the OS,
/// but it lands verbatim in `process_logs/*.log` and in the tool's own
/// error text, where it reads like a path-construction bug rather than the
/// deliberate join it is.
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Resolve `path` to an absolute path without requiring it to exist —
/// `std::fs::canonicalize` is the wrong tool here because several of the
/// paths this feeds (`host_dir`, `velo_dir`, a `work_dir` under either)
/// deliberately don't exist yet at this point; `prepare_dir` creates them
/// later.
///
/// Every external tool is invoked with its cwd set to its own install
/// directory, not the caller's (`invoke::invoke`'s `cmd.current_dir(parent)`
/// — Takajo refuses to run otherwise). A relative path baked into an
/// invocation's arguments before that cwd change then resolves against the
/// *tool's* directory instead of the one the user meant — `TriageSuite run
/// "test captures/..."` sent Hayabusa `--directory` pointed at its own
/// install directory, which matched nothing and silently produced zero
/// hits rather than an error. Every `plan()` in this module must see only
/// absolute paths so no tool can reproduce that class of bug from inside
/// its own `plan()`.
fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| lexically_normalize(&cwd.join(path)))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Resolve a Hayabusa-relative config path (`rules`/`rules_config`) against
/// Hayabusa's own install directory, *not* the invocation's cwd.
///
/// These are the one class of path this module deliberately does NOT run
/// through `absolutize`. Hayabusa ships its `rules/` (and `rules/config/`)
/// directories alongside its own binary, and — like every external tool —
/// runs with its cwd already set to that directory
/// (`invoke::invoke`'s `cmd.current_dir(parent)`), so the shipped
/// `triage.example.toml`'s `rules_config = "./rules/config"` is written to
/// resolve there by design, not against wherever the user happened to run
/// `triage` from. Feeding it through `absolutize` instead — resolving it
/// against the invocation cwd like the capture/output paths — was a
/// regression: it broke that exact config for anyone who runs `triage` from
/// outside Hayabusa's install directory, the common case.
///
/// Only a relative value is touched; an already-absolute one, or one that
/// can't be resolved because Hayabusa's binary itself can't be found (or
/// resolves to something with no parent directory), passes through
/// unchanged — the tool itself is left to report what's actually wrong with
/// it rather than this function guessing.
fn resolve_hayabusa_relative(
    value: &Option<String>,
    hayabusa_dir: Option<&Path>,
) -> Option<String> {
    let raw = value.as_deref()?;
    let path = Path::new(raw);
    if path.is_absolute() {
        return Some(raw.to_string());
    }
    match hayabusa_dir {
        Some(dir) => Some(
            lexically_normalize(&dir.join(path))
                .to_string_lossy()
                .into_owned(),
        ),
        None => Some(raw.to_string()),
    }
}

/// Create whatever the invocation's policy says the driver owns. Errors are
/// swallowed deliberately: if the directory can't be made, the tool itself will
/// fail with a far more specific message than we could invent here.
fn prepare_dir(inv: &Invocation) {
    match inv.dir_policy {
        OutputDirPolicy::CreateIfMissing => {
            let _ = std::fs::create_dir_all(&inv.work_dir);
        }
        OutputDirPolicy::ToolCreatesLeaf => {
            if let Some(parent) = inv.work_dir.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
    }
}

fn discover(spec: &OutputSpec) -> Vec<PathBuf> {
    match spec {
        OutputSpec::Path(path) => path_if_exists(path),
        OutputSpec::PrefixedIn { dir, prefix } => files_with_prefix(dir, prefix),
    }
}

/// Walk the external-tool registry once for one host, after that host's
/// in-process tools finish.
///
/// Registry order is execution order, report order, and dependency order all at
/// once: a tool's `requires()` slot is satisfied only from artifacts published by
/// tools already visited, so there is no second pass and no dependency graph.
///
/// Per tool the gates run in a fixed order that is itself load-bearing —
/// enabled, then prerequisite, then binary resolution, then plan. Checking the
/// prerequisite before resolving the binary is why a tool with nothing to consume
/// reports "skipped" rather than "not found on PATH", even when both are true.
///
/// One report per invocation attempted; a tool that is disabled contributes none,
/// and a tool that can't run contributes exactly one explaining why.
///
/// `collection_dir` is `Some(<Processed-HOST-stamp>)` under `--layout velo`
/// and `None` under `--layout native` — the same collection directory
/// `crate::velo::collection_dir_for` produces for `main.rs`'s process-log,
/// source-hash-log, and SysInfo gating. It serves two purposes here: process
/// logs (below) and, via `HostContext::velo_dir`, the Velo-category
/// placement of tools like Hayabusa and Takajo whose output belongs under
/// `<collection_dir>/<Category>` rather than the native `host_dir`.
///
/// Every invocation that actually ran a process gets its real stdout/stderr
/// written to `process_logs/<report_name>.log` when `collection_dir` is
/// `Some`; a tool that never started one (disabled, not found, or skipped
/// for a missing prerequisite) has no process output to write and gets no
/// log file.
pub fn run_external_tools_for_host(
    resolved: &ResolvedConfig,
    host: &HostCapture,
    out_root: &Path,
    collection_dir: Option<&Path>,
    overwrite: bool,
) -> Vec<ExternalToolReport> {
    let mut reports = Vec::new();
    let mut artifacts = Artifacts::default();
    // Every path a tool's `plan()` sees must already be absolute — see
    // `absolutize`'s doc comment — so this is resolved once, here, rather
    // than repeated (or, worse, missed) in each tool's own `plan()`.
    let host = HostCapture {
        artifact_root: absolutize(&host.artifact_root),
        ..host.clone()
    };
    let out_root = absolutize(out_root);
    let collection_dir = collection_dir.map(absolutize);
    // Hayabusa's own install directory, for resolving its tool-relative
    // `rules`/`rules_config` config below -- see
    // `resolve_hayabusa_relative`'s doc comment for why these must NOT go
    // through `absolutize`. Resolved once, cheaply (a `PATH` lookup or an
    // `is_file` check), ahead of the per-tool loop's own `resolve_bin` call.
    let hayabusa_dir = resolve_bin(&resolved.hayabusa.bin)
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    let resolved = ResolvedConfig {
        hayabusa: HayabusaConfig {
            rules: resolve_hayabusa_relative(&resolved.hayabusa.rules, hayabusa_dir.as_deref()),
            rules_config: resolve_hayabusa_relative(
                &resolved.hayabusa.rules_config,
                hayabusa_dir.as_deref(),
            ),
            ..resolved.hayabusa.clone()
        },
        ..resolved.clone()
    };
    let resolved = &resolved;
    // Computed once, here: every per-host output path must derive from
    // `output_id`, never the raw hostname, so a machine collected twice keeps a
    // stable directory per collection.
    let ctx = HostContext {
        host: &host,
        host_dir: out_root.join(&host.output_id),
        velo_dir: collection_dir.clone(),
    };

    for tool in registry::ALL {
        if !tool.enabled(resolved) {
            continue;
        }

        if let Some(req) = tool.requires() {
            if artifacts.get(req.slot).is_none() {
                reports.push(skipped(req.report_name, req.skipped_message));
                continue;
            }
        }

        let Some(bin) = resolve_bin(tool.bin(resolved)) else {
            reports.push(not_found(tool.key()));
            continue;
        };

        for inv in tool.plan(resolved, &ctx, &artifacts) {
            prepare_dir(&inv);
            let outputs = &inv.outputs;
            let (report, captured) = invoke(&bin, &inv.args, inv.report_name, || discover(outputs));
            write_process_log(
                collection_dir.as_deref(),
                &bin,
                &inv.args,
                &report,
                &captured,
                overwrite,
            );
            // Publish on a real filesystem check, not on the exit status: a tool
            // can report success and still write nothing.
            if let Some(publish) = &inv.publishes {
                if publish.path.is_file() {
                    artifacts.publish(publish.slot, publish.path.clone());
                }
            }
            reports.push(report);
        }
    }

    reports
}

/// Write one invocation's real stdout/stderr to `process_logs/<report_name>.log`,
/// headed by the exact command line that was run (binary + args, no
/// environment — this process never sets any tool-specific env vars, and
/// logging the ambient environment would risk leaking secrets that happen to
/// be set in it). Best-effort like every other process-log write
/// (`ProcessLog::line`'s doc comment): a full disk or an unwritable output
/// root must not turn a successful external-tool run into a failed one, so
/// `ProcessLog::open` failing here is silently skipped rather than surfaced.
fn write_process_log(
    process_log_dir: Option<&Path>,
    bin: &Path,
    args: &[OsString],
    report: &ExternalToolReport,
    captured: &CapturedOutput,
    overwrite: bool,
) {
    let Some(dir) = process_log_dir else { return };
    // `write_process_log` is only ever reached for a report `invoke` (above,
    // at this function's one call site) actually produced -- a disabled,
    // not-found, or prerequisite-skipped tool `continue`s earlier in
    // `run_external_tools_for_host` and never gets here. So `invoke` *was*
    // called; what `!report.invoked` catches is its `Err(e)` arm
    // (`invoke.rs`) -- the process never started (e.g. exec permission
    // denied), so there is genuinely no stdout/stderr to have captured, and
    // the failure itself is already in `report.error`.
    if !report.invoked {
        return;
    }
    // One log per `report_name`, and every `Invocation` in the registry
    // carries a distinct one (Hayabusa's three plan arms are `hayabusa-csv`,
    // `hayabusa-json` and `hayabusa-logon-summary`), so this never reopens a
    // path it wrote earlier in the same run -- which is why it can hold
    // `ProcessLog::open` to the same `--overwrite` contract as every other
    // output rather than needing an append mode.
    let Ok(mut log) = ProcessLog::open(dir, &report.tool, overwrite) else {
        return;
    };
    let command_line: String = std::iter::once(bin.to_string_lossy().into_owned())
        .chain(args.iter().map(|a| a.to_string_lossy().into_owned()))
        .collect::<Vec<_>>()
        .join(" ");
    log.line(&format!("--- command ---\n{command_line}"));
    log.line("--- stdout ---");
    log.line(&captured.stdout);
    log.line("--- stderr ---");
    log.line(&captured.stderr);
    if let Some(exit_code) = report.exit_code {
        log.line(&format!("exit code: {exit_code}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::test_host;
    use std::fs;
    use tempfile::TempDir;
    #[cfg(unix)]
    use triage_testkit::synthetic::{write_executable, write_stub};

    #[test]
    fn lexically_normalize_strips_current_dir_components() {
        assert_eq!(
            lexically_normalize(Path::new("/a/./b/./c")),
            Path::new("/a/b/c")
        );
    }

    #[test]
    fn lexically_normalize_collapses_parent_dir_against_a_normal_component() {
        assert_eq!(
            lexically_normalize(Path::new("/a/b/../c")),
            Path::new("/a/c")
        );
    }

    /// A leading `..` with nothing to collapse against is kept, not
    /// dropped -- this is a lexical (string-level) normalization, not a
    /// filesystem resolution, so it must not silently change what a
    /// genuinely-escaping relative path means.
    #[test]
    fn lexically_normalize_keeps_a_leading_parent_dir() {
        assert_eq!(
            lexically_normalize(Path::new("../a/b")),
            Path::new("../a/b")
        );
    }

    #[test]
    fn lexically_normalize_is_a_no_op_on_an_already_clean_path() {
        assert_eq!(
            lexically_normalize(Path::new("/a/b/c")),
            Path::new("/a/b/c")
        );
    }

    /// A config whose only difference from the defaults is that the two
    /// binaries resolve to the given paths.
    fn config_with_bins(hayabusa: &Path, takajo: &Path) -> ResolvedConfig {
        let mut resolved = ResolvedConfig::default();
        resolved.hayabusa.bin = hayabusa.to_str().unwrap().to_string();
        resolved.takajo.bin = takajo.to_str().unwrap().to_string();
        resolved
    }

    /// A config that runs exactly one invocation: hayabusa-csv against `bin`.
    fn csv_only_config(bin: &Path) -> ResolvedConfig {
        let mut resolved = ResolvedConfig::default();
        resolved.hayabusa.bin = bin.to_str().unwrap().to_string();
        resolved.hayabusa.json = false;
        resolved.hayabusa.logon_summary = false;
        resolved.takajo.enabled = false;
        resolved
    }

    fn stub_dir(td: &TempDir) -> PathBuf {
        let dir = td.path().join("bin");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn chains_takajo_off_hayabusas_jsonl_output() {
        let td = TempDir::new().unwrap();
        let stubs = stub_dir(&td);
        let resolved = config_with_bins(
            &write_stub(&stubs, "hayabusa", "--output", false),
            &write_stub(&stubs, "takajo", "-o", true),
        );
        let out_root = td.path().join("out");

        let reports =
            run_external_tools_for_host(&resolved, &test_host(td.path()), &out_root, None, false);

        let names: Vec<&str> = reports.iter().map(|r| r.tool.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "hayabusa-csv",
                "hayabusa-json",
                "hayabusa-logon-summary",
                "takajo-automagic"
            ]
        );
        for r in &reports {
            assert!(r.found, "{}: expected found", r.tool);
            assert!(r.invoked, "{}: expected invoked", r.tool);
            assert_eq!(r.exit_code, Some(0), "{}: expected exit 0", r.tool);
            assert!(
                r.error.is_none(),
                "{}: unexpected error {:?}",
                r.tool,
                r.error
            );
        }
        assert!(out_root.join("H/Hayabusa/timeline.csv").is_file());
        assert!(out_root.join("H/Hayabusa/timeline.jsonl").is_file());
        // The generic stub writes a single flat file at whatever path follows
        // `--output`, unlike real Hayabusa's two `<prefix>-*.csv` files — enough to
        // exercise files_with_prefix's discovery without needing the real binary.
        assert!(out_root.join("H/Hayabusa/logon-summary").is_file());
        assert!(out_root.join("H/Takajo/report.txt").is_file());
    }

    /// Real Takajo (2.16.1) `automagic -o` refuses to run if the target directory
    /// already exists ("Please specify a new folder name") — it creates the leaf
    /// directory itself and expects only the parent to exist. This stub reproduces
    /// that: it fails if its `-o` target directory is already present.
    #[cfg(unix)]
    #[test]
    fn does_not_pre_create_the_takajo_output_directory() {
        let td = TempDir::new().unwrap();
        let stubs = stub_dir(&td);
        let takajo_stub = stubs.join("takajo");
        write_executable(
            &takajo_stub,
            "#!/bin/sh\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then\n    if [ -d \"$a\" ]; then\n      echo \"directory already exists: $a\" >&2\n      exit 1\n    fi\n    mkdir -p \"$a\"\n    echo stub > \"$a/report.txt\"\n  fi\n  prev=\"$a\"\ndone\nexit 0\n",
        );
        let resolved = config_with_bins(
            &write_stub(&stubs, "hayabusa", "--output", false),
            &takajo_stub,
        );
        let out_root = td.path().join("out");

        let reports =
            run_external_tools_for_host(&resolved, &test_host(td.path()), &out_root, None, false);

        let takajo_report = reports
            .iter()
            .find(|r| r.tool == "takajo-automagic")
            .unwrap();
        assert_eq!(
            takajo_report.exit_code,
            Some(0),
            "report: {takajo_report:?}"
        );
        assert!(out_root.join("H/Takajo/report.txt").is_file());
    }

    /// Real Takajo (2.16.1) checks that its own executable exists relative to the
    /// process's current working directory and refuses to run otherwise — it must be
    /// invoked with cwd set to its own install directory, regardless of the absolute
    /// paths passed via `-t`/`-o`. This stub reproduces that requirement: it only
    /// succeeds when invoked with cwd == its own directory.
    #[cfg(unix)]
    #[test]
    fn invokes_the_tool_with_cwd_set_to_its_own_directory() {
        let td = TempDir::new().unwrap();
        let stubs = stub_dir(&td);
        let expected_cwd = stubs.canonicalize().unwrap();
        let bin = stubs.join("cwd-sensitive-tool");
        write_executable(
            &bin,
            &format!(
                "#!/bin/sh\nif [ \"$(pwd -P)\" != \"{}\" ]; then\n  echo \"wrong cwd: $(pwd -P)\" >&2\n  exit 1\nfi\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--output\" ]; then\n    echo stub > \"$a\"\n  fi\n  prev=\"$a\"\ndone\nexit 0\n",
                expected_cwd.display()
            ),
        );
        let resolved = csv_only_config(&bin);

        let reports = run_external_tools_for_host(
            &resolved,
            &test_host(td.path()),
            &td.path().join("out"),
            None,
            false,
        );

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].exit_code, Some(0), "report: {:?}", reports[0]);
        assert!(
            reports[0].error.is_none(),
            "unexpected error: {:?}",
            reports[0].error
        );
    }

    /// A relative `hayabusa.rules_config` must resolve against Hayabusa's own
    /// install directory, NOT the process's invocation cwd -- the regression
    /// this guards against had it resolving against the latter, breaking the
    /// shipped `triage.example.toml`'s `rules_config = "./rules/config"` for
    /// anyone who runs `triage` from anywhere but Hayabusa's own directory.
    /// The stub fails loudly if `--rules-config`'s value is anything other
    /// than `<hayabusa's own dir>/rules/config`, and this test's invocation
    /// cwd (the test process's cwd) is asserted to differ from that
    /// directory, so a regression back to cwd-relative resolution cannot
    /// pass by accident.
    #[cfg(unix)]
    #[test]
    fn relative_rules_config_resolves_against_hayabusas_own_directory() {
        let td = TempDir::new().unwrap();
        let stubs = stub_dir(&td);
        // `resolve_hayabusa_relative` joins onto the bin's own
        // (uncanonicalized) parent directory, exactly as `resolve_bin`
        // returns it for an explicit path -- so the expectation here must
        // match that (no canonicalized symlink resolution), but WITHOUT the
        // literal "./" `lexically_normalize` strips out before this reaches
        // the tool's argv.
        let expected = stubs.join("rules/config");
        assert_ne!(
            stubs.canonicalize().unwrap(),
            std::env::current_dir().unwrap(),
            "test setup must not coincide with the real regression's blind spot"
        );
        let bin = stubs.join("hayabusa");
        write_executable(
            &bin,
            &format!(
                "#!/bin/sh\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--rules-config\" ]; then\n    if [ \"$a\" != \"{expected}\" ]; then\n      echo \"wrong --rules-config: $a\" >&2\n      exit 1\n    fi\n  fi\n  if [ \"$prev\" = \"--output\" ]; then\n    echo stub > \"$a\"\n  fi\n  prev=\"$a\"\ndone\nexit 0\n",
                expected = expected.display()
            ),
        );
        let mut resolved = csv_only_config(&bin);
        resolved.hayabusa.rules_config = Some("./rules/config".to_string());

        let reports = run_external_tools_for_host(
            &resolved,
            &test_host(td.path()),
            &td.path().join("out"),
            None,
            false,
        );

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].exit_code, Some(0), "report: {:?}", reports[0]);
        assert!(
            reports[0].error.is_none(),
            "unexpected error: {:?}",
            reports[0].error
        );
    }

    /// Exercises `invoke`'s `Err(e)` spawn-failure arm -- the one branch
    /// `write_process_log`'s `!report.invoked` check actually catches
    /// (`driver.rs`'s doc comment on that check). `resolve_bin` accepts this
    /// path (it's a regular file at an explicit path), but `Command::output`
    /// fails to spawn it because it isn't executable, so the process never
    /// starts: `found: true`, `invoked: false`, an error, and -- the point of
    /// this test -- no `process_logs/<tool>.log` written, since there is no
    /// process output to have captured.
    #[cfg(unix)]
    #[test]
    fn a_spawn_failure_is_reported_and_writes_no_process_log() {
        use std::os::unix::fs::PermissionsExt;

        let td = TempDir::new().unwrap();
        let stubs = stub_dir(&td);
        let not_executable = stubs.join("hayabusa");
        fs::write(&not_executable, b"not a real binary\n").unwrap();
        fs::set_permissions(&not_executable, std::fs::Permissions::from_mode(0o644)).unwrap();
        let resolved = csv_only_config(&not_executable);
        let process_log_dir = td.path().join("collection");

        let reports = run_external_tools_for_host(
            &resolved,
            &test_host(td.path()),
            &td.path().join("out"),
            Some(&process_log_dir),
            false,
        );

        assert_eq!(reports.len(), 1);
        assert!(reports[0].found, "resolve_bin should still find the path");
        assert!(
            !reports[0].invoked,
            "a process that never started must report invoked: false"
        );
        assert!(reports[0].error.is_some(), "the spawn failure must surface");
        assert!(
            !process_log_dir
                .join("process_logs/hayabusa-csv.log")
                .exists(),
            "no process output was captured, so no process log should be written"
        );
    }

    #[test]
    fn hayabusa_not_found_reports_and_skips_takajo() {
        let td = TempDir::new().unwrap();
        let mut resolved = ResolvedConfig::default();
        resolved.hayabusa.bin = "definitely-not-a-real-binary-xyz123".to_string();

        let reports = run_external_tools_for_host(
            &resolved,
            &test_host(td.path()),
            &td.path().join("out"),
            None,
            false,
        );

        assert_eq!(reports.len(), 2); // hayabusa "not found" + takajo "skipped"
        assert_eq!(reports[0].tool, "hayabusa");
        assert!(!reports[0].found);
        assert_eq!(reports[1].tool, "takajo-automagic");
        assert!(reports[1].error.as_deref().unwrap().contains("skipped"));
    }

    #[test]
    fn disabled_tools_produce_no_reports() {
        let td = TempDir::new().unwrap();
        let mut resolved = ResolvedConfig::default();
        resolved.hayabusa.enabled = false;
        resolved.takajo.enabled = false;

        let reports = run_external_tools_for_host(
            &resolved,
            &test_host(td.path()),
            &td.path().join("out"),
            None,
            false,
        );
        assert!(reports.is_empty());
    }

    /// A tool that reads stdin would block forever on an inherited terminal,
    /// with nothing in the output to say why. Every external invocation runs
    /// with stdin closed, so the read returns EOF immediately.
    #[cfg(unix)]
    #[test]
    fn external_tools_run_with_stdin_closed() {
        let td = TempDir::new().unwrap();
        let bin = stub_dir(&td).join("hayabusa");
        write_executable(&bin, "#!/bin/sh\nread line\nexit 0\n");
        let resolved = csv_only_config(&bin);

        let reports = run_external_tools_for_host(
            &resolved,
            &test_host(td.path()),
            &td.path().join("out"),
            None,
            false,
        );

        // `read` fails at EOF, so reaching `exit 0` at all proves the process was
        // never left waiting for input.
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].exit_code, Some(0), "report: {:?}", reports[0]);
    }
}
