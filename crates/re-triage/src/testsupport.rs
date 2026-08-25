//! Test helpers for re-triage unit/integration tests. Compile-gated to
//! `#[cfg(test)]` so no test infrastructure leaks into production code.

use std::path::PathBuf;

/// Walk `root` recursively, collecting all file paths.
fn walkdir_min(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// Locate a hive by stem name (case-insensitive) under the shared read-only
/// test captures. Returns `(primary_path, log_paths)` or `None` when the
/// captures are absent.
pub fn find_hive(stem: &str) -> Option<(PathBuf, Vec<PathBuf>)> {
    // Navigate from the crate root to `../../test captures/`.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
    if !root.exists() {
        return None;
    }
    let stem_upper = stem.to_ascii_uppercase();
    for entry in walkdir_min(&root) {
        let fname = entry.file_name()?.to_string_lossy().to_string();
        if fname.to_ascii_uppercase() == stem_upper {
            let dir = entry.parent()?;
            let mut logs = Vec::new();
            for ext in [
                format!("{stem}.LOG1"),
                format!("{stem}.LOG2"),
                format!("{}.LOG1", stem.to_ascii_uppercase()),
                format!("{}.LOG2", stem.to_ascii_uppercase()),
            ] {
                let candidate = dir.join(&ext);
                if candidate.exists() && !logs.contains(&candidate) {
                    logs.push(candidate);
                }
            }
            logs.dedup();
            return Some((entry, logs));
        }
    }
    None
}
