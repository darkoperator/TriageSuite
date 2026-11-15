//! Tags: short lower-case labels on rows.
//!
//! An agent's tags carry the `ai:` prefix and an analyst's never do, so the
//! prefix alone says whose a tag is. An agent removes only a tag it applied.

use super::history::{record_all, Change};
use super::{
    check_batch, id_list, locate_rows, no_row, per_id_chunk, unwritable, who_may_undo, write_rows,
    LocatedRow, RowOutcome, IDS_PER_STATEMENT,
};
use crate::actor::Actor;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::query::check_offset;
use crate::time::now_iso;
use duckdb::types::Value as SqlValue;
use duckdb::{params, Connection};
use std::collections::HashMap;

/// The prefix every agent tag carries, and no analyst tag may.
pub const AGENT_TAG_PREFIX: &str = "ai:";
/// The longest tag, in characters.
pub const MAX_TAG: usize = 64;

#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TagInfo {
    pub name: String,
    /// Rows carrying it, rows out of the view included.
    pub rows: u64,
    pub created_by: String,
}

/// `text` lower-cased, when it is 1 to `MAX_TAG` characters of
/// `[a-z0-9:_.-]` once lower-cased; otherwise `Invalid`.
fn normalise(text: &str) -> Result<String> {
    let tag = text.to_ascii_lowercase();
    let valid = (1..=MAX_TAG).contains(&tag.len())
        && tag
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b":_.-".contains(&b));
    if !valid {
        return Err(EngineError::Invalid(format!(
            "{text:?} is not a tag: a tag is 1 to {MAX_TAG} characters of a-z 0-9 : _ . -"
        )));
    }
    Ok(tag)
}

/// Tag each of `rows` with `tag`, stored lower-case. An agent's tag must
/// start `ai:` and an analyst's must not; either actor using the other's
/// namespace is `Refused`, since the same tag is fine from the other actor.
/// An unknown or inactive row is not done; a row already carrying the tag
/// is done, unchanged. Whole-call problems as `mark_suspicious`'s, and
/// `Invalid` for a malformed tag, `ai:` alone among them: the prefix says
/// whose a tag is, not what it says.
pub fn add_tag(case: &mut Case, rows: &[i64], tag: &str, actor: &Actor) -> Result<Vec<RowOutcome>> {
    let tag = normalise(tag)?;
    if tag == AGENT_TAG_PREFIX {
        return Err(EngineError::Invalid(format!(
            "{AGENT_TAG_PREFIX:?} alone is not a tag: an agent's tag is {AGENT_TAG_PREFIX} \
             followed by at least one character"
        )));
    }
    let prefixed = tag.starts_with(AGENT_TAG_PREFIX);
    if actor.is_agent() && !prefixed {
        return Err(EngineError::Refused(format!(
            "an agent's tag starts {AGENT_TAG_PREFIX}; {tag:?} does not"
        )));
    }
    if !actor.is_agent() && prefixed {
        return Err(EngineError::Refused(format!(
            "a tag starting {AGENT_TAG_PREFIX} is an agent's; an analyst's tag may not"
        )));
    }
    let rows = &check_batch(rows, actor)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let mut batch = TagBatch::new(&tx, rows, &tag, actor, &at)?;
    let out = rows.iter().map(|&row| batch.add(row)).collect();
    batch.write()?;
    tx.commit()?;
    Ok(out)
}

/// Remove `tag` from each of `rows`. An agent removes only a tag it
/// applied; a row without the tag is not done. A row out of the view may
/// still lose a tag it carries. Whole-call problems as `add_tag`'s, but the
/// prefix rules do not apply: the per-row owner check decides (so a bare
/// `ai:` an earlier build let an agent add can still be removed).
pub fn remove_tag(
    case: &mut Case,
    rows: &[i64],
    tag: &str,
    actor: &Actor,
) -> Result<Vec<RowOutcome>> {
    let tag = normalise(tag)?;
    let rows = &check_batch(rows, actor)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let mut batch = TagBatch::new(&tx, rows, &tag, actor, &at)?;
    let out = rows.iter().map(|&row| batch.remove(row)).collect();
    batch.write()?;
    tx.commit()?;
    Ok(out)
}

/// One call's change to one tag over its rows, each named once. It reads
/// what it needs about them up front, decides each row in order, and writes
/// every change at the end a few statements at a time.
struct TagBatch<'a> {
    conn: &'a Connection,
    tag: &'a str,
    actor: &'a Actor,
    at: &'a str,
    located: HashMap<i64, LocatedRow>,
    /// Who tagged each row with `tag`, and when, as the batch has left it.
    tagged: HashMap<i64, (String, String)>,
    added: Vec<i64>,
    removed: Vec<i64>,
    history: Vec<Change<'static>>,
}

