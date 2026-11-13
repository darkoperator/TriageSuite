//! Notes on rows. Deleting one leaves a tombstone: the note is kept and
//! flagged, never removed, so the history's account of it stays readable.

use super::history::{record, Change};
use super::{find_row, inactive, no_row};
use crate::actor::Actor;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::query::check_offset;
use crate::time::now_iso;
use duckdb::{params, Connection};

/// The longest note, in characters.
pub const MAX_NOTE: usize = 10_000;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Note {
    pub id: i64,
    pub row_id: i64,
    pub text: String,
    pub written_by: String,
    pub written_at: String,
}

impl Note {
    fn json(&self, deleted: bool) -> serde_json::Value {
        serde_json::json!({
            "id": self.id, "row_id": self.row_id, "text": self.text,
            "written_by": self.written_by, "written_at": self.written_at,
            "deleted": deleted,
        })
    }
}

/// Write `text` (1 to `MAX_NOTE` characters) as a note on row `row_id`.
/// `Invalid` for empty or overlong text; `NotFound` for an unknown row;
/// `Refused` for a row of a superseded run or a removed source.
pub fn add_note(case: &mut Case, row_id: i64, text: &str, actor: &Actor) -> Result<Note> {
    let length = text.chars().count();
    if !(1..=MAX_NOTE).contains(&length) {
        return Err(EngineError::Invalid(format!(
            "a note is 1 to {MAX_NOTE} characters, not {length}"
        )));
    }
    let tx = case.conn_mut().transaction()?;
    let row = find_row(&tx, row_id)?.ok_or_else(|| EngineError::NotFound(no_row(row_id)))?;
    if let Some(reason) = inactive(&row) {
        return Err(EngineError::Refused(reason));
    }
    let at = now_iso();
    let id: i64 = tx.query_row(
        "INSERT INTO ox.note (id, row_id, text, written_by, written_at) \
         VALUES (nextval('ox.note_seq'), ?, ?, ?, ?) RETURNING id",
        params![row_id, text, actor.label(), at],
        |r| r.get(0),
    )?;
    let note = Note {
        id,
        row_id,
        text: text.to_string(),
        written_by: actor.label(),
        written_at: at.clone(),
    };
    record(
        &tx,
        actor,
        &at,
        change(id, "note_add", None, Some(note.json(false))),
    )?;
    tx.commit()?;
    Ok(note)
}

/// Delete note `note_id`, leaving a tombstone. An agent deletes only its
/// own (`Refused` otherwise). `NotFound` for an unknown id; `Refused` for a
/// note already deleted.
pub fn delete_note(case: &mut Case, note_id: i64, actor: &Actor) -> Result<()> {
    let tx = case.conn_mut().transaction()?;
    let (note, deleted) = read_note(&tx, note_id)?;
    if deleted {
        return Err(EngineError::Refused(format!(
            "note {note_id} was already deleted"
        )));
    }
    if actor.is_agent() && note.written_by != actor.label() {
        return Err(EngineError::Refused(format!(
            "note {note_id} was written by {}; an agent deletes only its own",
            note.written_by
        )));
    }
    tx.execute(
        "UPDATE ox.note SET deleted = true WHERE id = ?",
        params![note_id],
    )?;
    let deletion = change(
        note_id,
        "note_delete",
        Some(note.json(false)),
        Some(note.json(true)),
    );
    record(&tx, actor, &now_iso(), deletion)?;
    tx.commit()?;
    Ok(())
}

fn read_note(conn: &Connection, note_id: i64) -> Result<(Note, bool)> {
    conn.query_row(
        "SELECT id, row_id, text, written_by, written_at, deleted FROM ox.note WHERE id = ?",
        params![note_id],
        |r| Ok((note_row(r)?, r.get(5)?)),
    )
    .map_err(|e| match e {
        duckdb::Error::QueryReturnedNoRows => EngineError::NotFound(format!("no note {note_id}")),
        other => other.into(),
    })
}

fn note_row(r: &duckdb::Row<'_>) -> duckdb::Result<Note> {
    Ok(Note {
        id: r.get(0)?,
        row_id: r.get(1)?,
        text: r.get(2)?,
        written_by: r.get(3)?,
        written_at: r.get(4)?,
    })
}

/// The history entry for a change to note `id`.
fn change(
    id: i64,
    action: &str,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
) -> Change<'_> {
    Change {
        target_kind: "note",
        target_id: id.to_string(),
        action,
        before,
        after,
    }
}

/// Up to `limit` of the notes on row `row_id` that are not deleted, from
/// `offset`, oldest first; none for an unknown row. `Invalid` when `limit`
/// is 0 or over `MAX_LIMIT`, or `offset` past `i64::MAX`.
pub fn notes(case: &Case, row_id: i64, limit: u32, offset: u64) -> Result<Listing<Note>> {
    check_limit(limit)?;
    check_offset(offset)?;
    let fetched = case
        .conn()
        .prepare(&format!("{NOTES_ON} LIMIT ? OFFSET ?"))?
        .query_map(params![row_id, fetch_limit(limit), offset], note_row)?
        .collect::<std::result::Result<_, _>>()?;
    Ok(Listing::from_fetched(fetched, limit))
}

/// Every note on `row_id` that is not deleted, for the row detail, which
/// shows a row whole.
pub(super) fn notes_on(conn: &Connection, row_id: i64) -> Result<Vec<Note>> {
    Ok(conn
        .prepare(NOTES_ON)?
        .query_map(params![row_id], note_row)?
        .collect::<std::result::Result<_, _>>()?)
}

const NOTES_ON: &str = "SELECT id, row_id, text, written_by, written_at FROM ox.note \
                        WHERE row_id = ? AND NOT deleted ORDER BY id";
