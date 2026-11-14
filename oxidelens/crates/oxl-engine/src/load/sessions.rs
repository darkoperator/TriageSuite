//! Timeline Explorer sessions a run carries: `Sessions/<Name>.tle_sess` in
//! each host's `Processed-<HOST>-<stamp>/` folder, where the suite writes
//! them, or at the run's output root; JSON whose `SessionFiles` object
//! names, by absolute path, the CSVs to open together for one line of
//! enquiry (`docs/tools/TriageSuite.md`, "Timeline Explorer sessions").
//! Each host of a run has its own copy of a session (`Execution_Analysis`);
//! a load keeps one session per name, its files those of every copy.
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

use crate::case::{Case, FORMAT_2};
use crate::error::{EngineError, Result};
use duckdb::{params, Connection};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The most session files one run's load takes, across all its `Sessions`
/// folders: the root's first, then each host folder's, each by file name.
pub(crate) const MAX_SESSION_FILES: usize = 64;

/// The largest session file a load reads.
pub(crate) const MAX_SESSION_BYTES: u64 = 1024 * 1024;

/// The most entries of one folder a load keeps while looking for sessions:
/// host folders of the run root, session files of a `Sessions` folder, each
/// one that qualifies by name and kind (`host_verdict`, `session_verdict`);
/// nothing else takes a place or is counted. A suite run has one host folder per host and five
/// session files in each. Past this many, the first this many by name are
/// kept (the whole folder is read to find them, see `list`) and the load's
/// history says how many were not.
pub(crate) const MAX_FOLDER_ENTRIES: usize = 4096;

/// The most `Processed-*` folders a load looks in, the first by name. A run
/// has one per host; the history says how many were not looked in.
pub(crate) const MAX_HOST_FOLDERS: usize = 256;

/// The most left-out files and folders the load's history names; the rest
/// are counted in one last entry.
pub(crate) const MAX_SKIPPED_NAMED: usize = 32;

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

/// What a run's `Sessions` folders held.
#[derive(Default)]
pub(crate) struct RunSessions {
    pub kept: Vec<SessionFile>,
    /// The first `MAX_SKIPPED_NAMED` files and folders left out.
    skipped: Vec<SkippedSession>,
    /// How many more were left out.
    skipped_more: usize,
}

impl RunSessions {
    /// Record `file` (a path below the run root) as left out for `reason`:
    /// named while fewer than `MAX_SKIPPED_NAMED` are, counted after.
    fn skip(&mut self, file: impl Into<String>, reason: impl Into<String>) {
        if self.skipped.len() < MAX_SKIPPED_NAMED {
            self.skipped.push(SkippedSession {
                file: file.into(),
                reason: reason.into(),
            });
        } else {
            self.skipped_more += 1;
        }
    }

