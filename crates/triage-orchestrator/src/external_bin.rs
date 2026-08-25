use std::path::{Path, PathBuf};

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
