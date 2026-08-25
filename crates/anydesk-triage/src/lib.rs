//! AnyDeskTriage: parses AnyDesk's trace/connection log files: `ad.trace`
//! (interactive client, per-user under `%AppData%\AnyDesk\`), `ad_svc.trace`
//! (background service, system-wide under `%ProgramData%\AnyDesk\`), and
//! `connection_trace.txt` (session-level log, alongside each of the above),
//! plus each file's `.old` rotation.
//!
//! PROVISIONAL: AnyDesk's log format is not vendor-documented. The grammar
//! below was reconstructed from multiple independent sources that converge
//! on the same shape: published DFIR write-ups showing real example lines,
//! the `anydesk-log-reader` project's working parse regex (for both
//! `ad.trace`/`ad_svc.trace` and `connection_trace.txt`), and `AnyGrabber`'s
//! keyword-search approach (`'Logged in from '`, fuzzy-parsing a date out of
//! the line's first 30 characters — consistent with a short level word
//! immediately followed by the timestamp). Still not a self-generated
//! sample — treat field semantics as a strong, cross-corroborated
//! hypothesis, not ground truth, until confirmed against real logs.
//! `parse_trace_line`/`parse_connection_line` are the two functions to
//! correct once that confirmation happens.
//!
//! Reconstructed `ad.trace`/`ad_svc.trace` grammar (whitespace/column-padded
//! in the real file; padding is not preserved here beyond `RawLine`):
//!   <level> <YYYY-MM-DD> <HH:MM:SS[.mmm]> <component> <pid> <tid> [<num>] <module> - <message>
//! e.g. `info 2022-03-18 01:56:24.672 front 2428 7036 main - Process started...`
//! The optional bracketed numeric field (seen between tid and module in some
//! lines, e.g. a `3` before `anynet.relay_conn`) has unconfirmed meaning;
//! `anydesk-log-reader`'s regex treats it as always-present, which may mean
//! it's more common than the "optional" framing here suggests.
//!
//! Reconstructed `connection_trace.txt` grammar (two independent sources
//! now agree: a blog example and `anydesk-log-reader`'s regex):
//!   <Incoming|Outgoing> <date>, <time> <category> <remote_id> [<...more, unparsed>]
//! e.g. `Incoming 2022-03-18, 02:50 User 732092099 732092099` (category
//! "User"; the second numeric field's meaning is unconfirmed — kept in
//! `Message` rather than dropped).

use std::io::{BufRead, BufReader};
use std::path::Path;

use chrono::{NaiveDate, NaiveTime};
use serde::Serialize;
use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};

pub const DATASETS: &[DatasetSpec] = &[DatasetSpec {
    id: "raw_lines",
    default_basename: "AnyDeskTriage_Output",
    framing: JsonFraming::Ndjson,
    csv_only: false,
    override_suffix: None,
}];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceLineRecord {
    #[serde(rename = "SourceFile")]
    pub source_file: String,
    #[serde(rename = "LineNumber")]
    pub line_number: u64,
    /// "TraceLine" (matched the ad.trace/ad_svc.trace grammar),
    /// "ConnectionEvent" (matched the connection_trace.txt grammar), or
    /// "Unrecognized" (neither — only SourceFile/LineNumber/RawLine are
    /// populated).
    #[serde(rename = "LineKind")]
    pub line_kind: &'static str,
    /// TraceLine only: "info"/"error"/"warning"/etc, verbatim.
    #[serde(rename = "Level")]
    pub level: String,
    /// TraceLine only: "YYYY-MM-DD HH:MM:SS[.mmm]" verbatim. Not converted
    /// to `WinTimestamp`/UTC — this log's timestamps are believed to be
    /// host-local time, unconfirmed.
    #[serde(rename = "RawTimestamp")]
    pub raw_timestamp: String,
    /// TraceLine only: short subsystem tag, e.g. "front"/"lsvc"/"lctrl".
    #[serde(rename = "Component")]
    pub component: String,
    #[serde(rename = "Pid")]
    pub pid: Option<i64>,
    #[serde(rename = "Tid")]
    pub tid: Option<i64>,
    /// TraceLine only: dotted module/action name, e.g. "anynet.relay_conn".
    #[serde(rename = "Module")]
    pub module: String,
    /// ConnectionEvent only: "Incoming" or "Outgoing".
    #[serde(rename = "Direction")]
    pub direction: String,
    /// ConnectionEvent only: "YYYY-MM-DD, HH:MM" verbatim (note the comma —
    /// part of the real format, not a typo). Not converted to
    /// `WinTimestamp`/UTC, same reasoning as `RawTimestamp` above.
    #[serde(rename = "ConnectionRawTimestamp")]
    pub connection_raw_timestamp: String,
    /// ConnectionEvent only: e.g. "User" — the auth/connection method.
    #[serde(rename = "Category")]
    pub category: String,
    /// ConnectionEvent only: the remote AnyDesk ID.
    #[serde(rename = "RemoteId")]
    pub remote_id: String,
    /// TraceLine: text after " - ", whitespace-normalized. ConnectionEvent:
    /// any tokens beyond RemoteId (unconfirmed meaning), whitespace-
    /// normalized. Unrecognized: empty (see RawLine).
    #[serde(rename = "Message")]
    pub message: String,
    /// The original line, untouched, for every LineKind.
    #[serde(rename = "RawLine")]
    pub raw_line: String,
}

