//! SHA256 chain-of-custody records: every file this run generated inside a
//! collection directory.
//!
//! `write_output_hashes` is called last in `main.rs`'s per-collection
//! pipeline, after every other output-producing step (tool output, the
//! source hash log, the SysInfo report, Timeline Explorer sessions, and the
//! copied VeloResults tree), so its walk covers all of them.
//!
//! Lines are sorted by the collection-relative path, not by the order the
//! directory walk happened to reach them. That ordering is part of the
//! record, not a presentation detail: a chain-of-custody file two examiners
//! can diff byte-for-byte has to be deterministic, and a filesystem walk is
//! not (`read_dir` order varies by filesystem, and the walk here is an
//! explicit LIFO stack, so even one filesystem would not repeat itself
//! across an added directory). Byte-identical output for byte-identical
//! input is what makes a difference between two runs mean something.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use triage_core::error::TriageError;

/// Streaming SHA256, lowercase hex. Streaming is a memory-bound requirement,
/// not a micro-optimization: an MFT CSV routinely runs to several GB, and
/// `std::fs::read`-then-hash would put the whole file in memory at once.
///
/// Only a regular file is hashed, and the kind is settled *before* the open
/// rather than by letting the open fail: `open(2)` on a FIFO blocks until a
/// writer appears, so a named pipe anywhere this function is pointed would
/// hang the run rather than fail it. `metadata` is a `stat(2)` -- it reads
/// no bytes and blocks on none of these kinds -- and it resolves symlinks,
/// which is the property that matters here: `File::open` follows a link too,
/// so a symlink *to* a FIFO is the same hazard and is refused here as well.
/// `symlink_metadata` would call that link "not a regular file" and be right
/// by accident, while wrongly refusing a symlink to an ordinary file.
///
/// The guard lives here rather than at a call site because this is the only
/// place in the tree that opens a path to hash it; a future caller inherits
/// the protection instead of having to remember it.
pub fn sha256_file(path: &Path) -> Result<String, TriageError> {
    let meta = std::fs::metadata(path).map_err(|e| TriageError::Output {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    if !meta.is_file() {
        return Err(TriageError::Output {
            path: path.to_path_buf(),
            message: "not a regular file: refusing to hash it".into(),
        });
    }
    let mut file = std::fs::File::open(path).map_err(|e| TriageError::Output {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buf).map_err(|e| TriageError::Output {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Write `CaseInfo/<stamp>_OutputHashes.txt`: one line per generated file,
/// `<sha256> <size> <path relative to the collection directory>`.
///
/// The hash file cannot list its own hash, so the exclusion is done by
/// comparing paths, not by ordering (i.e. not by "walk before create" or
/// "create before walk"): the destination path is computed up front and
/// every candidate file is compared against it during the walk, so the
/// exclusion holds regardless of when `destination` is actually created on
/// disk.
pub fn write_output_hashes(collection_dir: &Path, stamp: &str) -> Result<PathBuf, TriageError> {
    let case_info = collection_dir.join("CaseInfo");
    std::fs::create_dir_all(&case_info).map_err(|e| TriageError::Output {
        path: case_info.clone(),
        message: e.to_string(),
    })?;
    let destination = case_info.join(format!("{stamp}_OutputHashes.txt"));

    let mut entries: Vec<(String, u64, String)> = Vec::new();
    let mut stack = vec![collection_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|e| TriageError::Output {
            path: dir.clone(),
            message: e.to_string(),
        })? {
            let entry = entry.map_err(|e| TriageError::Output {
                path: dir.clone(),
                message: e.to_string(),
            })?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path == destination {
                continue;
            }
            // This file is a chain-of-custody record: `0` is not a "size
            // unknown" placeholder, it is a positive claim that the file is
            // empty. Falling back to it on a metadata-read failure would
            // write a falsehood into the evidence log with no way for a
            // downstream reviewer to tell it apart from a genuine zero-byte
            // file. Propagate instead, matching every other failure path in
            // this function: a chain-of-custody file that cannot be
            // completed accurately should fail loudly, not ship with a hole
            // in it.
            let size = entry
                .metadata()
                .map(|m| m.len())
                .map_err(|e| TriageError::Output {
                    path: path.clone(),
                    message: e.to_string(),
                })?;
            let digest = sha256_file(&path)?;
            let relative = path
                .strip_prefix(collection_dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            entries.push((digest, size, relative));
        }
    }
    entries.sort_by(|a, b| a.2.cmp(&b.2));

    let mut out = std::fs::File::create(&destination).map_err(|e| TriageError::Output {
        path: destination.clone(),
        message: e.to_string(),
    })?;
    writeln!(out, "# TriageSuite output hashes — SHA256, sizes in bytes")
        .and_then(|_| writeln!(out, "# Run stamp: {stamp}"))
        .map_err(|e| TriageError::Output {
            path: destination.clone(),
            message: e.to_string(),
        })?;
    for (digest, size, relative) in entries {
        writeln!(out, "{digest} {size} {relative}").map_err(|e| TriageError::Output {
            path: destination.clone(),
            message: e.to_string(),
        })?;
    }
    Ok(destination)
}

/// Write `CaseInfo/<stamp>_SHA256_HashLog.txt` for the collection's source.
///
/// `None` means raw-directory input. The file is still written and states
/// that: an absent file is indistinguishable from a run that failed before
/// reaching this point.
pub fn write_source_hash_log(
    collection_dir: &Path,
    stamp: &str,
    source: Option<&Path>,
) -> Result<PathBuf, TriageError> {
    let case_info = collection_dir.join("CaseInfo");
    std::fs::create_dir_all(&case_info).map_err(|e| TriageError::Output {
        path: case_info.clone(),
        message: e.to_string(),
    })?;
    let destination = case_info.join(format!("{stamp}_SHA256_HashLog.txt"));
    let mut out = std::fs::File::create(&destination).map_err(|e| TriageError::Output {
        path: destination.clone(),
        message: e.to_string(),
    })?;

    writeln!(out, "# TriageSuite source hash log — SHA256")
        .and_then(|_| writeln!(out, "# Run stamp: {stamp}"))
        .map_err(|e| TriageError::Output {
            path: destination.clone(),
            message: e.to_string(),
        })?;

    match source {
        Some(path) => {
            let digest = sha256_file(path)?;
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            writeln!(out, "{digest} {size} {}", path.display()).map_err(|e| {
                TriageError::Output {
                    path: destination.clone(),
                    message: e.to_string(),
                }
            })?;
        }
        None => {
            writeln!(
                out,
                "No source archive: this collection was processed from a directory."
            )
            .map_err(|e| TriageError::Output {
                path: destination.clone(),
                message: e.to_string(),
            })?;
        }
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `f` on a thread and fail if it has not returned within `secs`.
    ///
    /// Every FIFO test below asserts that a call *returns* rather than
    /// blocking forever in `open(2)`. In-process that needs a bound: without
    /// one a regression does not fail the test, it wedges the whole suite,
    /// and `scripts/check.sh` never finishes. The blocked thread is abandoned
    /// on purpose -- a thread parked in `open` cannot be interrupted, and the
    /// harness's process exit reaps it. Ten seconds against a call that
    /// returns in microseconds is a margin, not a race.
    fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(std::time::Duration::from_secs(secs))
            .expect("call never returned: it is blocked, which is the defect this test guards")
    }

    #[cfg(unix)]
    fn mkfifo(at: &Path) {
        let made = std::process::Command::new("mkfifo")
            .arg(at)
            .status()
            .expect("mkfifo must be available on a unix host");
        assert!(made.success(), "mkfifo failed");
    }

    #[cfg(unix)]
    #[test]
    fn hashing_refuses_a_fifo_rather_than_blocking_on_its_open() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("pipe");
        mkfifo(&fifo);
        let err = within(10, move || sha256_file(&fifo)).unwrap_err();
        assert!(
            err.to_string().contains("not a regular file"),
            "expected a refusal naming the kind, got: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hashing_refuses_a_symlink_to_a_fifo_too() {
        // `File::open` follows the link, so the guard must resolve it as
        // well. This is the case `symlink_metadata` would get right by
        // accident and for the wrong reason.
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("pipe");
        mkfifo(&fifo);
        let link = tmp.path().join("link-to-pipe");
        std::os::unix::fs::symlink(&fifo, &link).unwrap();
        let err = within(10, move || sha256_file(&link)).unwrap_err();
        assert!(
            err.to_string().contains("not a regular file"),
            "expected a refusal naming the kind, got: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_an_ordinary_file_is_still_hashed() {
        // The companion to the test above: the guard must refuse the link's
        // *target kind*, not the fact that it is a link. Hashing through a
        // symlink is existing behaviour and stays.
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("abc");
        std::fs::write(&real, b"abc").unwrap();
        let link = tmp.path().join("link-to-abc");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            sha256_file(&link).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_in_the_output_tree_fails_the_record_rather_than_hanging_the_run() {
        // The reported defect: `write_output_hashes` walks `--out` and hashed
        // every non-directory entry, so a named pipe left there parked the
        // run in `open(2)` with no output and no error.
        //
        // It fails rather than skipping because of this module's own rule:
        // the hash file claims to list every file the run generated, so
        // omitting one silently would write a falsehood into the evidence
        // record. `main.rs` turns this into a named `output_errors` entry on
        // the collection and lets the remaining hosts finish.
        let tmp = tempfile::tempdir().unwrap();
        let collection = tmp.path().join("Collection-WS01");
        std::fs::create_dir_all(collection.join("FileSystem")).unwrap();
        std::fs::write(collection.join("FileSystem/real.csv"), b"a,b\n1,2\n").unwrap();
        mkfifo(&collection.join("FileSystem/pipe"));

        let err = within(10, move || {
            write_output_hashes(&collection, "2026-03-13T192553Z")
        })
        .unwrap_err();
        assert!(
            err.to_string().contains("not a regular file"),
            "expected a refusal naming the kind, got: {err}"
        );
    }

    #[test]
    fn sha256_matches_the_known_vector_for_an_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("empty");
        std::fs::write(&p, b"").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_matches_the_known_vector_for_abc() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("abc");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn output_hashes_covers_every_file_but_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let collection = tmp.path();
        std::fs::create_dir_all(collection.join("FileSystem")).unwrap();
        std::fs::write(collection.join("FileSystem/a.csv"), b"abc").unwrap();
        std::fs::create_dir_all(collection.join("CaseInfo")).unwrap();

        let written = write_output_hashes(collection, "2026-03-13T192553Z").unwrap();
        let body = std::fs::read_to_string(&written).unwrap();
        assert!(body.contains("FileSystem/a.csv"), "got {body}");
        assert!(
            body.contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            "got {body}"
        );
        assert!(body.contains(" 3 "), "size must be recorded: {body}");
        assert!(
            !body.contains("_OutputHashes.txt"),
            "the hash file must not list itself: {body}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn output_hashes_propagates_a_metadata_read_failure_instead_of_recording_zero() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let collection = tmp.path();
        let restricted = collection.join("FileSystem");
        std::fs::create_dir_all(&restricted).unwrap();
        std::fs::write(restricted.join("a.csv"), b"abc").unwrap();
        std::fs::create_dir_all(collection.join("CaseInfo")).unwrap();

        // Read but no execute/search permission: `read_dir` can still list
        // the directory's entries (that only needs read), but `stat`-ing a
        // path inside it -- which is what `DirEntry::metadata` does -- fails
        // with EACCES. This reproduces "listed but unstattable" without
        // racing a delete between listing and stat.
        std::fs::set_permissions(&restricted, std::fs::Permissions::from_mode(0o444)).unwrap();

        let result = write_output_hashes(collection, "2026-03-13T192553Z");

        // Restore permissions unconditionally so the tempdir can clean
        // itself up regardless of how the assertion below turns out.
        std::fs::set_permissions(&restricted, std::fs::Permissions::from_mode(0o755)).unwrap();

        match result {
            Err(TriageError::Output { .. }) => {}
            other => panic!(
                "expected a propagated TriageError::Output on a metadata failure \
                 (never a silent zero size), got {other:?}"
            ),
        }
    }

    #[test]
    fn source_hash_log_records_the_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Collection-WS01.zip");
        std::fs::write(&archive, b"abc").unwrap();
        let collection = tmp.path().join("Processed-WS01-2026-03-13T192553Z");
        std::fs::create_dir_all(&collection).unwrap();

        let written =
            write_source_hash_log(&collection, "2026-03-13T192553Z", Some(&archive)).unwrap();
        let body = std::fs::read_to_string(written).unwrap();
        assert!(body.contains("Collection-WS01.zip"), "got {body}");
        assert!(
            body.contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            "got {body}"
        );
    }

    /// Raw-directory input has no archive. The file must still exist and say so,
    /// rather than being silently absent — an absent file is indistinguishable
    /// from a failed run.
    #[test]
    fn source_hash_log_states_when_there_is_no_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let collection = tmp.path().join("Processed-WS01-X");
        std::fs::create_dir_all(&collection).unwrap();
        let written = write_source_hash_log(&collection, "X", None).unwrap();
        let body = std::fs::read_to_string(written).unwrap();
        assert!(
            body.to_lowercase().contains("no source archive"),
            "got {body}"
        );
    }
}
