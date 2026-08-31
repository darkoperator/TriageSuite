//! Firefox `formhistory.sqlite` -> `moz_formhistory`.
//!
//! The counterpart to Chromium's `autofill`, with the opposite epoch:
//! `firstUsed` and `lastUsed` are PRTime microseconds, where Chromium's are
//! unix seconds. Both feed the same dataset, which is exactly why each side
//! carries a test asserting its own epoch.

use crate::profile::BrowserId;
use crate::records::AutofillRecord;
use crate::sql::{self, Notes};
use crate::timeline::{artifact_name, kind, Timeline};
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_sqlite::Database;

pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    if !db.table_exists("moz_formhistory").unwrap_or(false) {
        return Ok(0);
    }
    let cols = sql::columns(db, "moz_formhistory");
    let source = path.display().to_string();
    let mut written = 0u64;

    let sql_text = format!(
        "SELECT {} FROM moz_formhistory ORDER BY id",
        sql::projection(
            &cols,
            &[
                "id",
                "fieldname",
                "value",
                "timesUsed",
                "firstUsed",
                "lastUsed",
                "guid"
            ]
        )
    );
    let rows = db.query(&sql_text).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: format!("moz_formhistory: {e}"),
    })?;

    for row in &rows {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }

        let field_name = sql::text(sql::cell(row, 1));
        let value = sql::text(sql::cell(row, 2));
        notes.note_if_lossy("Value", &value);

        // PRTime microseconds, unlike Chromium's unix seconds.
        let first_used =
            WinTimestamp::from_unix_micros(sql::int(sql::cell(row, 4)).unwrap_or_default());
        let last_used =
            WinTimestamp::from_unix_micros(sql::int(sql::cell(row, 5)).unwrap_or_default());

        let label = format!("{field_name}={value}");
        out.write(
            "autofill",
            &AutofillRecord {
                browser: id.browser.clone(),
                channel: id.channel.clone(),
                profile: id.profile.clone(),
                field_name,
                value,
                use_count: sql::int(sql::cell(row, 3)),
                first_used,
                last_used,
                guid: sql::text(sql::cell(row, 6)),
                entry_id: sql::int(sql::cell(row, 0)),
                notes: notes.into_string(),
                source_file: source.clone(),
            },
        )?;
        written += 1;

        timeline.push(
            out,
            first_used,
            kind::AUTOFILL_FIRST_USED,
            artifact_name::AUTOFILL,
            &label,
        )?;
        timeline.push(
            out,
            last_used,
            kind::AUTOFILL_LAST_USED,
            artifact_name::AUTOFILL,
            &label,
        )?;
    }

    Ok(written)
}