struct ParsedTraceLine {
    level: String,
    raw_timestamp: String,
    component: String,
    pid: i64,
    tid: i64,
    module: String,
    message: String,
}

fn looks_like_date(s: &str) -> bool {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

fn looks_like_time(s: &str) -> bool {
    match s.split_once('.') {
        Some((hms, frac)) => {
            !frac.is_empty()
                && frac.bytes().all(|b| b.is_ascii_digit())
                && NaiveTime::parse_from_str(hms, "%H:%M:%S").is_ok()
        }
        None => NaiveTime::parse_from_str(s, "%H:%M:%S").is_ok(),
    }
}

/// Matches `<level> <date> <time> <component> <pid> <tid> [<num>] <module> - <message>`.
fn parse_trace_line(line: &str) -> Option<ParsedTraceLine> {
    let mut it = line.split_whitespace();

    let level = it.next()?;
    if level.is_empty() || !level.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let date = it.next()?;
    if !looks_like_date(date) {
        return None;
    }
    let time = it.next()?;
    if !looks_like_time(time) {
        return None;
    }
    let component = it.next()?;
    let pid: i64 = it.next()?.parse().ok()?;
    let tid: i64 = it.next()?.parse().ok()?;

    let mut module = it.next()?;
    if module.bytes().all(|b| b.is_ascii_digit()) {
        // Optional numeric field between tid and module; unconfirmed meaning.
        module = it.next()?;
    }

    if it.next()? != "-" {
        return None;
    }
    let message = it.collect::<Vec<_>>().join(" ");

    Some(ParsedTraceLine {
        level: level.to_string(),
        raw_timestamp: format!("{date} {time}"),
        component: component.to_string(),
        pid,
        tid,
        module: module.to_string(),
        message,
    })
}

struct ParsedConnectionLine {
    direction: &'static str,
    raw_timestamp: String,
    category: String,
    remote_id: String,
    message: String,
}

/// Matches `<Incoming|Outgoing> <date>, <time> <category> <remote_id> [<...more>]`
/// (connection_trace.txt), e.g.
/// `Incoming 2022-03-18, 02:50 User 732092099 732092099`.
fn parse_connection_line(line: &str) -> Option<ParsedConnectionLine> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 5 {
        return None;
    }
    let direction = match tokens[0] {
        "Incoming" => "Incoming",
        "Outgoing" => "Outgoing",
        _ => return None,
    };
    Some(ParsedConnectionLine {
        direction,
        raw_timestamp: format!("{} {}", tokens[1], tokens[2]),
        category: tokens[3].to_string(),
        remote_id: tokens[4].to_string(),
        message: tokens[5..].join(" "),
    })
}

fn is_recognized_line(line: &str) -> bool {
    parse_trace_line(line).is_some() || parse_connection_line(line).is_some()
}

