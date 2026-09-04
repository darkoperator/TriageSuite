//! Chromium `downloads` + `downloads_url_chains`.
//!
//! The URL chain is the reason this is two tables rather than one: a download
//! that arrived through redirects records each hop, and the final `target_path`
//! frequently says nothing about where the file actually came from.
//!
//! As in history, the orphan pass is deliberate: a `downloads_url_chains` group
//! whose parent row has been deleted still carries the URLs, and those are
//! evidence.

use crate::profile::BrowserId;
use crate::records::DownloadRecord;
use crate::sql::{self, Notes};
use crate::timeline::{artifact_name, kind, Timeline};
use std::collections::BTreeMap;
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_sqlite::Database;

const DOWNLOAD_COLUMNS: &[&str] = &[
    "id",
    "guid",
    "current_path",
    "target_path",
    "start_time",
    "received_bytes",
    "total_bytes",
    "state",
    "danger_type",
    "interrupt_reason",
    "hash",
    "end_time",
    "opened",
    "last_access_time",
    "referrer",
    "site_url",
    "tab_url",
    "tab_referrer_url",
    "by_ext_id",
    "by_ext_name",
    "etag",
    "last_modified",
    "mime_type",
    "original_mime_type",
];

fn artifact_error(path: &Path, message: impl std::fmt::Display) -> TriageError {
    TriageError::Artifact {
        path: path.to_path_buf(),
        message: message.to_string(),
    }
}

/// `download id -> urls in chain_index order`, plus why the chains are missing
/// when they are.
///
/// A failure here used to return silently, so every download row showed a blank
/// `Download URL` and `URL Chain` with nothing to say the table had not been
/// read. Since the chain is often the only record of where a file came from, an
/// unreadable chain table must not look like a download with no origin.
fn load_url_chains(db: &Database) -> (BTreeMap<i64, Vec<String>>, Option<String>) {
    let mut chains: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    if !db.table_exists("downloads_url_chains").unwrap_or(false) {
        return (
            chains,
            Some("downloads_url_chains table is absent".to_string()),
        );
    }
    let rows = match db
        .query("SELECT id, chain_index, url FROM downloads_url_chains ORDER BY id, chain_index")
    {
        Ok(rows) => rows,
        Err(error) => {
            return (
                chains,
                Some(format!("downloads_url_chains unreadable: {error}")),
            )
        }
    };
    let mut skipped = 0u64;
    for row in &rows {
        match sql::int(sql::cell(row, 0)) {
            Some(id) => chains
                .entry(id)
                .or_default()
                .push(sql::text(sql::cell(row, 2))),
            None => skipped += 1,
        }
    }
    let note = (skipped > 0).then(|| {
        format!("{skipped} downloads_url_chains row(s) had an unusable id and were not attached")
    });
    (chains, note)
}

