//! Timeline Explorer session files. `.tle_sess` is plain JSON:
//! `{"SessionFiles": {"<absolute path>": [], ...}}`.
//!
//! The bundled config's patterns are matched with `globset` against every
//! file's path relative to the collection directory, rather than expanded
//! into candidate paths: `globset` builds a matcher and tests candidates
//! against it, it does not walk a filesystem for you. So this module does
//! its own single recursive walk of the collection directory and tests each
//! relative path against each session's compiled pattern set.

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use std::path::{Path, PathBuf};
use triage_core::error::TriageError;

const CONFIG: &str = include_str!("../../../../resources/velo/TimelineExplorerSessions.json");

#[derive(serde::Deserialize)]
struct SessionConfig {
    #[serde(rename = "Sessions")]
    sessions: Vec<SessionDef>,
}

#[derive(serde::Deserialize)]
struct SessionDef {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "FilePatterns")]
    file_patterns: Vec<String>,
}

/// Compile one session's patterns into a matcher over collection-relative
/// paths. The patterns are static strings shipped in the bundled config, so
/// one that fails to compile is a programming error in that config, not a
/// runtime condition to recover from.
fn glob_set(patterns: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(compile_pattern(pattern).expect("session pattern must be a valid glob"));
    }
    builder.build().expect("session glob set must compile")
}

/// Compile one session pattern, with `*` confined to a single path
/// component.
///
/// `globset`'s default is the opposite -- a bare `Glob::new` lets `*` cross
/// `/` -- and that default made every flat category pattern also select the
/// per-user slices below it. `Registry/*_RETriage_results*.csv` matched both
/// `Registry/<stamp>_RETriage_results_Batch.csv` and
/// `Registry/PerUser/Batch/<stamp>_RETriage_results_Batch_alice.csv`, so a
/// session loaded the same rows twice: the merged file at the category root
/// is built from exactly those slices and carries every one of their rows
/// plus a `TriageUser` column (`velo::merge`), which makes the slices
/// redundant by construction rather than merely overlapping. Every sort,
/// filter and count an analyst ran in Timeline Explorer double-counted. It
/// also dragged in the reclaimed system-scope slice, whose filename is a
/// 162-character internal bookkeeping label (`merge::RECLAIM_LABEL`) that no
/// analyst should ever be shown.
///
/// A pattern that genuinely wants to reach into a subdirectory still can --
/// by naming it, as `EventLogs/Individual/Security.csv` does, or with `**`.
/// `literal_separator(true)` only stops `*` from doing it by accident.
///
/// `tests/velo_sessions_patterns.rs` compiles through this same function, so
/// the guard cannot assert one semantics while production uses another, and
/// it asserts both halves: every pattern still reaches a real filename, and
/// none reaches a per-user slice.
pub fn compile_pattern(pattern: &str) -> Result<Glob, globset::Error> {
    GlobBuilder::new(pattern).literal_separator(true).build()
}

