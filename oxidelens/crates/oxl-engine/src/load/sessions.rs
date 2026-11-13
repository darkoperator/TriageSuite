//! Timeline Explorer sessions a run carries: `Sessions/<Name>.tle_sess` at
//! the run's output root, JSON whose `SessionFiles` object names, by
//! absolute path, the CSVs to open together for one line of enquiry
//! (`docs/tools/TriageSuite.md`, "Timeline Explorer sessions").
//!
//! A load keeps each session's list of paths in `ox.run_session`, so the
//! case still offers it when the run has moved or is gone. The paths name
//! where the run was written; a path is matched to the case by its end,
//! against the run's own inventory (`ox.inventory_file.path`, relative to
//! the output root), so where the run is now never matters.
//!
//! Session files are the run's output, read and never written, and they
//! are a convenience: a load never fails for one. A file over
//! `MAX_SESSION_BYTES`, past the first `MAX_SESSION_FILES`, or not a
//! session is left out and named, with why, in the load's history row.

use crate::case::{Case, CASE_FORMAT};
use crate::error::{EngineError, Result};
use duckdb::{params, Connection};
use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::path::Path;

/// The most session files one run's load keeps, taken in name order.
pub(crate) const MAX_SESSION_FILES: usize = 64;

/// The largest session file a load reads.
pub(crate) const MAX_SESSION_BYTES: u64 = 1024 * 1024;

/// What names a session file.
const SUFFIX: &str = ".tle_sess";

/// One session the case offers: a session file of a run in the case, and
/// the relations its files feed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Session {
    /// The run (`ox.source`) the session file came with.
    pub source_id: i64,
    /// The file's name without `.tle_sess`.
    pub name: String,
    /// Each relation and host one of the session's files feeds, sorted.
    pub relations: Vec<SessionRelation>,
    /// The session's files that feed nothing in the case: not a file of
    /// the run, or one whose dataset the load left out.
    pub unmatched: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SessionRelation {
    pub relation: String,
    pub host: String,
}

/// A session file as a load keeps it.
pub(crate) struct SessionFile {
    pub name: String,
    /// The paths its `SessionFiles` names, in the file's order.
    pub files: Vec<String>,
}

/// A session file a load left out, and why.
pub(crate) struct SkippedSession {
    pub file: String,
    pub reason: String,
}

/// What a run's `Sessions` folder held.
#[derive(Default)]
pub(crate) struct RunSessions {
    pub kept: Vec<SessionFile>,
    pub skipped: Vec<SkippedSession>,
}

impl RunSessions {
    /// The history row's account of them: the names kept, and each file
    /// left out with its reason.
    pub(crate) fn history_json(&self) -> (serde_json::Value, serde_json::Value) {
        (
            self.kept.iter().map(|s| s.name.as_str()).collect(),
            self.skipped
                .iter()
                .map(|s| serde_json::json!({ "file": s.file, "reason": s.reason }))
                .collect(),
        )
    }
}

/// The reason a link is left out with.
const LINK: &str = "link";

