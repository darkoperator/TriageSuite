use super::report::ExternalToolReport;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve a configured `bin` value to an executable path: an explicit path (containing a
/// separator) is checked directly; a bare name is looked up on `PATH`, first match wins.
/// Returns `None` if nothing resolves — the caller treats that as "tool not available."
pub fn resolve_bin(configured: &str) -> Option<PathBuf> {
    let candidate = Path::new(configured);
    if candidate.components().count() > 1 {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(configured))
        .find(|p| p.is_file())
}

/// Run `bin` with `args` to completion and fold the outcome into a report.
///
/// `find_outputs` is called only when the process exits successfully, and decides what
/// counts as "this tool's output" — a single file/dir existence check for most tools, or
/// a directory glob for logon-summary (which writes a variable number of `<prefix>-*.csv`
/// files, none at all if it found nothing to summarize).
pub(super) fn invoke(
    bin: &Path,
    args: &[OsString],
    tool: &str,
    find_outputs: impl FnOnce() -> Vec<PathBuf>,
) -> ExternalToolReport {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    // No external tool is ever attached to a terminal here, and one that decides
    // to prompt would hang the run forever with nothing in the output to say why
    // (several forensic CLIs drop into an interactive menu when they think a
    // human is present). Closing stdin turns that class of hazard into an
    // immediate EOF — the same kind of general safe default as the cwd rule
    // below.
    cmd.stdin(std::process::Stdio::null());
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
pub(super) fn path_if_exists(path: &Path) -> Vec<PathBuf> {
    if path.exists() {
        vec![path.to_path_buf()]
    } else {
        Vec::new()
    }
}

/// Files directly under `dir` whose basename starts with `prefix`, sorted for
/// deterministic reporting. Used for `logon-summary`, which writes a variable number of
/// `<prefix>-*.csv` files (none at all if it found nothing to summarize).
pub(super) fn files_with_prefix(dir: &Path, prefix: &str) -> Vec<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn resolves_an_explicit_path_to_an_existing_file() {
        let td = TempDir::new().unwrap();
        let bin = td.path().join("my-tool");
        fs::write(&bin, b"#!/bin/sh\n").unwrap();
        assert_eq!(resolve_bin(bin.to_str().unwrap()), Some(bin));
    }

    #[test]
    fn explicit_path_to_a_missing_file_resolves_to_none() {
        let td = TempDir::new().unwrap();
        let missing = td.path().join("nope");
        assert_eq!(resolve_bin(missing.to_str().unwrap()), None);
    }

    #[cfg(unix)]
    #[test]
    fn bare_name_resolves_via_path() {
        // `sh` is present on every unix CI/dev runner this suite targets.
        assert!(resolve_bin("sh").is_some());
    }

    #[test]
    fn unknown_bare_name_resolves_to_none() {
        assert_eq!(resolve_bin("definitely-not-a-real-binary-xyz123"), None);
    }
}