    /// The history row's account of them: the names kept, and each file
    /// or folder left out with its reason, the first `MAX_SKIPPED_NAMED` of
    /// them, then `{"file": "…", "reason": "and K more left out"}`.
    pub(crate) fn history_json(&self) -> (serde_json::Value, serde_json::Value) {
        let mut skipped: Vec<serde_json::Value> = self
            .skipped
            .iter()
            .map(|s| serde_json::json!({ "file": s.file, "reason": s.reason }))
            .collect();
        if self.skipped_more > 0 {
            skipped.push(serde_json::json!({
                "file": "…",
                "reason": format!("and {} more left out", self.skipped_more),
            }));
        }
        (
            self.kept.iter().map(|s| s.name.as_str()).collect(),
            skipped.into(),
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

/// What a host's output folder is called: `Processed-<HOST>-<stamp>`.
const HOST_FOLDER_PREFIX: &str = "Processed-";

/// Read the session files of the run at `root`: `Sessions/*.tle_sess` at the
/// root and in each host's folder, `Processed-*/Sessions/` (where the suite
/// writes them), taken in that order: the root's first, then the host
/// folders' by folder name, each folder's by file name. `MAX_SESSION_FILES`
/// counts files across every folder, and once it is reached no further
/// folder is read.
///
/// Same-named sessions (one host's `Execution_Analysis` and another's) are
/// one session: their file lists joined, a path named twice kept once.
///
/// Never fails: anything that cannot be read is skipped, with why, named by
/// its path below the run root (`Sessions/X.tle_sess`,
/// `Processed-WS01-…/Sessions/X.tle_sess`). No link is followed, neither a
/// `Processed-*` folder, nor a `Sessions` folder, nor a file in one; each is
/// skipped as a `link`.
///
/// Bounded whatever the run holds: at most `MAX_FOLDER_ENTRIES` entries of
/// any folder are kept (the first by name), at most `MAX_HOST_FOLDERS` host folders are looked
/// in, at most `MAX_SESSION_FILES` files are opened, each read at most
/// `MAX_SESSION_BYTES` far, and at most `MAX_SKIPPED_NAMED` left-out entries
/// are named.
pub(crate) fn read_sessions(root: &Path) -> RunSessions {
    let mut out = RunSessions::default();
    let mut folders = vec!["Sessions".to_string()];
    folders.extend(host_folders(root, &mut out));
    let mut merged: BTreeMap<String, (Vec<String>, HashSet<String>)> = BTreeMap::new();
    let mut taken = 0usize;
    for folder in &folders {
        if taken >= MAX_SESSION_FILES {
            // Not listed: only whether it is there, so the history can say
            // it was not read.
            if std::fs::symlink_metadata(below(root, folder)).is_ok() {
                out.skip(
                    folder.as_str(),
                    format!("not read: {MAX_SESSION_FILES} session files were already taken"),
                );
            }
            continue;
        }
        for file in session_files(root, folder, &mut out) {
            let label = format!("{folder}/{file}");
            if taken >= MAX_SESSION_FILES {
                out.skip(
                    label,
                    format!(
                        "more than {MAX_SESSION_FILES} session files; the first \
                         {MAX_SESSION_FILES} are taken (the root's, then each host \
                         folder's, by name)"
                    ),
                );
                continue;
            }
            taken += 1;
            match read_session(&below(root, folder).join(&file), &file) {
                Ok(session) => {
                    let (paths, seen) = merged.entry(session.name).or_default();
                    for f in session.files {
                        if seen.insert(f.clone()) {
                            paths.push(f);
                        }
                    }
                }
                Err(reason) => out.skip(label, reason),
            }
        }
    }
    out.kept = merged
        .into_iter()
        .map(|(name, (files, _))| SessionFile { name, files })
        .collect();
    out
}

/// What a folder entry is, as its listing says, without following it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dir,
    File,
    /// A symbolic link, or on Windows any reparse point (junctions included).
    Link,
    /// Anything else (a device, a FIFO), or a kind that could not be read.
    Other,
}

/// The kind of `entry`, from the listing itself: no extra system call on
/// Windows (`DirEntry::metadata` comes with the listing there, and carries
/// the reparse-point attribute a junction has) or on Linux and macOS
/// (`DirEntry::file_type`, which does not follow a link).
fn kind_of(entry: &std::fs::DirEntry) -> Kind {
    #[cfg(windows)]
    {
        match entry.metadata() {
            Ok(meta) if is_link(&meta) => Kind::Link,
            Ok(meta) if meta.is_dir() => Kind::Dir,
            Ok(meta) if meta.is_file() => Kind::File,
            _ => Kind::Other,
        }
    }
    #[cfg(not(windows))]
    {
        match entry.file_type() {
            Ok(t) if t.is_symlink() => Kind::Link,
            Ok(t) if t.is_dir() => Kind::Dir,
            Ok(t) if t.is_file() => Kind::File,
            _ => Kind::Other,
        }
    }
}

/// One folder entry, as the selection sees it.
struct Candidate {
    name: OsString,
    kind: Kind,
    path: PathBuf,
}

/// What a caller makes of a candidate.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// What the caller looks for: its name, valid Unicode.
    Take(String),
    /// Looks like one but is not to be taken; named in the history (its name
    /// shown lossily) with why.
    Report(String, &'static str),
    /// Nothing the caller looks for.
    Pass,
}

/// A `Processed-*` host folder: a folder, not a link, its whole name valid
/// Unicode and starting `Processed-`. One that is a link is reported.
fn host_verdict(c: &Candidate) -> Verdict {
    let Some(name) = c.name.to_str() else {
        return Verdict::Pass;
    };
    if !name.starts_with(HOST_FOLDER_PREFIX) {
        return Verdict::Pass;
    }
    match c.kind {
        Kind::Dir => Verdict::Take(name.to_string()),
        Kind::Link => Verdict::Report(name.to_string(), LINK),
        Kind::File | Kind::Other => Verdict::Pass,
    }
}

/// A session file: a regular file, not a link, its whole name valid Unicode
/// and ending `.tle_sess`. A link, something that is neither file nor folder,
/// or a name that only looks like one when read lossily, is reported; a
/// folder so named is passed over.
fn session_verdict(c: &Candidate) -> Verdict {
    let Some(name) = c.name.to_str() else {
        return if is_session_name(&c.name.to_string_lossy()) {
            Verdict::Report(
                c.name.to_string_lossy().into_owned(),
                "the file name is not valid Unicode",
            )
        } else {
            Verdict::Pass
        };
    };
    if !is_session_name(name) {
        return Verdict::Pass;
    }
    match c.kind {
        Kind::File => Verdict::Take(name.to_string()),
        Kind::Link => Verdict::Report(name.to_string(), LINK),
        Kind::Other => Verdict::Report(name.to_string(), "not a regular file"),
        Kind::Dir => Verdict::Pass,
    }
}

/// The entries of a folder a load takes: the first `MAX_FOLDER_ENTRIES` by
/// name of those its verdict takes, as (name, path), and how many it took;
/// the first `MAX_SKIPPED_NAMED` by name of those it reports, with why, and
/// how many it reported. An entry that cannot be read ends the listing and
/// is reported as `ended`.
#[derive(Default)]
struct Listed {
    taken: Vec<(String, PathBuf)>,
    taken_count: usize,
    reported: Vec<(String, &'static str)>,
    reported_count: usize,
    ended: Option<std::io::Error>,
}

/// Sort `candidates` by `verdict` into a `Listed`. Every check that rules a
/// candidate out (its name, its kind) is in the verdict, so nothing that
/// will not be taken can hold one of the `MAX_FOLDER_ENTRIES` places.
fn select(
    candidates: impl IntoIterator<Item = std::io::Result<Candidate>>,
    verdict: impl Fn(&Candidate) -> Verdict,
) -> Listed {
    let mut taken = Smallest::new(MAX_FOLDER_ENTRIES);
    let mut reported = Smallest::new(MAX_SKIPPED_NAMED);
    let mut ended = None;
    for candidate in candidates {
        let candidate = match candidate {
            Ok(candidate) => candidate,
            Err(e) => {
                ended = Some(e);
                break;
            }
        };
        match verdict(&candidate) {
            Verdict::Take(name) => taken.push((name, candidate.path)),
            Verdict::Report(name, why) => reported.push((name, why)),
            Verdict::Pass => {}
        }
    }
    let (taken, taken_count) = taken.into_sorted();
    let (reported, reported_count) = reported.into_sorted();
    Listed {
        taken,
        taken_count,
        reported,
        reported_count,
        ended,
    }
}

/// List `dir` and `select` its entries by `verdict`; an error when it cannot
/// be listed.
///
/// The whole listing is read, however long, so that which entries are kept
/// depends only on their names, never on the order the system lists them
/// (`Smallest`): memory stays O(`MAX_FOLDER_ENTRIES`) and time is linear in
/// the folder's size, which is fine for a run folder on a local disk. There
/// is deliberately no ceiling on the entries scanned: stopping part-way
/// would make the choice depend on listing order again.
fn list(dir: &Path, verdict: impl Fn(&Candidate) -> Verdict) -> std::io::Result<Listed> {
    let entries = std::fs::read_dir(dir)?.map(|entry| {
        entry.map(|entry| Candidate {
            name: entry.file_name(),
            kind: kind_of(&entry),
            path: entry.path(),
        })
    });
    Ok(select(entries, verdict))
}

impl RunSessions {
    /// Record what `listed` reported, each named `{prefix}{name}`, counting
    /// those past the names it kept.
    fn report(&mut self, prefix: &str, listed: &Listed) {
        for (name, why) in &listed.reported {
            self.skip(format!("{prefix}{name}"), *why);
        }
        self.skipped_more += listed.reported_count - listed.reported.len();
    }
}

/// The `k` smallest of the items pushed, and how many were pushed: a
/// max-heap that drops its largest whenever it holds more than `k`. Memory is
/// O(`k`) however many are pushed, and which items are kept depends only on
/// the items, never on the order they come in.
struct Smallest<T: Ord> {
    k: usize,
    heap: BinaryHeap<T>,
    seen: usize,
}

impl<T: Ord> Smallest<T> {
    fn new(k: usize) -> Smallest<T> {
        Smallest {
            k,
            heap: BinaryHeap::with_capacity(k.saturating_add(1).min(MAX_FOLDER_ENTRIES + 1)),
            seen: 0,
        }
    }

    fn push(&mut self, item: T) {
        self.seen += 1;
        self.heap.push(item);
        if self.heap.len() > self.k {
            self.heap.pop();
        }
    }

    /// The items kept, smallest first, and how many were pushed.
    fn into_sorted(self) -> (Vec<T>, usize) {
        (self.heap.into_sorted_vec(), self.seen)
    }
}

/// What the history says of a folder with more than `MAX_FOLDER_ENTRIES`
/// entries of a kind: `what` names the kind.
fn cut_short(wanted: usize, what: &str) -> String {
    format!(
        "{wanted} {what}; the first {MAX_FOLDER_ENTRIES} by name were looked at, \
         {} were not",
        wanted - MAX_FOLDER_ENTRIES
    )
}

/// The `Sessions` folders of the hosts' folders of the run at `root`, as
/// paths below it (`Processed-WS01-…/Sessions`), the first
/// `MAX_HOST_FOLDERS` by folder name. A `Processed-*` entry that is a link is
/// skipped as a `link`; one that is not a folder, or whose name is not
/// Unicode, is passed over (`host_verdict`).
fn host_folders(root: &Path, out: &mut RunSessions) -> Vec<String> {
    let listed = match list(root, host_verdict) {
        Ok(listed) => listed,
        Err(e) => {
            out.skip(".", format!("the run folder could not be read: {e}"));
            return Vec::new();
        }
    };
    if let Some(e) = &listed.ended {
        out.skip(
            ".",
            format!("the run folder could not be read to the end: {e}"),
        );
    }
    out.report("", &listed);
    if listed.taken_count > MAX_FOLDER_ENTRIES {
        out.skip(".", cut_short(listed.taken_count, "host folders"));
    }
    // Already sorted by name (`list`), so which host folders are looked in
    // does not depend on the order the system lists them.
    let mut hosts = Vec::new();
    for (name, path) in listed.taken {
        // Checked again as it is opened, without following a link: the
        // listing said a folder, but it may have been replaced since.
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if is_link(&meta) => out.skip(name, LINK),
            Ok(meta) if meta.is_dir() => hosts.push(name),
            Ok(_) => {}
            Err(e) => out.skip(name, format!("the folder could not be read: {e}")),
        }
    }
    if hosts.len() > MAX_HOST_FOLDERS {
        out.skip(
            "Processed-*",
            format!(
                "{} host folders; the first {MAX_HOST_FOLDERS} by name were looked in, \
                 {} were not",
                hosts.len(),
                hosts.len() - MAX_HOST_FOLDERS
            ),
        );
        hosts.truncate(MAX_HOST_FOLDERS);
    }
    hosts.into_iter().map(|h| format!("{h}/Sessions")).collect()
}

/// The session file names in the folder `folder` below `root`, sorted: none
/// when there is no such folder. A folder or file that is a link, or that
/// cannot be read, is skipped with why (`session_verdict`). At most
/// `MAX_FOLDER_ENTRIES` session files are kept, the first by name; past that
/// the history says how many were not.
fn session_files(root: &Path, folder: &str, out: &mut RunSessions) -> Vec<String> {
    let dir = below(root, folder);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if is_link(&meta) => {
            out.skip(folder, LINK);
            return Vec::new();
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            out.skip(folder, format!("the folder could not be read: {e}"));
            return Vec::new();
        }
    }
    let listed = match list(&dir, session_verdict) {
        Ok(listed) => listed,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            out.skip(folder, format!("the folder could not be read: {e}"));
            return Vec::new();
        }
    };
    if let Some(e) = &listed.ended {
        out.skip(
            folder,
            format!("the folder could not be read to the end: {e}"),
        );
    }
    out.report(&format!("{folder}/"), &listed);
    if listed.taken_count > MAX_FOLDER_ENTRIES {
        out.skip(folder, cut_short(listed.taken_count, "session files"));
    }
    // Already sorted by name (`list`), so what is skipped, and in what
    // order, does not depend on the order the system listed them.
    let mut names = Vec::new();
    for (name, path) in listed.taken {
        let label = format!("{folder}/{name}");
        // Checked again as it is opened, without following a link: the
        // listing said a regular file, but it may have been replaced since.
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) => {
                out.skip(label, format!("could not be read: {e}"));
                continue;
            }
        };
        if is_link(&meta) {
            out.skip(label, LINK);
            continue;
        }
        // A folder with the suffix is not a session file.
        if meta.is_dir() {
            continue;
        }
        if !meta.is_file() {
            out.skip(label, "not a regular file");
            continue;
        }
        names.push(name);
    }
    names
}