#[allow(clippy::too_many_lines)]
pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    if !db.table_exists("downloads").unwrap_or(false) {
        return Ok(0);
    }

    let source = path.display().to_string();
    let cols = sql::columns(db, "downloads");
    let (mut chains, chain_note) = load_url_chains(db);
    let mut written = 0u64;

    let sql_text = format!(
        "SELECT {} FROM downloads ORDER BY id",
        sql::projection(&cols, DOWNLOAD_COLUMNS)
    );
    let rows = db
        .query(&sql_text)
        .map_err(|e| artifact_error(path, format!("downloads: {e}")))?;

    for row in &rows {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }

        let download_id = sql::int(sql::cell(row, 0));
        // Take the chain, so whatever is left over is genuinely orphaned.
        let chain = download_id
            .and_then(|d| chains.remove(&d))
            .unwrap_or_default();
        // Say why the origin is blank, so it is never read as "this download
        // had no recorded source".
        if chain.is_empty() {
            if let Some(why) = &chain_note {
                notes.push(why.clone());
            }
        }

        let start_time =
            WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 4)).unwrap_or_default());
        let end_time =
            WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 11)).unwrap_or_default());
        let last_access_time =
            WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 13)).unwrap_or_default());

        let target_path = sql::text(sql::cell(row, 3));
        let current_path = sql::text(sql::cell(row, 2));
        notes.note_if_lossy("Target Path", &target_path);

        // Declared BLOB, but SQLite affinity is advisory and real profiles
        // hand some of these back as Text — see sql::bytes.
        let hash_sha256 = super::hex(sql::bytes(sql::cell(row, 10)));

        let by_ext_id = sql::text(sql::cell(row, 18));
        let by_ext_name = sql::text(sql::cell(row, 19));
        let by_extension = match (by_ext_id.is_empty(), by_ext_name.is_empty()) {
            (true, true) => String::new(),
            (false, false) => format!("{by_ext_name} ({by_ext_id})"),
            (true, false) => by_ext_name,
            (false, true) => by_ext_id,
        };

        let record = DownloadRecord {
            browser: id.browser.clone(),
            channel: id.channel.clone(),
            profile: id.profile.clone(),
            record_type: DownloadRecord::DOWNLOAD,
            start_time,
            end_time,
            last_access_time,
            target_path: target_path.clone(),
            current_path: current_path.clone(),
            download_url: chain.first().cloned().unwrap_or_default(),
            url_chain: chain.join(" -> "),
            referrer: sql::text(sql::cell(row, 14)),
            tab_url: sql::text(sql::cell(row, 16)),
            tab_referrer_url: sql::text(sql::cell(row, 17)),
            site_url: sql::text(sql::cell(row, 15)),
            mime_type: sql::text(sql::cell(row, 22)),
            original_mime_type: sql::text(sql::cell(row, 23)),
            received_bytes: sql::int(sql::cell(row, 5)),
            total_bytes: sql::int(sql::cell(row, 6)),
            state: sql::int(sql::cell(row, 7))
                .map(super::download_state)
                .unwrap_or_default(),
            danger_type: sql::int(sql::cell(row, 8))
                .map(super::danger_type)
                .unwrap_or_default(),
            interrupt_reason: sql::int(sql::cell(row, 9))
                .map(super::interrupt_reason)
                .unwrap_or_default(),
            opened: sql::bool_str(sql::cell(row, 12)),
            by_extension,
            hash_sha256,
            etag: sql::text(sql::cell(row, 20)),
            last_modified_header: sql::text(sql::cell(row, 21)),
            guid: sql::text(sql::cell(row, 1)),
            download_id,
            notes: notes.into_string(),
            source_file: source.clone(),
        };
        out.write("downloads", &record)?;
        written += 1;

        // A download has up to three distinct instants, and each is a separate
        // event on the timeline — the same fan-out PETriage does for run times.
        let label = if target_path.is_empty() {
            record_label(&current_path, &record.download_url)
        } else {
            target_path.clone()
        };
        timeline.push(
            out,
            start_time,
            kind::DOWNLOAD_STARTED,
            artifact_name::DOWNLOADS,
            &label,
        )?;
        timeline.push(
            out,
            end_time,
            kind::DOWNLOAD_COMPLETED,
            artifact_name::DOWNLOADS,
            &label,
        )?;
        timeline.push(
            out,
            last_access_time,
            kind::DOWNLOAD_LAST_ACCESSED,
            artifact_name::DOWNLOADS,
            &label,
        )?;
    }

    // Whatever chains were not claimed above have no parent row.
    for (orphan_id, chain) in chains {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }
        notes.push(format!(
            "downloads_url_chains id {orphan_id} has no matching downloads row"
        ));
        let record = DownloadRecord {
            browser: id.browser.clone(),
            channel: id.channel.clone(),
            profile: id.profile.clone(),
            record_type: DownloadRecord::ORPHAN_CHAIN,
            download_url: chain.first().cloned().unwrap_or_default(),
            url_chain: chain.join(" -> "),
            download_id: Some(orphan_id),
            notes: notes.into_string(),
            source_file: source.clone(),
            ..Default::default()
        };
        out.write("downloads", &record)?;
        written += 1;
    }

    Ok(written)
}

/// The most identifying non-empty value available for a timeline row.
fn record_label(current_path: &str, download_url: &str) -> String {
    if !current_path.is_empty() {
        current_path.to_string()
    } else {
        download_url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_falls_back_through_the_available_paths() {
        assert_eq!(
            record_label("/tmp/a.crdownload", "http://x"),
            "/tmp/a.crdownload"
        );
        assert_eq!(record_label("", "http://x"), "http://x");
        assert_eq!(record_label("", ""), "");
    }
}
