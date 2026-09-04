use super::config::ResolvedConfig;
use super::invoke::{files_with_prefix, invoke, path_if_exists, resolve_bin};
use super::registry;
use super::report::{not_found, skipped, ExternalToolReport};
use super::tool::{Artifacts, HostContext, Invocation, OutputDirPolicy, OutputSpec};
use crate::capture::HostCapture;
use std::path::{Path, PathBuf};

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
pub fn run_external_tools_for_host(
    resolved: &ResolvedConfig,
    host: &HostCapture,
    out_root: &Path,
) -> Vec<ExternalToolReport> {
    let mut reports = Vec::new();
    let mut artifacts = Artifacts::default();
    // Computed once, here: every per-host output path must derive from
    // `output_id`, never the raw hostname, so a machine collected twice keeps a
    // stable directory per collection.
    let ctx = HostContext {
        host,
        host_dir: out_root.join(&host.output_id),
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
            let report = invoke(&bin, &inv.args, inv.report_name, || discover(outputs));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::test_host;
    use std::fs;
    use tempfile::TempDir;
    #[cfg(unix)]
    use triage_testkit::synthetic::{write_executable, write_stub};

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

        let reports = run_external_tools_for_host(&resolved, &test_host(td.path()), &out_root);

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

        let reports = run_external_tools_for_host(&resolved, &test_host(td.path()), &out_root);

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

        let reports =
            run_external_tools_for_host(&resolved, &test_host(td.path()), &td.path().join("out"));

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].exit_code, Some(0), "report: {:?}", reports[0]);
        assert!(
            reports[0].error.is_none(),
            "unexpected error: {:?}",
            reports[0].error
        );
    }

    #[test]
    fn hayabusa_not_found_reports_and_skips_takajo() {
        let td = TempDir::new().unwrap();
        let mut resolved = ResolvedConfig::default();
        resolved.hayabusa.bin = "definitely-not-a-real-binary-xyz123".to_string();

        let reports =
            run_external_tools_for_host(&resolved, &test_host(td.path()), &td.path().join("out"));

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

        let reports =
            run_external_tools_for_host(&resolved, &test_host(td.path()), &td.path().join("out"));
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

        let reports =
            run_external_tools_for_host(&resolved, &test_host(td.path()), &td.path().join("out"));

        // `read` fails at EOF, so reaching `exit 0` at all proves the process was
        // never left waiting for input.
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].exit_code, Some(0), "report: {:?}", reports[0]);
    }
}
