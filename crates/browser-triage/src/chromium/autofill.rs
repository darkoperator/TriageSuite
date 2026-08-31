//! Chromium `Web Data` -> `autofill`.
//!
//! Everything typed into a form and remembered: usernames, email addresses,
//! search boxes, addresses. Plaintext, and frequently the most directly
//! attributable artifact in a browser profile.
//!
//! # The epoch trap
//!
//! `autofill.date_created` and `date_last_used` are **unix seconds**
//! (`base::Time::ToTimeT`), not the WebKit microseconds every other Chromium
//! table uses. Reading them as WebKit would put every row in the year 1601 —
//! wrong, but plausible-looking enough to survive review, which is why it has
//! its own test.
//!
//! `Web Data` also holds `credit_cards`, `local_addresses` and `keywords`.
//! Those are out of scope for now and their absence is documented, so nobody
//! reads this dataset as the whole file.

use crate::profile::BrowserId;
use crate::records::AutofillRecord;
use crate::sql::{self, Notes};
use crate::timeline::{artifact_name, kind, Timeline};
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_sqlite::Database;

pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    if !db.table_exists("autofill").unwrap_or(false) {
        return Ok(0);
    }
    let cols = sql::columns(db, "autofill");
    let source = path.display().to_string();
    let mut written = 0u64;

    let sql_text = format!(
        "SELECT {} FROM autofill ORDER BY date_created",
        sql::projection(
            &cols,
            &["name", "value", "count", "date_created", "date_last_used"]
        )
    );
    let rows = db.query(&sql_text).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: format!("autofill: {e}"),
    })?;

    for row in &rows {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }

        let field_name = sql::text(sql::cell(row, 0));
        let value = sql::text(sql::cell(row, 1));
        notes.note_if_lossy("Field Name", &field_name);
        notes.note_if_lossy("Value", &value);

        // Unix seconds here, not WebKit microseconds, and 0 means "never"
        // rather than 1970-01-01. See the module docs and time_t_or_none.
        let first_used = super::time_t_or_none(sql::int(sql::cell(row, 3)));
        let last_used = super::time_t_or_none(sql::int(sql::cell(row, 4)));

        let label = format!("{field_name}={value}");
        out.write(
            "autofill",
            &AutofillRecord {
                browser: id.browser.clone(),
                channel: id.channel.clone(),
                profile: id.profile.clone(),
                field_name,
                value,
                use_count: sql::int(sql::cell(row, 2)),
                first_used,
                last_used,
                guid: String::new(),
                entry_id: None,
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