/// Whether `meta`, read without following links, is a link: a symbolic
/// link, or on Windows any reparse point, junctions included. A link is
/// never followed: it could lead a load out of the run folder.
fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// Read the session files at `root/Sessions`: none when there is no such
/// folder. Never fails: anything that cannot be read is skipped, with why.
/// No link is followed, neither `Sessions` itself nor a file in it; each is
/// skipped as a `link`.
pub(crate) fn read_sessions(root: &Path) -> RunSessions {
    let mut out = RunSessions::default();
    let dir = root.join("Sessions");
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if is_link(&meta) => {
            out.skipped.push(SkippedSession {
                file: "Sessions".to_string(),
                reason: LINK.to_string(),
            });
            return out;
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
        Err(e) => {
            out.skipped.push(SkippedSession {
                file: "Sessions".to_string(),
                reason: format!("the folder could not be read: {e}"),
            });
            return out;
        }
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
        Err(e) => {
            out.skipped.push(SkippedSession {
                file: "Sessions".to_string(),
                reason: format!("the folder could not be read: {e}"),
            });
            return out;
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                out.skipped.push(SkippedSession {
                    file: "Sessions".to_string(),
                    reason: format!("the folder could not be read to the end: {e}"),
                });
                break;
            }
        };
        let raw = entry.file_name();
        let lossy = raw.to_string_lossy().into_owned();
        if !is_session_name(&lossy) {
            continue;
        }
        let skip = |reason: String| SkippedSession {
            file: lossy.clone(),
            reason,
        };
        // Read without following a link.
        let meta = match std::fs::symlink_metadata(entry.path()) {
            Ok(meta) => meta,
            Err(e) => {
                out.skipped.push(skip(format!("could not be read: {e}")));
                continue;
            }
        };
        if is_link(&meta) {
            out.skipped.push(skip(LINK.to_string()));
            continue;
        }
        // A folder with the suffix is not a session file.
        if meta.is_dir() {
            continue;
        }
        if !meta.is_file() {
            out.skipped.push(skip("not a regular file".to_string()));
            continue;
        }
        match raw.into_string() {
            Ok(name) => names.push(name),
            Err(_) => out
                .skipped
                .push(skip("the file name is not valid Unicode".to_string())),
        }
    }
    names.sort();
    for (i, file) in names.into_iter().enumerate() {
        if i >= MAX_SESSION_FILES {
            out.skipped.push(SkippedSession {
                file,
                reason: format!(
                    "more than {MAX_SESSION_FILES} session files; the first \
                     {MAX_SESSION_FILES} by name are kept"
                ),
            });
            continue;
        }
        match read_session(&dir.join(&file), &file) {
            Ok(session) => out.kept.push(session),
            Err(reason) => out.skipped.push(SkippedSession { file, reason }),
        }
    }
    out
}

/// Whether `file` is named like a session file, with a name before the
/// suffix; the suffix is matched without regard to ASCII case.
fn is_session_name(file: &str) -> bool {
    file.len() > SUFFIX.len()
        && file.is_char_boundary(file.len() - SUFFIX.len())
        && file[file.len() - SUFFIX.len()..].eq_ignore_ascii_case(SUFFIX)
}

/// One session file, or why it is left out.
fn read_session(path: &Path, file: &str) -> std::result::Result<SessionFile, String> {
    let too_big = || format!("larger than 1 MiB ({MAX_SESSION_BYTES} bytes)");
    let handle = open_no_follow(path).map_err(|e| format!("could not be read: {e}"))?;
    let meta = handle
        .metadata()
        .map_err(|e| format!("could not be read: {e}"))?;
    // What was opened is checked again: the entry may have been replaced
    // since it was listed.
    if is_link(&meta) {
        return Err(LINK.to_string());
    }
    if !meta.is_file() {
        return Err("not a regular file".to_string());
    }
    let size = meta.len();
    if size > MAX_SESSION_BYTES {
        return Err(too_big());
    }
    // Read at most one byte past the limit, so a file that grew since its
    // size was taken is still caught, and never read whole.
    let mut bytes = Vec::new();
    handle
        .take(MAX_SESSION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("could not be read: {e}"))?;
    if bytes.len() as u64 > MAX_SESSION_BYTES {
        return Err(too_big());
    }
    let files = parse_session(&bytes)?;
    Ok(SessionFile {
        name: file[..file.len() - SUFFIX.len()].to_string(),
        files,
    })
}

/// Open `path` for reading without following a link at its last component:
/// on Windows the reparse point itself is opened (and then refused by its
/// attributes), elsewhere opening a symbolic link fails. Elsewhere it also
/// opens without blocking, so a FIFO put in a file's place cannot hang a
/// load.
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