/// Write one `.tle_sess` per session that matched at least one existing file
/// under `collection_dir`. Returns how many were written. Sessions matching
/// zero files are skipped entirely rather than written empty: an empty
/// session file is worse than none, since Timeline Explorer would open
/// nothing and an analyst would think the data was missing.
pub fn write_sessions(collection_dir: &Path) -> Result<usize, TriageError> {
    let config: SessionConfig = serde_json::from_str(CONFIG).map_err(|e| TriageError::Output {
        path: PathBuf::from("TimelineExplorerSessions.json"),
        message: e.to_string(),
    })?;

    // One walk of the collection directory feeds every session's matcher,
    // rather than a per-session walk.
    let mut all_files: Vec<PathBuf> = Vec::new();
    triage_core::discovery::walk_files(collection_dir, &[], &mut |_| {}, &mut |path| {
        all_files.push(path.to_path_buf());
    });

    let mut written = 0usize;
    for session in &config.sessions {
        let set = glob_set(&session.file_patterns);
        let mut matched: Vec<&PathBuf> = all_files
            .iter()
            .filter(|path| {
                let Ok(relative) = path.strip_prefix(collection_dir) else {
                    return false;
                };
                // Timeline Explorer patterns are written with forward
                // slashes; normalize so matching is platform-independent.
                let relative = relative.to_string_lossy().replace('\\', "/");
                set.is_match(relative)
            })
            .collect();
        matched.sort();
        matched.dedup();
        if matched.is_empty() {
            continue;
        }

        let dir = collection_dir.join("Sessions");
        std::fs::create_dir_all(&dir).map_err(|e| TriageError::Output {
            path: dir.clone(),
            message: e.to_string(),
        })?;
        let destination = dir.join(format!("{}.tle_sess", session.name));

        // Timeline Explorer reads absolute paths.
        let mut files = serde_json::Map::new();
        for path in matched {
            let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            files.insert(
                absolute.to_string_lossy().to_string(),
                serde_json::Value::Array(Vec::new()),
            );
        }
        let mut root = serde_json::Map::new();
        root.insert("SessionFiles".to_string(), serde_json::Value::Object(files));

        let body = serde_json::to_string_pretty(&serde_json::Value::Object(root)).map_err(|e| {
            TriageError::Output {
                path: destination.clone(),
                message: e.to_string(),
            }
        })?;
        std::fs::write(&destination, body).map_err(|e| TriageError::Output {
            path: destination,
            message: e.to_string(),
        })?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_reference_only_files_that_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let collection = tmp.path();
        std::fs::create_dir_all(collection.join("FileSystem")).unwrap();
        std::fs::write(collection.join("FileSystem/x_PETriage_results.csv"), "a\n").unwrap();

        let count = write_sessions(collection).unwrap();
        assert!(count >= 1);

        let session =
            std::fs::read_to_string(collection.join("Sessions/Execution_Analysis.tle_sess"))
                .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&session).unwrap();
        let files = parsed["SessionFiles"].as_object().unwrap();
        assert_eq!(files.len(), 1);
        for path in files.keys() {
            assert!(
                std::path::Path::new(path).is_file(),
                "{path} does not exist"
            );
            assert!(
                std::path::Path::new(path).is_absolute(),
                "{path} is not absolute"
            );
        }
    }

    #[test]
    fn a_session_matching_nothing_is_not_written() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(write_sessions(tmp.path()).unwrap(), 0);
        assert!(!tmp
            .path()
            .join("Sessions")
            .join("Execution_Analysis.tle_sess")
            .exists());
    }

    #[test]
    fn a_session_matches_files_across_multiple_patterns() {
        let tmp = tempfile::tempdir().unwrap();
        let collection = tmp.path();
        std::fs::create_dir_all(collection.join("EventLogs/Individual")).unwrap();
        std::fs::write(collection.join("EventLogs/Individual/Security.csv"), "a\n").unwrap();
        // Takajo's `automagic -o` is handed the ThreatHunting directory and
        // writes its report files straight into it, which is where the
        // session patterns look for them. Observed, not assumed: see
        // `tests/data/external_tools_observed_output.txt`. (It also creates
        // a `scriptblock-logs/` subdirectory, which no pattern names, so
        // this fixture does not model it.)
        std::fs::create_dir_all(collection.join("ThreatHunting")).unwrap();
        std::fs::write(collection.join("ThreatHunting/TimelineLogon.csv"), "a\n").unwrap();

        let count = write_sessions(collection).unwrap();
        assert!(count >= 1);

        let session =
            std::fs::read_to_string(collection.join("Sessions/Lateral_Movement_Logons.tle_sess"))
                .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&session).unwrap();
        let files = parsed["SessionFiles"].as_object().unwrap();
        assert_eq!(files.len(), 2, "expected both matches: {files:?}");
    }

    /// A session selects the merged file at the category root and none of
    /// the per-user slices it was built from.
    ///
    /// End-to-end over a real directory, so it pins the behaviour
    /// `write_sessions` actually has rather than the semantics
    /// `compile_pattern` is configured with. The three files below are the
    /// exact shapes a real run puts in `Registry/` for RETriage's `Batch`
    /// dataset -- a merged file, one profile's slice, and the reclaimed
    /// system-scope slice -- and the merged file already contains every row
    /// of the other two plus a `TriageUser` column (`velo::merge`).
    #[test]
    fn a_session_selects_the_merged_file_and_not_its_per_user_slices() {
        let tmp = tempfile::tempdir().unwrap();
        let collection = tmp.path();
        let per_user = collection.join("Registry/PerUser/Batch");
        std::fs::create_dir_all(&per_user).unwrap();
        let stem = "2026-01-01T000000Z_RETriage_results_Batch";
        let merged = collection.join(format!("Registry/{stem}.csv"));
        std::fs::write(&merged, "a\n").unwrap();
        std::fs::write(per_user.join(format!("{stem}_alice.csv")), "a\n").unwrap();
        std::fs::write(
            per_user.join(crate::velo::merge::reclaimed_slice_filename(stem, "csv")),
            "a\n",
        )
        .unwrap();

        write_sessions(collection).unwrap();

        let session =
            std::fs::read_to_string(collection.join("Sessions/Persistence.tle_sess")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&session).unwrap();
        let files = parsed["SessionFiles"].as_object().unwrap();
        let names: Vec<&String> = files.keys().collect();
        assert_eq!(
            names.len(),
            1,
            "the merged file alone belongs in the session: {names:?}"
        );
        assert!(
            names[0].ends_with(&format!("{stem}.csv")),
            "expected the merged file, got {names:?}"
        );
        assert!(
            !names[0].contains("PerUser"),
            "a per-user slice reached the session: {names:?}"
        );
    }

    // A test that builds its "realistic" filenames by substituting `*` back
    // into the pattern under test was tried here and removed: it can only
    // ever pass, since every concrete file it builds is a template instance
    // of the very pattern it is meant to check, regardless of whether that
    // pattern matches anything a real tool produces. The real guard lives in
    // `tests/velo_sessions_patterns.rs`, which builds its corpus from each
    // tool's actual `binary_name()`/`datasets()` through the real
    // `velo_basename()` instead -- independent of what this config's
    // patterns say -- and does fail against the patterns this config has got
    // wrong twice now: the four `_results.csv` ones, and the four external-
    // tool ones (confirmed both times by reverting the config and re-running).
}
