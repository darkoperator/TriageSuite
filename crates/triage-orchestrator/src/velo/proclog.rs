//! `process_logs/<Tool>.log`, one per tool per collection.
//!
//! TriageSuite's parsers run in-process, so unlike VeloProcessor there is no
//! child-process stdout to capture. The log carries the equivalent record:
//! what was discovered, what validated, what failed and why, how many records
//! were written, and how long it took. An empty-looking body for an
//! in-process tool means it recorded nothing noteworthy at each of those
//! steps — it does not mean the tool did nothing; the summary block written
//! by `finish_with_counts` is always present and carries the actual counts.
//! External tools (Hayabusa, Takajo) get their real captured output instead,
//! written directly by the external driver rather than through `line`/
//! `finish_with_counts`.

use std::io::Write;
use std::path::Path;
use triage_core::error::TriageError;

pub struct ProcessLog {
    file: std::fs::File,
}

impl ProcessLog {
    /// Open `<collection_dir>/process_logs/<tool>.log`.
    ///
    /// `overwrite` is the run's `--overwrite` flag and is honoured here the
    /// same way every other output honours it: without it, an existing log
    /// is an error rather than something to truncate. This file used to
    /// truncate unconditionally, which made it the one generated artifact
    /// that ignored a documented contract -- and the artifact it silently
    /// destroyed was a previous run's record of what happened, which is
    /// exactly the kind of thing `--overwrite` exists to protect.
    ///
    /// Two runs only reach the same path when they share a collection
    /// directory, i.e. the same host and the same run stamp, so this is not
    /// a condition an ordinary second run hits. The caller treats a failure
    /// here as "this run has no process log" (`ProcessLog::line`'s doc
    /// comment): the log is a convenience artifact and the manifest remains
    /// authoritative, so refusing to open it never fails the run.
    pub fn open(collection_dir: &Path, tool: &str, overwrite: bool) -> Result<Self, TriageError> {
        let dir = collection_dir.join("process_logs");
        std::fs::create_dir_all(&dir).map_err(|e| TriageError::Output {
            path: dir.clone(),
            message: e.to_string(),
        })?;
        let path = dir.join(format!("{tool}.log"));
        let file = if overwrite {
            std::fs::File::create(&path)
        } else {
            std::fs::File::options()
                .write(true)
                .create_new(true)
                .open(&path)
        };
        let file = file.map_err(|e| {
            if !overwrite && e.kind() == std::io::ErrorKind::AlreadyExists {
                TriageError::Output {
                    path,
                    message: "output file exists; pass --overwrite to replace it".into(),
                }
            } else {
                TriageError::Output {
                    path,
                    message: e.to_string(),
                }
            }
        })?;
        Ok(Self { file })
    }

    /// Append a line. Logging failures are swallowed: a full disk must not
    /// turn a successful parse into a failed run, and the manifest carries
    /// the authoritative record either way — this file is a convenience
    /// artifact for an analyst, not the run's source of truth.
    pub fn line(&mut self, text: &str) {
        let _ = writeln!(self.file, "{text}");
    }

    /// Write the closing summary block. Same reasoning as `line`: every write
    /// here is best-effort, because a logging failure must never be allowed
    /// to turn a successful tool run into a failed one.
    pub fn finish_with_counts(
        mut self,
        matched: u64,
        parsed: u64,
        failed: u64,
        records: u64,
        elapsed: std::time::Duration,
    ) {
        let _ = writeln!(self.file, "---");
        let _ = writeln!(self.file, "files matched: {matched}");
        let _ = writeln!(self.file, "parsed: {parsed}");
        let _ = writeln!(self.file, "failed: {failed}");
        let _ = writeln!(self.file, "records: {records}");
        let _ = writeln!(self.file, "duration: {:.1}s", elapsed.as_secs_f64());
        let _ = self.file.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_log_records_the_run_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let mut log = ProcessLog::open(tmp.path(), "PETriage", false).unwrap();
        log.line("discovered 12 candidate files");
        log.line("unsupported: C:/x.pf — version 16 predates the supported range");
        log.finish_with_counts(12, 11, 1, 45, std::time::Duration::from_millis(1500));

        let body = std::fs::read_to_string(tmp.path().join("process_logs/PETriage.log")).unwrap();
        assert!(body.contains("discovered 12 candidate files"), "got {body}");
        assert!(body.contains("version 16 predates"), "got {body}");
        assert!(body.contains("parsed: 11"), "got {body}");
        assert!(body.contains("failed: 1"), "got {body}");
        assert!(body.contains("records: 45"), "got {body}");
        assert!(body.contains("1.5"), "duration must be recorded: {body}");
    }

    /// `--overwrite` applies to this file like any other output: without it
    /// a second open of the same path is refused, with it the log is
    /// replaced.
    #[test]
    fn a_second_open_needs_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let first = ProcessLog::open(tmp.path(), "PETriage", false).unwrap();
        first.finish_with_counts(1, 1, 0, 1, std::time::Duration::from_millis(1));

        assert!(
            ProcessLog::open(tmp.path(), "PETriage", false).is_err(),
            "a pre-existing process log must not be truncated without --overwrite"
        );
        let body = std::fs::read_to_string(tmp.path().join("process_logs/PETriage.log")).unwrap();
        assert!(
            body.contains("records: 1"),
            "the refused open must leave the existing log intact: {body}"
        );

        let replacement = ProcessLog::open(tmp.path(), "PETriage", true).unwrap();
        replacement.finish_with_counts(2, 2, 0, 9, std::time::Duration::from_millis(1));
        let body = std::fs::read_to_string(tmp.path().join("process_logs/PETriage.log")).unwrap();
        assert!(body.contains("records: 9"), "got {body}");
        assert!(!body.contains("records: 1\n"), "got {body}");
    }

    /// Every write is best-effort by design: a full disk, or any other write
    /// failure, must not turn a successful parse into a failed run. The log
    /// is built here directly over a read-only handle (a child module may
    /// name its parent's private field), because `open` can only produce
    /// writable handles and this property is about the writes, not the open.
    /// If `line` or `finish_with_counts` ever started propagating or
    /// unwrapping a write error, this test would fail rather than the defect
    /// reaching a live run.
    #[test]
    fn write_failures_are_swallowed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("readonly.log");
        std::fs::write(&path, b"").unwrap();
        let mut log = ProcessLog {
            file: std::fs::File::open(&path).unwrap(),
        };

        log.line("parse failed: C:/x.pf — truncated or corrupt structure");
        log.finish_with_counts(1, 0, 1, 0, std::time::Duration::from_millis(1));

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "",
            "a read-only handle must have rejected every write"
        );
    }
}