/// The paths a session file's `SessionFiles` names, in order, or why the
/// text is not a session. A byte-order mark, which .NET tools write, is
/// allowed.
fn parse_session(bytes: &[u8]) -> std::result::Result<Vec<String>, String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let not_a_session = |why: String| format!("not a Timeline Explorer session: {why}");
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| not_a_session(e.to_string()))?;
    let files = value
        .get("SessionFiles")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| not_a_session("no SessionFiles object".to_string()))?;
    Ok(files.keys().cloned().collect())
}

/// Keep `sessions` as `source_id`'s, in the caller's transaction.
pub(crate) fn store_sessions(
    conn: &Connection,
    source_id: i64,
    sessions: &RunSessions,
) -> Result<()> {
    for session in &sessions.kept {
        let files = serde_json::to_string(&session.files).map_err(|e| {
            EngineError::Internal(format!("a session's file list does not serialize: {e}"))
        })?;
        conn.execute(
            "INSERT INTO ox.run_session (source_id, name, files_json) VALUES (?, ?, ?)",
            params![source_id, session.name, files],
        )?;
    }
    Ok(())
}

/// For an upgrade from format 1, in its transaction: the sessions of each
/// run still in the case, read again from where the run was loaded from.
/// Best effort: a run that moved, or whose session files cannot be read,
/// gets none or fewer, and that is no failure.
pub(crate) fn backfill_sessions(conn: &Connection) -> Result<()> {
    let runs: Vec<(i64, String)> = conn
        .prepare("SELECT id, path FROM ox.source WHERE kind = 'run' AND NOT removed ORDER BY id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    for (id, path) in runs {
        store_sessions(conn, id, &read_sessions(Path::new(&path)))?;
    }
    Ok(())
}

/// Every session of each run in the case that is not removed and still has
/// rows in a view (one superseded throughout, or one whose datasets are all
/// empty, offers none), sorted by source,
/// then name. Empty for a format-1 case, which keeps no sessions.
pub fn sessions(case: &Case) -> Result<Vec<Session>> {
    if case.format() < CASE_FORMAT {
        return Ok(Vec::new());
    }
    let conn = case.conn();
    let stored: Vec<(i64, String, String)> = conn
        .prepare(
            "SELECT rs.source_id, rs.name, rs.files_json \
             FROM ox.run_session rs JOIN ox.source s ON s.id = rs.source_id \
             WHERE s.kind = 'run' AND NOT s.removed \
               AND EXISTS (SELECT 1 FROM ox.source_dataset d \
                           WHERE d.source_id = s.id AND d.active AND d.rows > 0) \
             ORDER BY rs.source_id, rs.name",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut files_of: HashMap<i64, RunFiles> = HashMap::new();
    let mut out = Vec::with_capacity(stored.len());
    for (source_id, name, files_json) in stored {
        // The engine wrote the list from a parsed session.
        let files: Vec<String> = serde_json::from_str(&files_json).map_err(|e| {
            EngineError::Internal(format!(
                "session {name:?} of source {source_id} holds a file list that does not \
                 parse: {e}"
            ))
        })?;
        let run = match files_of.entry(source_id) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(RunFiles::read(conn, source_id)?)
            }
        };
        let mut relations = BTreeSet::new();
        let mut unmatched = 0u32;
        for file in &files {
            match run.feeds(file) {
                Some(fed) => {
                    relations.insert(fed.clone());
                }
                None => unmatched = unmatched.saturating_add(1),
            }
        }
        out.push(Session {
            source_id,
            name,
            relations: relations.into_iter().collect(),
            unmatched,
        });
    }
    Ok(out)
}

/// A run's files that feed a relation, by their normalised path relative
/// to the output root, grouped by last component for the lookup.
struct RunFiles {
    by_name: HashMap<String, Vec<(String, SessionRelation)>>,
}

