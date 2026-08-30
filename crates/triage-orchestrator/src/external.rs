use crate::capture::HostCapture;
use crate::external_args::{
    hayabusa_csv_args, hayabusa_json_args, hayabusa_logon_summary_args, takajo_automagic_args,
};
use crate::external_bin::resolve_bin;
use crate::external_config::ResolvedConfig;
use serde::Serialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct ExternalToolReport {
    pub tool: String,
    pub found: bool,
    pub invoked: bool,
    pub exit_code: Option<i32>,
    pub output_paths: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn not_found(tool: &str) -> ExternalToolReport {
    ExternalToolReport {
        tool: tool.to_string(),
        found: false,
        invoked: false,
        exit_code: None,
        output_paths: Vec::new(),
        error: None,
    }
}

/// `find_outputs` is called only when the process exits successfully, and decides what
/// counts as "this tool's output" — a single file/dir existence check for most tools, or
/// a directory glob for logon-summary (which writes a variable number of `<prefix>-*.csv`
/// files, none at all if it found nothing to summarize).
fn invoke(
    bin: &Path,
    args: &[OsString],
    tool: &str,
    find_outputs: impl FnOnce() -> Vec<PathBuf>,
) -> ExternalToolReport {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    // Takajo (2.16.1) checks that its own executable exists relative to the process's
    // cwd and refuses to run otherwise, regardless of the absolute paths passed via its
    // own flags — it must be invoked from its own install directory. Setting this for
    // every external tool (not just Takajo) is a safe, general default.
    if let Some(parent) = bin.parent() {
        cmd.current_dir(parent);
    }
    match cmd.output() {
        Ok(out) => {
            let ok = out.status.success();
            // Only claim output paths in the manifest if they actually exist on disk —
            // a zero exit code alone doesn't guarantee the tool wrote anything (see
            // execute.rs's `result.output_paths.retain(|path| path.exists());` for the
            // same convention).
            let output_paths = if ok { find_outputs() } else { Vec::new() };
            let error = (!ok).then(|| {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if stderr.is_empty() {
                    format!("exited with status {:?}", out.status.code())
                } else {
                    stderr
                }
            });
            ExternalToolReport {
                tool: tool.to_string(),
                found: true,
                invoked: true,
                exit_code: out.status.code(),
                output_paths,
                error,
            }
        }
        Err(e) => ExternalToolReport {
            tool: tool.to_string(),
            found: true,
            invoked: false,
            exit_code: None,
            output_paths: Vec::new(),
            error: Some(e.to_string()),
        },
    }
}

/// A single-element output list if `path` exists, else empty. The common case for tools
/// that write exactly one known file/directory (everything but `logon-summary`).
fn path_if_exists(path: &Path) -> Vec<PathBuf> {
    if path.exists() {
        vec![path.to_path_buf()]
    } else {
        Vec::new()
    }
}

/// Files directly under `dir` whose basename starts with `prefix`, sorted for
/// deterministic reporting. Used for `logon-summary`, which writes a variable number of
/// `<prefix>-*.csv` files (none at all if it found nothing to summarize).
fn files_with_prefix(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut matches: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix))
        })
        .collect();
    matches.sort();
    matches
}

