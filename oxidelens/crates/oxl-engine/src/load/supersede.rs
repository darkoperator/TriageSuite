//! More than one run of a host: the newer run's rows replace the older
//! run's, relation by relation and host by host, and the analyst's
//! annotations follow the rows that are the same in both.
//!
//! A row is "the same" when its `_ox_hash` and `_ox_occ` match: the hash is
//! blind to the run id and the output path, and the occurrence tells two
//! byte-identical rows apart. An annotated row with no such counterpart
//! keeps its annotations where they are and is reported as left behind; its
//! old row stays in the case, just no longer in the view.
//!
//! Removing the newer run undoes it: the run it superseded is reinstated
//! and the annotations move back the same way.
//!
//! Only runs supersede and are superseded. A loose file feeds a relation of
//! its own and is left alone.

use super::stage::{host_column, source_schema};
use super::Superseded;
use crate::actor::Actor;
use crate::annot::{id_list, write_rows, IDS_PER_STATEMENT};
use crate::error::{EngineError, Result};
use duckdb::types::Value as SqlValue;
use duckdb::{params, Connection};
use std::collections::{BTreeSet, HashMap};
use triage_core::output::duckdb::sql::quote_ident;

/// The annotation tables; each points at a loaded row by its `row_id`.
const ANNOTATIONS: [&str; 3] = ["ox.row_mark", "ox.row_tag", "ox.note"];