impl RunFiles {
    /// Each file of `source_id`'s inventory whose dataset the load put in a
    /// relation for its host.
    fn read(conn: &Connection, source_id: i64) -> Result<RunFiles> {
        let rows: Vec<(String, String, String)> = conn
            .prepare(
                "SELECT f.path, f.view, f.host FROM ox.inventory_file f \
                 WHERE f.source_id = ? AND EXISTS ( \
                   SELECT 1 FROM ox.source_dataset d \
                   WHERE d.source_id = f.source_id AND d.relation = f.view \
                     AND d.host = f.host)",
            )?
            .query_map(params![source_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<std::result::Result<_, _>>()?;
        let mut by_name: HashMap<String, Vec<(String, SessionRelation)>> = HashMap::new();
        for (path, relation, host) in rows {
            let path = normalise(&path);
            let Some(last) = path.rsplit('/').next().filter(|l| !l.is_empty()) else {
                continue;
            };
            by_name
                .entry(last.to_string())
                .or_default()
                .push((path.clone(), SessionRelation { relation, host }));
        }
        Ok(RunFiles { by_name })
    }

    /// The relation and host `entry`, a path a session names, feeds: the
    /// run file whose path is the longest whole-component tail of it.
    fn feeds(&self, entry: &str) -> Option<&SessionRelation> {
        let entry = normalise(entry);
        let last = entry.rsplit('/').next()?;
        self.by_name
            .get(last)?
            .iter()
            .filter(|(path, _)| ends_with_components(&entry, path))
            .max_by_key(|(path, _)| path.len())
            .map(|(_, fed)| fed)
    }
}

/// `path` with `\` as `/`, runs of separators as one, no separator at
/// either end, and ASCII letters lower-cased.
fn normalise(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|c| !c.is_empty())
        .collect::<Vec<_>>()
        .join("/")
        .to_ascii_lowercase()
}

/// Whether normalised `path` ends with normalised `tail` as whole
/// components.
fn ends_with_components(path: &str, tail: &str) -> bool {
    !tail.is_empty()
        && path.ends_with(tail)
        && (path.len() == tail.len() || path.as_bytes()[path.len() - tail.len() - 1] == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_match_by_whole_trailing_components() {
        let tail = normalise("Processed-WS01-1/Execution/pf.csv");
        for (entry, matches) in [
            (r"C:\out\Processed-WS01-1\Execution\pf.csv", true),
            (r"c:\OUT\processed-ws01-1\EXECUTION\PF.CSV", true),
            ("Processed-WS01-1/Execution/pf.csv", true),
            (r"\\srv\share\\Processed-WS01-1\\Execution\pf.csv", true),
            (r"C:\out\xProcessed-WS01-1\Execution\pf.csv", false),
            (r"C:\out\Processed-WS01-1\Execution\xpf.csv", false),
            ("Execution/pf.csv", false),
            ("", false),
        ] {
            assert_eq!(
                ends_with_components(&normalise(entry), &tail),
                matches,
                "{entry}"
            );
        }
        assert!(!ends_with_components("a/b", ""));
    }

    #[test]
    fn only_tle_sess_files_with_a_name_are_sessions() {
        for (name, is) in [
            ("Execution_Analysis.tle_sess", true),
            ("X.TLE_SESS", true),
            (".tle_sess", false),
            ("x.tle_sess.bak", false),
            ("é.tle_sess", true),
            ("tle_sess", false),
            ("ééééé", false),
        ] {
            assert_eq!(is_session_name(name), is, "{name}");
        }
    }

    #[test]
    fn a_session_parses_with_or_without_a_byte_order_mark() {
        let text = br#"{"SessionFiles": {"C:\\a.csv": [], "C:\\b.csv": []}}"#;
        assert_eq!(parse_session(text).unwrap(), [r"C:\a.csv", r"C:\b.csv"]);
        let mut bommed = b"\xEF\xBB\xBF".to_vec();
        bommed.extend_from_slice(text);
        assert_eq!(parse_session(&bommed).unwrap().len(), 2);
        for bad in [&b""[..], b"[]", b"{}", br#"{"SessionFiles": []}"#, b"\xff"] {
            assert!(parse_session(bad).is_err(), "{bad:?}");
        }
    }
}