impl<'a> TagBatch<'a> {
    fn new(
        conn: &'a Connection,
        rows: &[i64],
        tag: &'a str,
        actor: &'a Actor,
        at: &'a str,
    ) -> Result<TagBatch<'a>> {
        Ok(TagBatch {
            conn,
            tag,
            actor,
            at,
            located: locate_rows(conn, rows)?,
            tagged: tags_on(conn, rows, tag)?,
            added: Vec::new(),
            removed: Vec::new(),
            history: Vec::new(),
        })
    }

    fn add(&mut self, row: i64) -> RowOutcome {
        let tag = self.tag;
        if let Some(reason) = unwritable(&self.located, row) {
            return RowOutcome::refused(row, reason);
        }
        if let Some((by, _)) = self.tagged.get(&row) {
            return RowOutcome::unchanged(
                row,
                format!("row {row} already carries {tag}, tagged by {by}"),
            );
        }
        let label = self.actor.label();
        let after = tag_json(row, tag, &label, self.at);
        self.history.push(change(row, "tag_add", None, Some(after)));
        self.added.push(row);
        self.tagged.insert(row, (label, self.at.to_string()));
        RowOutcome::changed(row)
    }

    fn remove(&mut self, row: i64) -> RowOutcome {
        let tag = self.tag;
        let Some((by, tagged_at)) = self.tagged.get(&row).cloned() else {
            let reason = if self.located.contains_key(&row) {
                format!("row {row} does not carry {tag}")
            } else {
                no_row(row)
            };
            return RowOutcome::refused(row, reason);
        };
        if self.actor.is_agent() && by != self.actor.label() {
            return RowOutcome::refused(
                row,
                format!(
                    "tagged {tag} by {by}; only {} can remove it",
                    who_may_undo(&by)
                ),
            );
        }
        let before = tag_json(row, tag, &by, &tagged_at);
        self.history
            .push(change(row, "tag_remove", Some(before), None));
        self.removed.push(row);
        self.tagged.remove(&row);
        RowOutcome::changed(row)
    }

    /// Write every change the batch decided, and its history. The tag is
    /// defined first, when the batch put it on any row.
    fn write(self) -> Result<()> {
        let (conn, tag, at) = (self.conn, self.tag, self.at);
        let label = self.actor.label();
        per_id_chunk(
            conn,
            "DELETE FROM ox.row_tag WHERE tag = ? AND row_id IN ({ids})",
            &[SqlValue::Text(tag.to_string())],
            &self.removed,
        )?;
        if !self.removed.is_empty() {
            // A tag is defined while a row carries it.
            conn.execute(
                "DELETE FROM ox.tag WHERE name = ? \
                 AND NOT EXISTS (SELECT 1 FROM ox.row_tag WHERE tag = ?)",
                params![tag, tag],
            )?;
        }
        if !self.added.is_empty() {
            conn.execute(
                "INSERT INTO ox.tag (name, created_by, created_at) VALUES (?, ?, ?) \
                 ON CONFLICT DO NOTHING",
                params![tag, label, at],
            )?;
        }
        let rows: Vec<Vec<SqlValue>> = self
            .added
            .iter()
            .map(|row| {
                vec![
                    SqlValue::BigInt(*row),
                    SqlValue::Text(tag.to_string()),
                    SqlValue::Text(label.clone()),
                    SqlValue::Text(at.to_string()),
                ]
            })
            .collect();
        write_rows(
            conn,
            "INSERT INTO ox.row_tag (row_id, tag, tagged_by, tagged_at) VALUES {values}",
            "(?, ?, ?, ?)",
            &rows,
        )?;
        record_all(conn, self.actor, at, self.history)
    }
}

/// Who tagged each of `rows` that carries `tag`, and when: the earliest,
/// should a row carry it more than once.
fn tags_on(conn: &Connection, rows: &[i64], tag: &str) -> Result<HashMap<i64, (String, String)>> {
    let mut out = HashMap::new();
    for chunk in rows.chunks(IDS_PER_STATEMENT) {
        let mut stmt = conn.prepare(&format!(
            "SELECT row_id, tagged_by, tagged_at FROM ox.row_tag \
             WHERE tag = ? AND row_id IN ({}) \
             QUALIFY row_number() OVER (PARTITION BY row_id ORDER BY tagged_at) = 1",
            id_list(chunk)
        ))?;
        let found = stmt.query_map(params![tag], |r| {
            Ok((r.get::<_, i64>(0)?, (r.get(1)?, r.get(2)?)))
        })?;
        for row in found {
            let (row, by_at) = row?;
            out.insert(row, by_at);
        }
    }
    Ok(out)
}

fn tag_json(row: i64, tag: &str, by: &str, at: &str) -> serde_json::Value {
    serde_json::json!({ "row_id": row, "tag": tag, "tagged_by": by, "tagged_at": at })
}

/// The history entry for a change to one tag on `row`. The target is the
/// row; the tag is in the before and after objects.
fn change(
    row: i64,
    action: &str,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
) -> Change<'_> {
    Change {
        target_kind: "row_tag",
        target_id: row.to_string(),
        action,
        before,
        after,
    }
}

/// Up to `limit` of the tags some row carries, from `offset`, by name, with
/// the rows carrying each (rows out of the view included). A tag is defined
/// while a row carries it: removing it from its last row drops it, and a
/// definition an earlier build left without rows is not listed. `Invalid`
/// when `limit` is 0 or over `MAX_LIMIT`, or `offset` past `i64::MAX`.
pub fn tags(case: &Case, limit: u32, offset: u64) -> Result<Listing<TagInfo>> {
    check_limit(limit)?;
    check_offset(offset)?;
    let fetched = case
        .conn()
        .prepare(
            "SELECT t.name, count(rt.row_id), t.created_by FROM ox.tag t \
             JOIN ox.row_tag rt ON rt.tag = t.name \
             GROUP BY t.name, t.created_by ORDER BY t.name LIMIT ? OFFSET ?",
        )?
        .query_map(params![fetch_limit(limit), offset], |r| {
            let rows: i64 = r.get(1)?;
            Ok(TagInfo {
                name: r.get(0)?,
                rows: u64::try_from(rows).unwrap_or_default(),
                created_by: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(Listing::from_fetched(fetched, limit))
}
