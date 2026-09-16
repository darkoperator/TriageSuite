//! Passthrough of Velociraptor's own result JSONs.

use std::path::{Path, PathBuf};
use triage_core::error::TriageError;

/// Copy `<extraction>/results/` to `<collection_dir>/VeloResults/`.
/// `Ok(None)` when the capture has no results directory.
///
/// Not a byte-for-byte copy of the tree in one respect: **symlinks are
/// skipped, never followed or recreated**, so a capture whose `results/`
/// contains one comes out of this passthrough with that entry missing. The
/// reason is in `copy_tree` below -- a capture is untrusted input, and
/// following a link in one would let the copy read or write outside both the
/// capture and the output tree. Callers that present `VeloResults/` as a
/// faithful passthrough should say "every regular file", not "everything".
pub fn copy_velo_results(
    extraction: &Path,
    collection_dir: &Path,
) -> Result<Option<PathBuf>, TriageError> {
    let source = extraction.join("results");
    // `symlink_metadata`, not `is_dir()`: `is_dir()` follows symlinks, so a
    // capture whose `results` is itself a symlink to an external directory
    // would have this walk straight into that directory and copy its
    // regular files into `VeloResults/` -- which the hash walk then
    // attests to as collected evidence. A symlinked root is treated the
    // same as "no results directory": silently skipped, matching how a
    // symlinked child is silently skipped in `copy_tree` below.
    let is_real_dir = std::fs::symlink_metadata(&source)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false);
    if !is_real_dir {
        return Ok(None);
    }
    let destination = collection_dir.join("VeloResults");
    copy_tree(&source, &destination)?;
    Ok(Some(destination))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), TriageError> {
    std::fs::create_dir_all(destination).map_err(|e| TriageError::Output {
        path: destination.to_path_buf(),
        message: e.to_string(),
    })?;
    for entry in std::fs::read_dir(source).map_err(|e| TriageError::Output {
        path: source.to_path_buf(),
        message: e.to_string(),
    })? {
        let entry = entry.map_err(|e| TriageError::Output {
            path: source.to_path_buf(),
            message: e.to_string(),
        })?;
        let from = entry.path();
        let to = destination.join(entry.file_name());

        // symlink_metadata, not metadata: a capture is untrusted input, and
        // following a symlink in one would let the copy read or write outside
        // the capture and the output tree. Links are skipped, not resolved.
        //
        // Hard links are deliberately out of scope: they are indistinguishable
        // from an ordinary file here (`file_type().is_symlink()` is false and
        // there is no separate "is a hard link" to test), they cannot point
        // outside the filesystem they live on the way a symlink can, and a
        // hard-linked file *is* the data, so copying it is correct rather
        // than a traversal. The cost is duplicated bytes when a capture hard
        // links the same result twice, which is a size question, not an
        // evidence one.
        let meta = std::fs::symlink_metadata(&from).map_err(|e| TriageError::Output {
            path: from.clone(),
            message: e.to_string(),
        })?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| TriageError::Output {
                path: from.clone(),
                message: e.to_string(),
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_are_copied_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let extraction = tmp.path().join("extracted");
        std::fs::create_dir_all(extraction.join("results/sub")).unwrap();
        std::fs::write(extraction.join("results/a.json"), "{}").unwrap();
        std::fs::write(extraction.join("results/sub/b.json"), "{}").unwrap();
        let collection = tmp.path().join("Processed-WS01-X");
        std::fs::create_dir_all(&collection).unwrap();

        let dest = copy_velo_results(&extraction, &collection)
            .unwrap()
            .unwrap();
        assert!(dest.join("a.json").is_file());
        assert!(dest.join("sub/b.json").is_file());
    }

    #[test]
    fn a_capture_without_results_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let extraction = tmp.path().join("extracted");
        std::fs::create_dir_all(&extraction).unwrap();
        let collection = tmp.path().join("c");
        std::fs::create_dir_all(&collection).unwrap();
        assert!(copy_velo_results(&extraction, &collection)
            .unwrap()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_results_root_is_skipped_not_followed() {
        use std::os::unix::fs::symlink;

        // `results` itself -- not a child of it -- is a symlink to a
        // directory outside the capture. `source.is_dir()` follows
        // symlinks, so without a `symlink_metadata` check on the root this
        // walks straight into the external directory and copies its
        // regular files into `VeloResults/`, and the subsequent hash walk
        // then attests to them as collected evidence.
        let tmp = tempfile::tempdir().unwrap();
        let extraction = tmp.path().join("extracted");
        std::fs::create_dir_all(&extraction).unwrap();

        let outside = tmp.path().join("outside_dir");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.json"), "secret").unwrap();
        symlink(&outside, extraction.join("results")).unwrap();

        let collection = tmp.path().join("Processed-WS01-X");
        std::fs::create_dir_all(&collection).unwrap();

        // A symlinked root is treated the same as "no results directory":
        // silently skipped, consistent with how a symlinked child is
        // silently skipped below, and with no channel to report a reason
        // to the caller (`main.rs` treats `Err` as fatal, so a symlinked
        // root cannot be surfaced as anything short of that without
        // changing what a plain "no results" capture does too).
        assert!(copy_velo_results(&extraction, &collection)
            .unwrap()
            .is_none());
        assert!(!collection.join("VeloResults").exists());

        // The hash walk is the attestation, not the copy: a file absent
        // from `VeloResults/` but still named in `OutputHashes.txt` would
        // be a worse defect than the one this test guards against. Run
        // the real hash walk over the collection directory and confirm
        // the external file's name and content never reach it.
        let hashes_path =
            crate::velo::hashes::write_output_hashes(&collection, "2026-03-13T192553Z").unwrap();
        let hashes_body = std::fs::read_to_string(hashes_path).unwrap();
        assert!(
            !hashes_body.contains("secret.json"),
            "external file must not be attested: {hashes_body}"
        );
        let secret_digest =
            crate::velo::hashes::sha256_file(&outside.join("secret.json")).unwrap_or_default();
        assert!(
            !hashes_body.contains(&secret_digest),
            "external file's content must not be attested: {hashes_body}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_results_is_skipped_not_followed() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let extraction = tmp.path().join("extracted");
        std::fs::create_dir_all(extraction.join("results")).unwrap();
        std::fs::write(extraction.join("results/a.json"), "{}").unwrap();

        // A symlink escaping the results tree, e.g. pointing at a secret
        // outside the capture. It must be skipped, never resolved and copied.
        let outside = tmp.path().join("outside_secret");
        std::fs::write(&outside, "secret").unwrap();
        symlink(&outside, extraction.join("results/escape.json")).unwrap();

        let collection = tmp.path().join("Processed-WS01-X");
        std::fs::create_dir_all(&collection).unwrap();

        let dest = copy_velo_results(&extraction, &collection)
            .unwrap()
            .unwrap();
        assert!(dest.join("a.json").is_file());
        assert!(!dest.join("escape.json").exists());
    }
}
