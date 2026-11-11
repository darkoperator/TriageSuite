//! Tags: short lower-case labels on rows.
//!
//! An agent's tags carry the `ai:` prefix and an analyst's never do, so the
//! prefix alone says whose a tag is. An agent removes only a tag it applied.

use super::history::{record, Change};
use super::{check_batch, find_row, no_row, unwritable, RowOutcome};
use crate::actor::Actor;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::time::now_iso;
use duckdb::{params, Connection};

/// The prefix every agent tag carries, and no analyst tag may.
pub const AGENT_TAG_PREFIX: &str = "ai:";
/// The longest tag, in characters.
pub const MAX_TAG: usize = 64;

#[derive(Debug, Clone, serde::Serialize)]
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
/// `Invalid` for a malformed tag.
pub fn add_tag(case: &mut Case, rows: &[i64], tag: &str, actor: &Actor) -> Result<Vec<RowOutcome>> {
    let tag = normalise(tag)?;
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
    check_batch(rows, actor)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let out = rows
        .iter()
        .map(|&row| add_one(&tx, row, &tag, actor, &at))
        .collect::<Result<_>>()?;
    tx.commit()?;
    Ok(out)
}

fn add_one(conn: &Connection, row: i64, tag: &str, actor: &Actor, at: &str) -> Result<RowOutcome> {
    if let Some(reason) = unwritable(conn, row)? {
        return Ok(RowOutcome::refused(row, reason));
    }
    if let Some(by) = tagged_by(conn, row, tag)? {
        return Ok(RowOutcome::unchanged(
            row,
            format!("row {row} already carries {tag}, tagged by {by}"),
        ));
    }
    let label = actor.label();
    conn.execute(
        "INSERT INTO ox.tag (name, created_by, created_at) VALUES (?, ?, ?) \
         ON CONFLICT DO NOTHING",
        params![tag, label, at],
    )?;
    conn.execute(
        "INSERT INTO ox.row_tag (row_id, tag, tagged_by, tagged_at) VALUES (?, ?, ?, ?)",
        params![row, tag, label, at],
    )?;
    let after = tag_json(row, tag, &label, at);
    record(conn, actor, at, change(row, "tag_add", None, Some(after)))?;
    Ok(RowOutcome::changed(row))
}

/// Remove `tag` from each of `rows`. An agent removes only a tag it
/// applied; a row without the tag is not done. A row out of the view may
/// still lose a tag it carries. Whole-call problems as `add_tag`'s, but the
/// prefix rules do not apply: the per-row owner check decides.
pub fn remove_tag(
    case: &mut Case,
    rows: &[i64],
    tag: &str,
    actor: &Actor,
) -> Result<Vec<RowOutcome>> {
    let tag = normalise(tag)?;
    check_batch(rows, actor)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let out = rows
        .iter()
        .map(|&row| remove_one(&tx, row, &tag, actor, &at))
        .collect::<Result<_>>()?;
    tx.commit()?;
    Ok(out)
}

fn remove_one(
    conn: &Connection,
    row: i64,
    tag: &str,
    actor: &Actor,
    at: &str,
) -> Result<RowOutcome> {
    let Some((by, tagged_at)) = tag_on(conn, row, tag)? else {
        let reason = match find_row(conn, row)? {
            None => no_row(row),
            Some(_) => format!("row {row} does not carry {tag}"),
        };
        return Ok(RowOutcome::refused(row, reason));
    };
    if actor.is_agent() && by != actor.label() {
        return Ok(RowOutcome::refused(
            row,
            format!("tagged {tag} by {by}; only an analyst can remove it"),
        ));
    }
    conn.execute(
        "DELETE FROM ox.row_tag WHERE row_id = ? AND tag = ?",
        params![row, tag],
    )?;
    let before = tag_json(row, tag, &by, &tagged_at);
    record(
        conn,
        actor,
        at,
        change(row, "tag_remove", Some(before), None),
    )?;
    Ok(RowOutcome::changed(row))
}

/// Who tagged `row` with `tag`, and when; `None` when it does not carry it.
fn tag_on(conn: &Connection, row: i64, tag: &str) -> Result<Option<(String, String)>> {
    Ok(conn
        .prepare(
            "SELECT tagged_by, tagged_at FROM ox.row_tag WHERE row_id = ? AND tag = ? \
             ORDER BY tagged_at LIMIT 1",
        )?
        .query_map(params![row, tag], |r| Ok((r.get(0)?, r.get(1)?)))?
        .next()
        .transpose()?)
}

fn tagged_by(conn: &Connection, row: i64, tag: &str) -> Result<Option<String>> {
    Ok(tag_on(conn, row, tag)?.map(|(by, _)| by))
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

/// Up to `limit` of the tags the case has defined, from `offset`, by name,
/// with the rows carrying each (a tag stays defined when its last row loses
/// it). `Invalid` when `limit` is 0 or over `MAX_LIMIT`.
pub fn tags(case: &Case, limit: u32, offset: u64) -> Result<Listing<TagInfo>> {
    check_limit(limit)?;
    let fetched = case
        .conn()
        .prepare(
            "SELECT t.name, count(rt.row_id), t.created_by FROM ox.tag t \
             LEFT JOIN ox.row_tag rt ON rt.tag = t.name \
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