fn build_record(source_file: &str, line_number: u64, line: &str) -> TraceLineRecord {
    if let Some(t) = parse_trace_line(line) {
        return TraceLineRecord {
            source_file: source_file.to_string(),
            line_number,
            line_kind: "TraceLine",
            level: t.level,
            raw_timestamp: t.raw_timestamp,
            component: t.component,
            pid: Some(t.pid),
            tid: Some(t.tid),
            module: t.module,
            direction: String::new(),
            connection_raw_timestamp: String::new(),
            category: String::new(),
            remote_id: String::new(),
            message: t.message,
            raw_line: line.to_string(),
        };
    }
    if let Some(c) = parse_connection_line(line) {
        return TraceLineRecord {
            source_file: source_file.to_string(),
            line_number,
            line_kind: "ConnectionEvent",
            level: String::new(),
            raw_timestamp: String::new(),
            component: String::new(),
            pid: None,
            tid: None,
            module: String::new(),
            direction: c.direction.to_string(),
            connection_raw_timestamp: c.raw_timestamp,
            category: c.category,
            remote_id: c.remote_id,
            message: c.message,
            raw_line: line.to_string(),
        };
    }
    TraceLineRecord {
        source_file: source_file.to_string(),
        line_number,
        line_kind: "Unrecognized",
        level: String::new(),
        raw_timestamp: String::new(),
        component: String::new(),
        pid: None,
        tid: None,
        module: String::new(),
        direction: String::new(),
        connection_raw_timestamp: String::new(),
        category: String::new(),
        remote_id: String::new(),
        message: String::new(),
        raw_line: line.to_string(),
    }
}

pub struct AnyDeskTool;

impl Default for AnyDeskTool {
    fn default() -> Self {
        AnyDeskTool
    }
}

