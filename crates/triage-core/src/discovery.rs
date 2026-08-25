use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct DiscoveryReport {
    /// Candidate files matching the patterns, sorted by path for determinism.
    pub files: Vec<PathBuf>,
    /// Entries that could not be read; reported in the summary, never fatal.
    pub inaccessible: u64,
}

fn build_globs(patterns: &[&str]) -> GlobSet {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        b.add(
            GlobBuilder::new(p)
                .case_insensitive(true)
                .build()
                .expect("tool-defined patterns are valid globs"),
        );
    }
    b.build().expect("tool-defined patterns are valid globs")
}

/// Recursively walk `root` for files whose names match `patterns`
/// (case-insensitive), never following symlinks, skipping any `exclude`
/// directories (output dirs nested beneath the input), counting
/// inaccessible entries, and calling `tick` per visited entry so the
/// caller can drive a discovery spinner. Spec section 3.6.
///
/// Patterns match against the filename component only; path-component
/// patterns like `dir/*.ext` will never match.
pub fn discover(
    root: &Path,
    patterns: &[&str],
    exclude: &[PathBuf],
    tick: &mut dyn FnMut(&Path),
) -> DiscoveryReport {
    let globs = build_globs(patterns);
    let mut files = Vec::new();
    let inaccessible = walk_files(root, exclude, tick, &mut |path| {
        if path
            .file_name()
            .is_some_and(|name| globs.is_match(Path::new(name)))
        {
            files.push(path.to_path_buf());
        }
    });
    files.sort();
    DiscoveryReport {
        files,
        inaccessible,
    }
}

/// Walk every regular file once and dispatch it immediately to `on_file`.
pub fn walk_files(
    root: &Path,
    exclude: &[PathBuf],
    tick: &mut dyn FnMut(&Path),
    on_file: &mut dyn FnMut(&Path),
) -> u64 {
    // Canonicalize the root once. Excludes that live under the input root are
    // rebased onto the canonical root lexically (no syscall per entry).
    // Excludes outside the root fall back to their own canonical form.
    // This avoids ~one syscall per walked file and fixes a bug where broken
    // symlinks returned a non-canonical path that never matched the canonical
    // excludes, silently disabling exclusion on macOS (/tmp → /private/tmp).
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let exclude: Vec<PathBuf> = exclude
        .iter()
        .map(|p| {
            if let Ok(stripped) = p.strip_prefix(root) {
                canon_root.join(stripped)
            } else {
                p.canonicalize().unwrap_or_else(|_| p.clone())
            }
        })
        .collect();
    let mut inaccessible = 0u64;

    let walker = WalkDir::new(root).follow_links(false).into_iter();
    let mut it = walker.filter_entry(|e| {
        let rebased = match e.path().strip_prefix(root) {
            Ok(stripped) => canon_root.join(stripped),
            Err(_) => e.path().to_path_buf(),
        };
        !exclude.iter().any(|x| rebased.starts_with(x))
    });
    for entry in &mut it {
        match entry {
            Err(_) => inaccessible += 1,
            Ok(e) => {
                tick(e.path());
                if e.file_type().is_file() {
                    on_file(e.path());
                }
            }
        }
    }
    inaccessible
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(p: &std::path::Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    #[test]
    fn finds_matching_files_case_insensitively_and_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("C/Users/a/Recent/B.STUB"));
        touch(&tmp.path().join("C/Users/a/Recent/a.stub"));
        touch(&tmp.path().join("C/Windows/notes.txt"));
        let report = discover(tmp.path(), &["*.stub"], &[], &mut |_| {});
        let names: Vec<_> = report
            .files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // byte-wise path sort: uppercase 'B' precedes lowercase 'a'
        assert_eq!(names, vec!["B.STUB", "a.stub"]);
    }

    #[test]
    fn excludes_output_directory_under_input() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("data/a.stub"));
        touch(&tmp.path().join("out/StubTriage/system/b.stub"));
        let out = tmp.path().join("out");
        let report = discover(tmp.path(), &["*.stub"], &[out], &mut |_| {});
        assert_eq!(report.files.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("real/a.stub"));
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("link")).unwrap();
        let report = discover(tmp.path(), &["*.stub"], &[], &mut |_| {});
        assert_eq!(report.files.len(), 1);
    }

    #[test]
    fn ticks_during_walk() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("a/b.stub"));
        let mut ticks = 0u32;
        discover(tmp.path(), &["*.stub"], &[], &mut |_| ticks += 1);
        assert!(ticks > 0);
    }

    /// Regression test for macOS-style symlinked temp dirs (/tmp → /private/tmp).
    ///
    /// We walk the REAL path but pass the exclude directory via a SYMLINKED
    /// path.  The exclude is not a prefix of the raw root, so it falls into the
    /// `canonicalize` branch of the exclude-rebase logic.  After
    /// canonicalization both paths are equivalent, so the exclusion must still
    /// fire.  This is the variant we chose because WalkDir with
    /// follow_links(false) special-cases the root symlink and may yield no
    /// entries if the root itself is a symlink, making the walk-via-symlink
    /// variant unreliable across platforms.
    #[cfg(unix)]
    #[test]
    fn exclusion_works_when_root_is_reached_via_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("cap/data/a.stub"));
        touch(&tmp.path().join("cap/out/T/system/b.stub"));
        let link = tmp.path().join("caplink");
        std::os::unix::fs::symlink(tmp.path().join("cap"), &link).unwrap();
        // Walk via the real path, but supply the exclude via the symlink path.
        // The exclude is not under `root` (they differ), so it canonicalizes
        // to the same location as the real out dir — exclusion must still work.
        let report = discover(
            &tmp.path().join("cap"),
            &["*.stub"],
            &[link.join("out")],
            &mut |_| {},
        );
        assert_eq!(
            report.files.len(),
            1,
            "expected only data/a.stub; got {} files",
            report.files.len()
        );
        assert!(report.files[0].ends_with("data/a.stub"));
    }
}