/// Run Hayabusa (up to three times: dfir-timeline csv / dfir-timeline jsonl /
/// logon-summary) and, if it produced a JSONL timeline and Takajo is enabled, chain
/// Takajo `automagic` off that output. One report per invocation attempted; a tool that
/// isn't found or isn't enabled contributes at most one report explaining why nothing
/// ran.
pub fn run_external_tools_for_host(
    resolved: &ResolvedConfig,
    host: &HostCapture,
    out_root: &Path,
) -> Vec<ExternalToolReport> {
    let mut reports = Vec::new();
    let host_dir = out_root.join(&host.output_id);
    let hayabusa_dir = host_dir.join("Hayabusa");
    let takajo_dir = host_dir.join("Takajo");
    let mut jsonl_output: Option<PathBuf> = None;

    if resolved.hayabusa.enabled {
        match resolve_bin(&resolved.hayabusa.bin) {
            Some(bin) => {
                if resolved.hayabusa.csv {
                    let _ = std::fs::create_dir_all(&hayabusa_dir);
                    let out_file = hayabusa_dir.join("timeline.csv");
                    let args =
                        hayabusa_csv_args(&resolved.hayabusa, &host.artifact_root, &out_file);
                    let check = out_file.clone();
                    reports.push(invoke(&bin, &args, "hayabusa-csv", || {
                        path_if_exists(&check)
                    }));
                }
                if resolved.hayabusa.json {
                    let _ = std::fs::create_dir_all(&hayabusa_dir);
                    let out_file = hayabusa_dir.join("timeline.jsonl");
                    let args =
                        hayabusa_json_args(&resolved.hayabusa, &host.artifact_root, &out_file);
                    let check = out_file.clone();
                    let report = invoke(&bin, &args, "hayabusa-json", || path_if_exists(&check));
                    // Gate the Takajo chain on the JSONL actually existing on disk, not
                    // merely on the subprocess reporting success with no error.
                    if out_file.is_file() {
                        jsonl_output = Some(out_file);
                    }
                    reports.push(report);
                }
                if resolved.hayabusa.logon_summary {
                    let _ = std::fs::create_dir_all(&hayabusa_dir);
                    let prefix_path = hayabusa_dir.join("logon-summary");
                    let args = hayabusa_logon_summary_args(
                        &resolved.hayabusa,
                        &host.artifact_root,
                        &prefix_path,
                    );
                    let dir = hayabusa_dir.clone();
                    reports.push(invoke(&bin, &args, "hayabusa-logon-summary", || {
                        files_with_prefix(&dir, "logon-summary")
                    }));
                }
            }
            None => reports.push(not_found("hayabusa")),
        }
    }

    if resolved.takajo.enabled {
        match jsonl_output {
            Some(jsonl) => match resolve_bin(&resolved.takajo.bin) {
                Some(bin) => {
                    // Takajo's `automagic -o` creates the leaf directory itself and
                    // refuses to run if it already exists — only ensure its parent
                    // (host_dir) is present, never pre-create takajo_dir.
                    let _ = std::fs::create_dir_all(&host_dir);
                    let args = takajo_automagic_args(&resolved.takajo, &jsonl, &takajo_dir);
                    let check = takajo_dir.clone();
                    reports.push(invoke(&bin, &args, "takajo-automagic", || {
                        path_if_exists(&check)
                    }));
                }
                None => reports.push(not_found("takajo")),
            },
            None => reports.push(ExternalToolReport {
                tool: "takajo-automagic".to_string(),
                found: true,
                invoked: false,
                exit_code: None,
                output_paths: Vec::new(),
                error: Some(
                    "skipped: hayabusa did not produce a JSONL timeline for this host".to_string(),
                ),
            }),
        }
    }

    reports
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external_config::{HayabusaConfig, TakajoConfig};
    use std::fs;
    use tempfile::TempDir;

    #[cfg(unix)]
    mod unix_stubs {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// Writes an executable stub script that, when invoked, scans its own argv for the
        /// `output_flag` and, for the argument immediately following it, either writes a
        /// placeholder file at that path (`as_dir == false`) or creates it as a directory
        /// containing a `report.txt` (`as_dir == true`) — enough to exercise the real
        /// orchestration/chaining logic without needing the actual 60MB+ binaries.
        pub(super) fn write_stub(
            dir: &Path,
            name: &str,
            output_flag: &str,
            as_dir: bool,
        ) -> PathBuf {
            let path = dir.join(name);
            let body = if as_dir {
                format!(
                    "#!/bin/sh\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"{output_flag}\" ]; then\n    mkdir -p \"$a\"\n    echo stub > \"$a/report.txt\"\n  fi\n  prev=\"$a\"\ndone\nexit 0\n"
                )
            } else {
                format!(
                    "#!/bin/sh\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"{output_flag}\" ]; then\n    echo stub > \"$a\"\n  fi\n  prev=\"$a\"\ndone\nexit 0\n"
                )
            };
            fs::write(&path, body).unwrap();
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
            path
        }
    }
    #[cfg(unix)]
    use unix_stubs::write_stub;

    #[cfg(unix)]
    #[test]
    fn chains_takajo_off_hayabusas_jsonl_output() {
        let td = TempDir::new().unwrap();
        let stub_dir = td.path().join("bin");
        fs::create_dir_all(&stub_dir).unwrap();
        let hayabusa_stub = write_stub(&stub_dir, "hayabusa", "--output", false);
        let takajo_stub = write_stub(&stub_dir, "takajo", "-o", true);

        let mut resolved = crate::external_config::ResolvedConfig {
            hayabusa: HayabusaConfig::default(),
            takajo: TakajoConfig::default(),
        };
        resolved.hayabusa.bin = hayabusa_stub.to_str().unwrap().to_string();
        resolved.takajo.bin = takajo_stub.to_str().unwrap().to_string();

        let host = HostCapture {
            host: "H".to_string(),
            output_id: "H".to_string(),
            os: "unknown".to_string(),
            collection_dir: td.path().to_path_buf(),
            artifact_root: td.path().to_path_buf(),
            source_archive: None,
        };
        let out_root = td.path().join("out");

        let reports = run_external_tools_for_host(&resolved, &host, &out_root);

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
        use std::os::unix::fs::PermissionsExt;

        let td = TempDir::new().unwrap();
        let stub_dir = td.path().join("bin");
        fs::create_dir_all(&stub_dir).unwrap();
        let hayabusa_stub = write_stub(&stub_dir, "hayabusa", "--output", false);
        let body = "#!/bin/sh\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then\n    if [ -d \"$a\" ]; then\n      echo \"directory already exists: $a\" >&2\n      exit 1\n    fi\n    mkdir -p \"$a\"\n    echo stub > \"$a/report.txt\"\n  fi\n  prev=\"$a\"\ndone\nexit 0\n";
        let takajo_stub = stub_dir.join("takajo");
        fs::write(&takajo_stub, body).unwrap();
        let mut perms = fs::metadata(&takajo_stub).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&takajo_stub, perms).unwrap();

        let mut resolved = crate::external_config::ResolvedConfig {
            hayabusa: HayabusaConfig::default(),
            takajo: TakajoConfig::default(),
        };
        resolved.hayabusa.bin = hayabusa_stub.to_str().unwrap().to_string();
        resolved.takajo.bin = takajo_stub.to_str().unwrap().to_string();

        let host = HostCapture {
            host: "H".to_string(),
            output_id: "H".to_string(),
            os: "unknown".to_string(),
            collection_dir: td.path().to_path_buf(),
            artifact_root: td.path().to_path_buf(),
            source_archive: None,
        };
        let out_root = td.path().join("out");
        let reports = run_external_tools_for_host(&resolved, &host, &out_root);

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
        use std::os::unix::fs::PermissionsExt;

        let td = TempDir::new().unwrap();
        let stub_dir = td.path().join("bin");
        fs::create_dir_all(&stub_dir).unwrap();
        let expected_cwd = stub_dir.canonicalize().unwrap();
        let body = format!(
            "#!/bin/sh\nif [ \"$(pwd -P)\" != \"{}\" ]; then\n  echo \"wrong cwd: $(pwd -P)\" >&2\n  exit 1\nfi\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--output\" ]; then\n    echo stub > \"$a\"\n  fi\n  prev=\"$a\"\ndone\nexit 0\n",
            expected_cwd.display()
        );
        let bin = stub_dir.join("cwd-sensitive-tool");
        fs::write(&bin, body).unwrap();
        let mut perms = fs::metadata(&bin).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&bin, perms).unwrap();

        let mut resolved = crate::external_config::ResolvedConfig {
            hayabusa: HayabusaConfig::default(),
            takajo: TakajoConfig::default(),
        };
        resolved.hayabusa.bin = bin.to_str().unwrap().to_string();
        resolved.hayabusa.json = false; // isolate to the csv invocation
        resolved.hayabusa.logon_summary = false; // isolate to the csv invocation
        resolved.takajo.enabled = false;

        let host = HostCapture {
            host: "H".to_string(),
            output_id: "H".to_string(),
            os: "unknown".to_string(),
            collection_dir: td.path().to_path_buf(),
            artifact_root: td.path().to_path_buf(),
            source_archive: None,
        };
        let reports = run_external_tools_for_host(&resolved, &host, &td.path().join("out"));

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
        let mut resolved = crate::external_config::ResolvedConfig {
            hayabusa: HayabusaConfig::default(),
            takajo: TakajoConfig::default(),
        };
        resolved.hayabusa.bin = "definitely-not-a-real-binary-xyz123".to_string();

        let host = HostCapture {
            host: "H".to_string(),
            output_id: "H".to_string(),
            os: "unknown".to_string(),
            collection_dir: td.path().to_path_buf(),
            artifact_root: td.path().to_path_buf(),
            source_archive: None,
        };
        let reports = run_external_tools_for_host(&resolved, &host, &td.path().join("out"));

        assert_eq!(reports.len(), 2); // hayabusa "not found" + takajo "skipped"
        assert_eq!(reports[0].tool, "hayabusa");
        assert!(!reports[0].found);
        assert_eq!(reports[1].tool, "takajo-automagic");
        assert!(reports[1].error.as_deref().unwrap().contains("skipped"));
    }

    #[test]
    fn disabled_tools_produce_no_reports() {
        let td = TempDir::new().unwrap();
        let mut resolved = crate::external_config::ResolvedConfig {
            hayabusa: HayabusaConfig::default(),
            takajo: TakajoConfig::default(),
        };
        resolved.hayabusa.enabled = false;
        resolved.takajo.enabled = false;

        let host = HostCapture {
            host: "H".to_string(),
            output_id: "H".to_string(),
            os: "unknown".to_string(),
            collection_dir: td.path().to_path_buf(),
            artifact_root: td.path().to_path_buf(),
            source_archive: None,
        };
        let reports = run_external_tools_for_host(&resolved, &host, &td.path().join("out"));
        assert!(reports.is_empty());
    }
}
