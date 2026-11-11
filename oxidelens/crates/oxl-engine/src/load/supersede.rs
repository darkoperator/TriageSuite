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
//! Only runs supersede and are superseded. A loose file feeds a relation of
//! its own and is left alone.

use super::stage::{host_column_of, source_schema};
use super::Superseded;
use crate::actor::Actor;
use crate::error::{EngineError, Result};
use duckdb::{params, Connection};
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
            let (moved, left_behind) = rebind(conn, old_source, new_source, &relation, &host)?;
            let before = serde_json::json!({
                "source_id": old_source, "relation": relation, "host": host, "active": true,
            });
            let after = serde_json::json!({
                "source_id": old_source, "relation": relation, "host": host, "active": false,
                "superseded_by": new_source, "moved": moved, "left_behind": left_behind,
            });
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
                moved,
                left_behind,
            });
        }
    }
    Ok(superseded)
}

/// Repoint every mark, tag and note on an annotated row of `old_source`'s
/// `relation` for `host` at the row of `new_source` with the same hash and
/// occurrence. Returns how many annotated rows moved and how many had no
/// counterpart.
fn rebind(
    conn: &Connection,
    old_source: i64,
    new_source: i64,
    relation: &str,
    host: &str,
) -> Result<(u64, u64)> {
    let host_column = |source: i64| -> Result<String> {
        host_column_of(conn, source, relation)?.ok_or_else(|| {
            EngineError::Invalid(format!(
                "source {source} has rows of {relation} for host {host} but no host column"
            ))
        })
    };
    let old_host = host_column(old_source)?;
    let new_host = host_column(new_source)?;
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
            "SELECT o._ox_row, n._ox_row FROM {old} o \
             LEFT JOIN {new} n ON n._ox_hash = o._ox_hash AND n._ox_occ = o._ox_occ \
                              AND n.{new_host} = ? \
             WHERE o.{old_host} = ? AND o._ox_row IN ({annotated}) \
             ORDER BY o._ox_row",
            old = table(old_source),
            new = table(new_source),
            new_host = quote_ident(&new_host),
            old_host = quote_ident(&old_host),
        ))?
        .query_map(params![host, host], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;

    let (mut moved, mut left_behind) = (0u64, 0u64);
    for (old_row, new_row) in pairs {
        let Some(new_row) = new_row else {
            left_behind += 1;
            continue;
        };
        for t in ANNOTATIONS {
            conn.execute(
                &format!("UPDATE {t} SET row_id = ? WHERE row_id = ?"),
                params![new_row, old_row],
            )?;
        }
        moved += 1;
    }
    Ok((moved, left_behind))
}