/// `folder` (a `/`-separated path below `root`) as a path.
fn below(root: &Path, folder: &str) -> PathBuf {
    folder.split('/').fold(root.to_path_buf(), |p, c| p.join(c))
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

/// For an upgrade to format 3, in its transaction: the sessions of each
/// run still in the case that has none, read again from where the run was
/// loaded from. A run with sessions keeps exactly the rows it has. Best
/// effort: a run that moved, or whose session files cannot be read, gets
/// none or fewer, and that is no failure.
pub(crate) fn backfill_sessions(conn: &Connection) -> Result<()> {
    let runs: Vec<(i64, String)> = conn
        .prepare(
            "SELECT s.id, s.path FROM ox.source s \
             WHERE s.kind = 'run' AND NOT s.removed \
               AND NOT EXISTS (SELECT 1 FROM ox.run_session rs WHERE rs.source_id = s.id) \
             ORDER BY s.id",
        )?
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
    if case.format() < FORMAT_2 {
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

    /// Whatever order the items come in, the same smallest `k` are kept: a
    /// name that sorts first but is listed last is never left out.
    #[test]
    fn the_smallest_k_are_kept_whatever_the_order() {
        let names: Vec<String> = (0..10_000).map(|i| format!("S{i:05}.tle_sess")).collect();
        let forward = {
            let mut s = Smallest::new(4096);
            names.iter().cloned().for_each(|n| s.push(n));
            s.into_sorted()
        };
        let reverse = {
            let mut s = Smallest::new(4096);
            names.iter().rev().cloned().for_each(|n| s.push(n));
            s.into_sorted()
        };
        assert_eq!(forward, reverse);
        let (kept, seen) = reverse;
        assert_eq!(seen, 10_000);
        assert_eq!(kept.len(), 4096);
        assert_eq!(kept[0], "S00000.tle_sess");
        assert_eq!(kept[4095], "S04095.tle_sess");
        // Fewer than k: all of them, sorted; none: none.
        let mut s = Smallest::new(4);
        for n in ["c", "a", "b"] {
            s.push(n);
        }
        assert_eq!(s.into_sorted(), (vec!["a", "b", "c"], 3));
        assert_eq!(Smallest::<u8>::new(4).into_sorted(), (vec![], 0));
    }

    /// `before` + a character no UTF-8 holds + `after`: a lone surrogate on
    /// Windows, a stray continuation byte elsewhere.
    fn not_unicode(before: &str, after: &str) -> OsString {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let mut wide: Vec<u16> = before.encode_utf16().collect();
            wide.push(0xD800);
            wide.extend(after.encode_utf16());
            OsString::from_wide(&wide)
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::ffi::OsStringExt;
            let mut bytes = before.as_bytes().to_vec();
            bytes.push(0x80);
            bytes.extend_from_slice(after.as_bytes());
            OsString::from_vec(bytes)
        }
    }

    fn candidate(name: OsString, kind: Kind) -> std::io::Result<Candidate> {
        Ok(Candidate {
            path: PathBuf::from(&name),
            name,
            kind,
        })
    }

    /// More than `MAX_FOLDER_ENTRIES` entries that sort first but cannot be
    /// host folders (names not Unicode, links, files) never crowd out the one
    /// that can, and are not counted as host folders.
    #[test]
    fn only_real_host_folders_take_a_place() {
        let mut all = Vec::new();
        for i in 0..MAX_FOLDER_ENTRIES + 10 {
            all.push(candidate(
                not_unicode(&format!("Processed-A{i:05}"), ""),
                Kind::Dir,
            ));
            all.push(candidate(format!("Processed-B{i:05}").into(), Kind::Link));
            all.push(candidate(format!("Processed-C{i:05}").into(), Kind::File));
        }
        all.push(candidate("Processed-Z".into(), Kind::Dir));
        all.push(candidate("Other".into(), Kind::Dir));
        let listed = select(all, host_verdict);
        let taken: Vec<&str> = listed.taken.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(taken, ["Processed-Z"]);
        assert_eq!(listed.taken_count, 1);
        // The links are reported, the first 32 by name, and all counted.
        assert_eq!(listed.reported.len(), MAX_SKIPPED_NAMED);
        assert_eq!(listed.reported[0], ("Processed-B00000".to_string(), LINK));
        assert_eq!(listed.reported_count, MAX_FOLDER_ENTRIES + 10);
    }

    /// The same for session files: folders, links, devices and names that
    /// are not Unicode, sorting first, leave the real session file its place.
    #[test]
    fn only_real_session_files_take_a_place() {
        let mut all = Vec::new();
        for i in 0..MAX_FOLDER_ENTRIES + 10 {
            all.push(candidate(
                not_unicode(&format!("a{i:05}"), SUFFIX),
                Kind::File,
            ));
            all.push(candidate(format!("b{i:05}{SUFFIX}").into(), Kind::Dir));
            all.push(candidate(format!("c{i:05}{SUFFIX}").into(), Kind::Link));
            all.push(candidate(format!("d{i:05}{SUFFIX}").into(), Kind::Other));
        }
        all.push(candidate(format!("z{SUFFIX}").into(), Kind::File));
        let listed = select(all, session_verdict);
        let taken: Vec<&str> = listed.taken.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(taken, [format!("z{SUFFIX}")]);
        assert_eq!(listed.taken_count, 1);
        // Bad names, links and devices are reported; folders are not.
        assert_eq!(listed.reported_count, 3 * (MAX_FOLDER_ENTRIES + 10));
        assert_eq!(
            listed.reported[0].1, "the file name is not valid Unicode",
            "{:?}",
            listed.reported[0]
        );
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