/// Supersede, for every relation and host `new_source` (a run just loaded,
/// inside the load's transaction) is active for, every other live run active
/// for the same pair: its `ox.source_dataset` row becomes inactive and its
/// annotations are repointed at the new rows. One `supersede` history row
/// per superseded pair. Returns them sorted by relation and host.
pub(crate) fn supersede(
    conn: &Connection,
    new_source: i64,
    at: &str,
    by: &Actor,
) -> Result<Vec<Superseded>> {
    let kind: String = conn.query_row(
        "SELECT kind FROM ox.source WHERE id = ?",
        params![new_source],
        |r| r.get(0),
    )?;
    if kind != "run" {
        return Ok(Vec::new());
    }
    let fed: Vec<(String, String)> = conn
        .prepare(
            "SELECT relation, host FROM ox.source_dataset \
             WHERE source_id = ? AND active AND host IS NOT NULL ORDER BY relation, host",
        )?
        .query_map(params![new_source], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;

    let mut superseded = Vec::new();
    for (relation, host) in fed {
        let older: Vec<i64> = conn
            .prepare(
                "SELECT sd.source_id FROM ox.source_dataset sd \
                 JOIN ox.source s ON s.id = sd.source_id \
                 WHERE sd.relation = ? AND sd.host = ? AND sd.active \
                   AND sd.source_id <> ? AND s.kind = 'run' AND NOT s.removed \
                 ORDER BY sd.source_id",
            )?
            .query_map(params![relation, host, new_source], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        for old_source in older {
            conn.execute(
                "UPDATE ox.source_dataset SET active = false \
                 WHERE source_id = ? AND relation = ? AND host = ?",
                params![old_source, relation, host],
            )?;
            let rebound = rebind(conn, old_source, new_source, &relation, &host)?;
            let before = serde_json::json!({
                "source_id": old_source, "relation": relation, "host": host, "active": true,
            });
            let mut after = serde_json::json!({
                "source_id": old_source, "relation": relation, "host": host, "active": false,
                "superseded_by": new_source,
            });
            if let Some(after) = after.as_object_mut() {
                after.extend(rebound.history_fields());
            }
            conn.execute(
                "INSERT INTO ox.history \
                 (id, \"at\", \"by\", target_kind, target_id, action, before_json, after_json) \
                 VALUES (nextval('ox.history_seq'), ?, ?, 'source', ?, 'supersede', ?, ?)",
                params![
                    at,
                    by.label(),
                    old_source.to_string(),
                    before.to_string(),
                    after.to_string()
                ],
            )?;
            superseded.push(Superseded {
                relation: relation.clone(),
                host: host.clone(),
                old_source,
                moved: rebound.moved,
                left_behind: rebound.left_behind,
            });
        }
    }
    Ok(superseded)
}

/// A run brought back into a relation's view for a host by the removal of
/// the run that had superseded it.
pub(super) struct Reinstated {
    pub relation: String,
    pub host: String,
    pub source: i64,
    /// What moved back and what was left on the removed run's rows
    /// (orphaned), as `rebind` reports it.
    pub rebound: Rebound,
}

impl Reinstated {
    /// This reinstatement's entry in the `source_remove` history row.
    pub(super) fn history_json(&self) -> serde_json::Value {
        let mut e = serde_json::json!({
            "relation": self.relation, "host": self.host, "source_id": self.source,
        });
        if let Some(e) = e.as_object_mut() {
            e.extend(self.rebound.history_fields());
        }
        e
    }
}

/// Supersession undone, inside a removal's transaction and before
/// `removed_source`'s schema is dropped: for every relation and host it is
/// the active run for, the most recently loaded live run with an inactive
/// row for that pair becomes active again, and annotations move back to its
/// identical rows.
pub(super) fn reinstate(conn: &Connection, removed_source: i64) -> Result<Vec<Reinstated>> {
    let fed: Vec<(String, String)> = conn
        .prepare(
            "SELECT relation, host FROM ox.source_dataset \
             WHERE source_id = ? AND active AND host IS NOT NULL ORDER BY relation, host",
        )?
        .query_map(params![removed_source], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut reinstated = Vec::new();
    for (relation, host) in fed {
        let previous: Option<i64> = conn
            .query_row(
                "SELECT sd.source_id FROM ox.source_dataset sd \
                 JOIN ox.source s ON s.id = sd.source_id \
                 WHERE sd.relation = ? AND sd.host = ? AND NOT sd.active \
                   AND sd.source_id <> ? AND s.kind = 'run' AND NOT s.removed \
                 ORDER BY s.loaded_at DESC, s.id DESC LIMIT 1",
                params![relation, host, removed_source],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                duckdb::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        let Some(source) = previous else {
            continue;
        };
        conn.execute(
            "UPDATE ox.source_dataset SET active = true \
             WHERE source_id = ? AND relation = ? AND host = ?",
            params![source, relation, host],
        )?;
        let rebound = rebind(conn, removed_source, source, &relation, &host)?;
        reinstated.push(Reinstated {
            relation,
            host,
            source,
            rebound,
        });
    }
    Ok(reinstated)
}

/// What `rebind` did, for the caller's report and its history row.
pub(super) struct Rebound {
    /// Annotations that moved, counted per record: each mark, each tag row
    /// and each note that is not deleted.
    pub moved: u64,
    /// Annotations that stayed on the old rows, counted the same way.
    pub left_behind: u64,
    /// Every annotation that moved, deleted notes included, as
    /// `{kind, from_row, to_row}` plus `tag` for a tag and `note_id` and
    /// `deleted` for a note; `kind` is `row_mark`, `row_tag` or `note`.
    pub moves: Vec<serde_json::Value>,
    /// Every annotation that stayed, in the same shape without `to_row`.
    pub stayed: Vec<serde_json::Value>,
}

impl Rebound {
    /// The history fields: the lists under `moved` and `left_behind`, so a
    /// row's history reaches the annotations that left it, and the counts
    /// beside them.
    fn history_fields(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut fields = serde_json::Map::new();
        fields.insert("moved".into(), self.moves.clone().into());
        fields.insert("left_behind".into(), self.stayed.clone().into());
        fields.insert("moved_count".into(), self.moved.into());
        fields.insert("left_behind_count".into(), self.left_behind.into());
        fields
    }
}

/// One annotation `rebind` moved (with `to_row`) or left (without).
fn entry(kind: &str, from_row: i64, to_row: Option<i64>) -> serde_json::Value {
    let mut e = serde_json::json!({"kind": kind, "from_row": from_row});
    if let Some(to_row) = to_row {
        e["to_row"] = to_row.into();
    }
    e
}

fn tag_entry(tag: &str, from_row: i64, to_row: Option<i64>) -> serde_json::Value {
    let mut e = entry("row_tag", from_row, to_row);
    e["tag"] = tag.into();
    e
}

fn note_entry(note: (i64, bool), from_row: i64, to_row: Option<i64>) -> serde_json::Value {
    let mut e = entry("note", from_row, to_row);
    e["note_id"] = note.0.into();
    e["deleted"] = note.1.into();
    e
}

/// Repoint every mark, tag and note on an annotated row of `from_source`'s
/// `relation` for `host` at the row of `to_source` with the same hash and
/// occurrence. Supersession moves them forward to the new run; removing
/// that run moves them back to the run it had superseded.
///
/// Counted per annotation record (see `Rebound`); a deleted note moves with
/// its row and is listed, but is not counted. An annotation stays where it
/// is, and is left behind, when its row has no counterpart, or when the
/// counterpart already has a mark (for a mark) or the same tag (for a tag).
///
/// The annotations are read and written set-wise: the pairs, then the
/// marks, tags and notes of every row involved, a statement each (per
/// `IDS_PER_STATEMENT` rows), then one write per kind (per
/// `write_rows` chunk). So the cost does not grow by a statement per
/// annotated row. The pairs map each old row to at most one new row, in
/// another table, so what was read up front stays true while deciding.
pub(super) fn rebind(
    conn: &Connection,
    from_source: i64,
    to_source: i64,
    relation: &str,
    host: &str,
) -> Result<Rebound> {
    let host_column = quote_ident(&host_column(conn, relation)?.ok_or_else(|| {
        // A host's rows came from that host column; without one the case
        // contradicts itself.
        EngineError::Internal(format!(
            "{relation} has rows for host {host} but no host column"
        ))
    })?);
    let table = |source: i64| {
        format!(
            "{}.{}",
            quote_ident(&source_schema(source)),
            quote_ident(relation)
        )
    };
    let annotated = ANNOTATIONS
        .map(|t| format!("SELECT row_id FROM {t}"))
        .join(" UNION ");
    let pairs: Vec<(i64, Option<i64>)> = conn
        .prepare(&format!(
            "SELECT o._ox_row, n._ox_row FROM {from} o \
             LEFT JOIN {to} n ON n._ox_hash = o._ox_hash AND n._ox_occ = o._ox_occ \
                             AND n.{host_column} = ? \
             WHERE o.{host_column} = ? AND o._ox_row IN ({annotated}) \
             ORDER BY o._ox_row",
            from = table(from_source),
            to = table(to_source),
        ))?
        .query_map(params![host, host], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;

    let mut out = Rebound {
        moved: 0,
        left_behind: 0,
        moves: Vec::new(),
        stayed: Vec::new(),
    };
    if pairs.is_empty() {
        return Ok(out);
    }

    // What every row involved carries, read a few statements at a time.
    let old_rows: Vec<i64> = pairs.iter().map(|(old, _)| *old).collect();
    let mut rows = old_rows.clone();
    rows.extend(pairs.iter().filter_map(|(_, new)| *new));
    let mut marks: HashMap<i64, u64> = HashMap::new();
    for (row, n) in read_by_row(
        conn,
        "SELECT row_id, count(*) FROM ox.row_mark WHERE row_id IN ({ids}) GROUP BY row_id",
        &rows,
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
    )? {
        marks.insert(row, u64::try_from(n).unwrap_or_default());
    }
    let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
    for (row, tag) in read_by_row(
        conn,
        "SELECT row_id, tag FROM ox.row_tag WHERE row_id IN ({ids}) ORDER BY row_id, tag",
        &rows,
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )? {
        tags.entry(row).or_default().push(tag);
    }
    // `(id, deleted)` of every note on an old row, deleted ones included.
    let mut notes: HashMap<i64, Vec<(i64, bool)>> = HashMap::new();
    for (row, id, deleted) in read_by_row(
        conn,
        "SELECT row_id, id, deleted FROM ox.note WHERE row_id IN ({ids}) ORDER BY row_id, id",
        &old_rows,
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, bool>(2)?,
            ))
        },
    )? {
        notes.entry(row).or_default().push((id, deleted));
    }

    let none = Vec::new();
    let mut mark_moves: Vec<Vec<SqlValue>> = Vec::new();
    let mut tag_moves: BTreeSet<(i64, String, i64)> = BTreeSet::new();
    let mut note_moves: Vec<Vec<SqlValue>> = Vec::new();
    for (old_row, new_row) in pairs {
        let n_marks = marks.get(&old_row).copied().unwrap_or(0);
        let old_tags = tags.get(&old_row).unwrap_or(&none);
        let old_notes = notes.get(&old_row).map_or(&[][..], Vec::as_slice);
        let live = len(old_notes.iter().filter(|(_, deleted)| !deleted));
        let Some(new_row) = new_row else {
            out.left_behind += n_marks + len(old_tags.iter()) + live;
            if n_marks > 0 {
                out.stayed.push(entry("row_mark", old_row, None));
            }
            out.stayed
                .extend(old_tags.iter().map(|t| tag_entry(t, old_row, None)));
            out.stayed
                .extend(old_notes.iter().map(|n| note_entry(*n, old_row, None)));
            continue;
        };
        // A row carries at most one mark: one the target already has (an
        // earlier run's, left behind when a later run lacked the row)
        // stands, and the incoming one stays where it is.
        if n_marks > 0 {
            if marks.get(&new_row).copied().unwrap_or(0) > 0 {
                out.left_behind += n_marks;
                out.stayed.push(entry("row_mark", old_row, None));
            } else {
                mark_moves.push(vec![SqlValue::BigInt(old_row), SqlValue::BigInt(new_row)]);
                out.moved += n_marks;
                out.moves.push(entry("row_mark", old_row, Some(new_row)));
            }
        }
        // Likewise a tag the target already carries is not doubled.
        let present = tags.get(&new_row).unwrap_or(&none);
        for tag in old_tags {
            if present.contains(tag) {
                out.left_behind += 1;
                out.stayed.push(tag_entry(tag, old_row, None));
            } else {
                tag_moves.insert((old_row, tag.clone(), new_row));
                out.moved += 1;
                out.moves.push(tag_entry(tag, old_row, Some(new_row)));
            }
        }
        // Notes always move, deleted ones too; only live ones are counted.
        if !old_notes.is_empty() {
            note_moves.push(vec![SqlValue::BigInt(old_row), SqlValue::BigInt(new_row)]);
        }
        out.moved += live;
        out.moves.extend(
            old_notes
                .iter()
                .map(|n| note_entry(*n, old_row, Some(new_row))),
        );
    }

    const PAIR: &str = "(CAST(? AS BIGINT), CAST(? AS BIGINT))";
    write_rows(
        conn,
        "UPDATE ox.row_mark AS m SET row_id = v.to_row \
         FROM (VALUES {values}) AS v(from_row, to_row) WHERE m.row_id = v.from_row",
        PAIR,
        &mark_moves,
    )?;
    let tag_moves: Vec<Vec<SqlValue>> = tag_moves
        .into_iter()
        .map(|(old, tag, new)| {
            vec![
                SqlValue::BigInt(old),
                SqlValue::Text(tag),
                SqlValue::BigInt(new),
            ]
        })
        .collect();
    write_rows(
        conn,
        "UPDATE ox.row_tag AS t SET row_id = v.to_row \
         FROM (VALUES {values}) AS v(from_row, tag, to_row) \
         WHERE t.row_id = v.from_row AND t.tag = v.tag",
        "(CAST(? AS BIGINT), CAST(? AS VARCHAR), CAST(? AS BIGINT))",
        &tag_moves,
    )?;
    write_rows(
        conn,
        "UPDATE ox.note AS n SET row_id = v.to_row \
         FROM (VALUES {values}) AS v(from_row, to_row) WHERE n.row_id = v.from_row",
        PAIR,
        &note_moves,
    )?;
    Ok(out)
}

/// What `sql` returns over `ids`, read `IDS_PER_STATEMENT` ids per
/// statement: its `{ids}` becomes each chunk as a SQL list. No statement
/// runs for no ids.
fn read_by_row<T>(
    conn: &Connection,
    sql: &str,
    ids: &[i64],
    row: impl Fn(&duckdb::Row<'_>) -> duckdb::Result<T>,
) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for chunk in ids.chunks(IDS_PER_STATEMENT) {
        let mut stmt = conn.prepare(&sql.replace("{ids}", &id_list(chunk)))?;
        let rows = stmt.query_map([], &row)?;
        for r in rows {
            out.push(r?);
        }
    }
    Ok(out)
}

fn len<T>(items: impl Iterator<Item = T>) -> u64 {
    u64::try_from(items.count()).unwrap_or(u64::MAX)
}