impl Tool for AnyDeskTool {
    fn binary_name(&self) -> &'static str {
        "AnyDeskTriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &[
            "ad.trace",
            "ad.trace.old",
            "ad_svc.trace",
            "ad_svc.trace.old",
            "connection_trace.txt",
            "connection_trace.txt.old",
        ]
    }

    /// Content gate (spec 3.2: never extension-only): the first non-blank
    /// line must match either the trace-line or connection-event grammar.
    /// Still PROVISIONAL — see the module-level doc comment.
    fn validate_legacy(&self, path: &Path) -> bool {
        let Ok(file) = std::fs::File::open(path) else {
            return false;
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { return false };
            if line.trim().is_empty() {
                continue;
            }
            return is_recognized_line(&line);
        }
        false
    }

    fn invalid_content_is_corrupt(&self) -> bool {
        // The content gate is a heuristic reconstructed from third-party
        // write-ups, not a confirmed format signature; a miss is "not one
        // of ours", not corruption.
        false
    }

    fn datasets(&self) -> &'static [DatasetSpec] {
        DATASETS
    }

    /// `ad.trace`/per-user `connection_trace.txt` live under a user's
    /// `%AppData%\AnyDesk\`, resolvable to that user by path; `ad_svc.trace`
    /// lives under system-wide `%ProgramData%\AnyDesk\` with no user
    /// segment. `UserElseSystem` derives the user when the path carries
    /// one, else falls back to system — exactly this split, no
    /// per-filename special-casing needed.
    fn scope(&self) -> Scope {
        Scope::UserElseSystem
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        let file = std::fs::File::open(path).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let source_file = path.display().to_string();
        let mut count = 0u64;
        for (idx, line) in BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|e| TriageError::Artifact {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;
            if line.trim().is_empty() {
                continue;
            }
            out.write(
                "raw_lines",
                &build_record(&source_file, idx as u64 + 1, &line),
            )?;
            count += 1;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use triage_core::output::layout::OutputLayoutMode;
    use triage_core::output::router::{run_stamp, RouterOptions};

    // Example lines as published across independent DFIR write-ups (see
    // project memory for sources); not yet cross-checked against a
    // self-generated sample.
    const EX_MAIN: &str =
        "info 2022-03-18 01:56:24.672 front 2428 7036 main - Process started at 2022-03-18. PID 2428. OS is Windows 10 (64 bit)";
    const EX_CLIPBOARD: &str =
        "info 2022-08-23 23:20:16.707 lctrl 2244 2248 clipbrd.capture - Relaying text offers.";
    const EX_RELAY: &str = "info 2021-02-04 23:25:10.500 lsvc 9988 6992 3 anynet.relay_conn - External address: 116.255.x.x:47220.";

    #[test]
    fn parses_trace_line_without_extra_numeric_field() {
        let parsed = parse_trace_line(EX_MAIN).expect("should parse");
        assert_eq!(parsed.level, "info");
        assert_eq!(parsed.raw_timestamp, "2022-03-18 01:56:24.672");
        assert_eq!(parsed.component, "front");
        assert_eq!(parsed.pid, 2428);
        assert_eq!(parsed.tid, 7036);
        assert_eq!(parsed.module, "main");
        assert_eq!(
            parsed.message,
            "Process started at 2022-03-18. PID 2428. OS is Windows 10 (64 bit)"
        );
    }

    #[test]
    fn parses_trace_line_with_dotted_module() {
        let parsed = parse_trace_line(EX_CLIPBOARD).expect("should parse");
        assert_eq!(parsed.component, "lctrl");
        assert_eq!(parsed.module, "clipbrd.capture");
        assert_eq!(parsed.message, "Relaying text offers.");
    }

    #[test]
    fn parses_trace_line_with_optional_extra_numeric_field() {
        let parsed = parse_trace_line(EX_RELAY).expect("should parse");
        assert_eq!(parsed.component, "lsvc");
        assert_eq!(parsed.pid, 9988);
        assert_eq!(parsed.tid, 6992);
        assert_eq!(parsed.module, "anynet.relay_conn");
        assert_eq!(parsed.message, "External address: 116.255.x.x:47220.");
    }

    #[test]
    fn rejects_line_not_starting_with_level_and_timestamp() {
        assert!(parse_trace_line("    continuation of a stack trace").is_none());
        assert!(parse_trace_line("2022-03-18 01:56:24.672 front 2428 7036 main - msg").is_none());
        assert!(parse_trace_line("hi").is_none());
    }

    #[test]
    fn parses_incoming_connection_event() {
        let parsed = parse_connection_line("Incoming 2022-03-18, 02:50 User 732092099 732092099")
            .expect("should parse");
        assert_eq!(parsed.direction, "Incoming");
        assert_eq!(parsed.raw_timestamp, "2022-03-18, 02:50");
        assert_eq!(parsed.category, "User");
        assert_eq!(parsed.remote_id, "732092099");
        assert_eq!(parsed.message, "732092099");
    }

    #[test]
    fn connection_event_without_enough_fields_is_rejected() {
        assert!(parse_connection_line("Incoming 2022-03-18, 02:50").is_none());
    }

    #[test]
    fn non_matching_line_is_unrecognized() {
        assert!(parse_connection_line("some other text").is_none());
        assert!(!is_recognized_line("some other text"));
    }

    fn tool() -> AnyDeskTool {
        AnyDeskTool
    }

    #[test]
    fn validate_legacy_accepts_trace_and_connection_files() {
        let tmp = tempfile::tempdir().unwrap();

        let trace = tmp.path().join("ad.trace");
        std::fs::write(&trace, format!("{EX_MAIN}\n")).unwrap();
        assert!(tool().validate_legacy(&trace));

        let conn = tmp.path().join("connection_trace.txt");
        std::fs::write(
            &conn,
            "Incoming 2022-03-18, 02:50 User 732092099 732092099\n",
        )
        .unwrap();
        assert!(tool().validate_legacy(&conn));
    }

    #[test]
    fn validate_legacy_rejects_unrelated_text_file() {
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("ad.trace");
        std::fs::write(&input, "just some unrelated text\nmore text\n").unwrap();
        assert!(!tool().validate_legacy(&input));
    }

    #[test]
    fn parse_emits_one_record_per_non_blank_line_with_correct_kinds() {
        let tool = tool();
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("ad.trace");
        std::fs::write(
            &input,
            format!("{EX_MAIN}\n\n{EX_RELAY}\nsome unrecognized continuation line\n"),
        )
        .unwrap();

        let out_dir = tmp.path().join("out");
        let mut router = OutputRouter::new(
            tool.binary_name(),
            tool.datasets(),
            RouterOptions {
                csv_root: Some(out_dir.clone()),
                json_root: None,
                csvf: None,
                jsonf: None,
                pretty: false,
                overwrite: false,
                run_stamp: Some(run_stamp()),
                layout_mode: OutputLayoutMode::Flat,
            },
        )
        .unwrap();
        router.set_identity(triage_core::attribution::Identity::System);

        let count = tool.parse(&input, &mut router).unwrap();
        // Blank line skipped; 3 non-blank lines emitted.
        assert_eq!(count, 3);
    }
}
