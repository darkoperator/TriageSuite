//! Loading a TriageSuite run: its `duckdb/datasets.json` inventory and the
//! CSV files it names.
//!
//! The suite's own `render_sql` builds the views, in a staging schema, so a
//! run reads exactly as it does in the suite's `views.sql`; the engine then
//! copies each typed view into the case. The inventory's paths are relative
//! to the output root, and the root is taken from where the run is now, so a
//! collection that moved since it was written loads without regenerating.

use super::stage::{materialize, rebuild_relations, source_schema, HashColumn};
use super::{LoadReport, LoadedRelation};
use crate::actor::Actor;
use crate::case::Case;
use crate::catalog::catalog_dataset;
use crate::error::{EngineError, Result};
use crate::time::now_iso;
use duckdb::{params, Connection};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use triage_core::output::duckdb::inventory::{
    Dataset, DatasetFile, FileIdentity, Inventory, Status,
};
use triage_core::output::duckdb::render::render_sql;
use triage_core::output::duckdb::sql::quote_ident;
use triage_core::output::duckdb::types::SqlType;

/// The schema the suite's views are rendered into during a load.
const STAGE: &str = "ox_stage";

/// Load the run whose output root is `out_root` into `case`.
///
/// Refused when the inventory's status is not `ok`, when the run is already
/// in the case, or when the inventory declares a SQL type the suite never
/// writes. Any failure leaves the case exactly as it was.
pub fn load_run(case: &mut Case, out_root: &Path, actor: &Actor) -> Result<LoadReport> {
    let root = canonical_root(out_root)?;
    let mut inventory = read_inventory(&root)?;
    check_inventory(&inventory)?;
    inventory.out_root = root.clone();
    let manifest = match std::fs::read_to_string(root.join("run_manifest.json")) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };

    let conn = case.conn_mut();
    let mut source_id = None;
    let tx = conn.transaction()?;
    match load_in(&tx, &inventory, &root, manifest, actor, &mut source_id) {
        Ok(report) => {
            tx.commit()?;
            Ok(report)
        }
        Err(e) => {
            drop(tx);
            discard(conn, source_id);
            Err(e)
        }
    }
}

/// After a rolled-back load: put the session back on `main` and make sure
/// no schema the load created survives. The rollback has already undone
/// them; this is the belt to its braces, so its own errors are ignored.
fn discard(conn: &Connection, source_id: Option<i64>) {
    let _ = conn.execute_batch("RESET schema;");
    let _ = conn.execute_batch(&format!(
        "DROP SCHEMA IF EXISTS {} CASCADE;",
        quote_ident(STAGE)
    ));
    if let Some(id) = source_id {
        let _ = conn.execute_batch(&format!(
            "DROP SCHEMA IF EXISTS {} CASCADE;",
            quote_ident(&source_schema(id))
        ));
    }
}

/// `out_root` made absolute with every link resolved. On Windows the
/// verbatim `\\?\` prefix `canonicalize` adds is taken off again, because
/// the path ends up in `read_csv` calls and in what the analyst sees.
fn canonical_root(out_root: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(out_root).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            EngineError::NotFound(format!("no run output root at {}", out_root.display()))
        } else {
            e.into()
        }
    })?;
    let text = canonical.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return Ok(PathBuf::from(format!(r"\\{unc}")));
    }
    if let Some(local) = text.strip_prefix(r"\\?\") {
        return Ok(PathBuf::from(local));
    }
    Ok(canonical)
}

fn read_inventory(root: &Path) -> Result<Inventory> {
    let path = root.join("duckdb").join("datasets.json");
    let text = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            EngineError::NotFound(format!(
                "{} has no duckdb/datasets.json; it is not a TriageSuite run output root",
                root.display()
            ))
        } else {
            e.into()
        }
    })?;
    serde_json::from_str(&text).map_err(|e| {
        EngineError::Invalid(format!(
            "{} is not a TriageSuite inventory: {e}",
            path.display()
        ))
    })
}

/// The checks that need nothing but the inventory itself.
fn check_inventory(inventory: &Inventory) -> Result<()> {
    if inventory.status != Status::Ok {
        let status = serde_json::to_value(inventory.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| format!("{:?}", inventory.status));
        let mut message = format!(
            "run {} has status {status}: it produced nothing to load",
            inventory.run_id
        );
        if !inventory.warnings.is_empty() {
            message.push_str(&format!(" ({})", inventory.warnings.join("; ")));
        }
        return Err(EngineError::Refused(message));
    }
    // A type spelling is pasted into a TRY_CAST, so a tampered inventory is
    // untrusted SQL text: only the spellings the suite writes get through.
    for dataset in &inventory.datasets {
        for (column, effective) in &dataset.effective_types {
            if SqlType::from_sql(&effective.sql_type).is_none() {
                return Err(EngineError::Invalid(format!(
                    "{}: column {column:?} declares SQL type {:?}, which TriageSuite never writes",
                    dataset.view, effective.sql_type
                )));
            }
        }
    }
    Ok(())
}

/// Everything a load writes, inside the caller's transaction. `source_id`
/// is set as soon as the id is allocated, so the caller can clean up.
fn load_in(
    conn: &Connection,
    inventory: &Inventory,
    root: &Path,
    manifest: Option<String>,
    actor: &Actor,
    source_id: &mut Option<i64>,
) -> Result<LoadReport> {
    let present: i64 = conn.query_row(
        "SELECT count(*) FROM ox.source WHERE run_id = ? AND NOT removed",
        params![inventory.run_id],
        |r| r.get(0),
    )?;
    if present > 0 {
        return Err(EngineError::Refused(format!(
            "run {} is already in this case",
            inventory.run_id
        )));
    }

    // The rendered SQL names its views unqualified; the search path puts
    // them in the staging schema. SET is not transactional: on success it
    // is reset here; on failure the transaction is aborted and refuses even
    // a RESET, so `discard` resets it after the rollback.
    conn.execute_batch(&format!(
        "CREATE SCHEMA {stage}; SET schema = '{STAGE}';",
        stage = quote_ident(STAGE)
    ))?;
    conn.execute_batch(&render_sql(inventory))?;
    conn.execute_batch("RESET schema;")?;

    let id: i64 = conn.query_row("SELECT nextval('ox.source_seq')", [], |r| r.get(0))?;
    *source_id = Some(id);
    let loaded_at = now_iso();
    let loaded_by = actor.label();
    let path = root.display().to_string();
    conn.execute(
        "INSERT INTO ox.source \
         (id, kind, path, run_id, generation, loaded_at, loaded_by, manifest_json, removed) \
         VALUES (?, 'run', ?, ?, ?, ?, ?, ?, false)",
        params![
            id,
            path,
            inventory.run_id,
            inventory.generation,
            loaded_at,
            loaded_by,
            manifest
        ],
    )?;

    let mut relations = Vec::new();
    let mut warnings = Vec::new();
    for dataset in &inventory.datasets {
        let included: Vec<&DatasetFile> = dataset
            .files
            .iter()
            .filter(|f| f.included_in_view)
            .collect();
        if included.is_empty() {
            warnings.push(format!(
                "{} / {}: no file is included in a view; nothing loaded",
                dataset.tool, dataset.dataset_id
            ));
            continue;
        }
        relations.push(load_dataset(conn, id, dataset, &included)?);
        catalog_dataset(conn, dataset)?;
    }

    record_inventory(conn, id, inventory)?;

    let after = serde_json::json!({
        "kind": "run",
        "path": path,
        "run_id": inventory.run_id,
        "generation": inventory.generation,
        "relations": relations.iter().map(|r| serde_json::json!({
            "name": r.name, "hosts": r.hosts, "rows": r.rows,
        })).collect::<Vec<_>>(),
    });
    conn.execute(
        "INSERT INTO ox.history \
         (id, \"at\", \"by\", target_kind, target_id, action, before_json, after_json) \
         VALUES (nextval('ox.history_seq'), ?, ?, 'source', ?, 'source_load', NULL, ?)",
        params![loaded_at, loaded_by, id.to_string(), after.to_string()],
    )?;

    rebuild_relations(conn)?;
    conn.execute_batch(&format!("DROP SCHEMA {} CASCADE;", quote_ident(STAGE)))?;

    Ok(LoadReport {
        source_id: id,
        relations,
        superseded: Vec::new(),
        warnings,
    })
}

/// Materialize one dataset's typed view and record its table, its hosts and
/// its relation.
fn load_dataset(
    conn: &Connection,
    source_id: i64,
    dataset: &Dataset,
    included: &[&DatasetFile],
) -> Result<LoadedRelation> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT kind FROM ox.relation WHERE name = ?",
            params![dataset.view],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            duckdb::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    if let Some(kind) = existing.filter(|k| k != "dataset") {
        return Err(EngineError::Refused(format!(
            "the case already has a {kind} relation named {}",
            dataset.view
        )));
    }

    let meta = &dataset.metadata_columns;
    // Host and identity, then every column by name, each read as its text.
    // The run id and the output file are left out: they differ between two
    // runs that read the same event.
    let mut columns: Vec<&String> = dataset.columns.iter().collect();
    columns.sort();
    let hash_columns: Vec<HashColumn> = [&meta.host, &meta.identity]
        .into_iter()
        .chain(columns)
        .map(|name| HashColumn {
            name: name.clone(),
            text_expr: quote_ident(
                dataset
                    .effective_types
                    .get(name)
                    .map_or(name, |t| &t.text_column),
            ),
        })
        .collect();
    let staged = format!("{}.{}", quote_ident(STAGE), quote_ident(&dataset.view));
    let (rows, first, last) = materialize(conn, source_id, &dataset.view, &staged, &hash_columns)?;
    let (first, last) = if rows == 0 {
        (None, None)
    } else {
        (Some(first), Some(last))
    };
    let schema = source_schema(source_id);
    conn.execute(
        "INSERT INTO ox.source_table \
         (source_id, relation, table_schema, first_row, last_row, rows) \
         VALUES (?, ?, ?, ?, ?, ?)",
        params![
            source_id,
            dataset.view,
            schema,
            first,
            last,
            i64::try_from(rows).unwrap_or(i64::MAX)
        ],
    )?;

    let hosts: BTreeSet<&str> = included.iter().map(|f| f.host.as_str()).collect();
    let count_sql = format!(
        "SELECT count(*) FROM {}.{} WHERE {} = ?",
        quote_ident(&schema),
        quote_ident(&dataset.view),
        quote_ident(&meta.host)
    );
    for host in &hosts {
        let host_rows: i64 = conn.query_row(&count_sql, params![host], |r| r.get(0))?;
        conn.execute(
            "INSERT INTO ox.source_dataset (source_id, relation, host, rows, active) \
             VALUES (?, ?, ?, ?, true)",
            params![source_id, dataset.view, host, host_rows],
        )?;
    }

    conn.execute(
        "INSERT INTO ox.relation \
         (name, kind, tool, primary_time, evidence_path_column, host_column) \
         VALUES (?, 'dataset', ?, NULL, ?, ?) ON CONFLICT DO NOTHING",
        params![
            dataset.view,
            dataset.tool,
            dataset.evidence_path_column,
            meta.host
        ],
    )?;

    Ok(LoadedRelation {
        name: dataset.view.clone(),
        hosts: hosts.into_iter().map(str::to_string).collect(),
        rows,
    })
}

/// Copy the inventory into `ox.inventory_dataset`, `ox.inventory_file` and
/// `ox.inventory_note`, so the case still says what the run held after the
/// run itself is gone.
fn record_inventory(conn: &Connection, source_id: i64, inventory: &Inventory) -> Result<()> {
    for dataset in &inventory.datasets {
        conn.execute(
            "INSERT INTO ox.inventory_dataset \
             (source_id, view, raw_view, tool, dataset_id, source, evidence_path_column, \
              metadata_columns_json, columns_json, effective_types_json) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                source_id,
                dataset.view,
                dataset.raw_view,
                dataset.tool,
                dataset.dataset_id,
                dataset.source,
                dataset.evidence_path_column,
                to_json(&dataset.metadata_columns)?,
                to_json(&dataset.columns)?,
                to_json(&dataset.effective_types)?
            ],
        )?;
        for file in &dataset.files {
            let identity = file.identity.as_ref().map(|i| match i {
                FileIdentity::User { name } => format!("user:{name}"),
                FileIdentity::System => "system".to_string(),
                FileIdentity::Unknown => "unknown".to_string(),
            });
            let role = serde_json::to_value(file.role)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| format!("{:?}", file.role));
            conn.execute(
                "INSERT INTO ox.inventory_file \
                 (source_id, view, path, host, identity, role, included_in_view, \
                  derived_into, header_json, warnings_json) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    source_id,
                    dataset.view,
                    file.path.to_string_lossy(),
                    file.host,
                    identity,
                    role,
                    file.included_in_view,
                    file.derived_into
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    file.header.as_ref().map(to_json).transpose()?,
                    to_json(&file.warnings)?
                ],
            )?;
        }
    }

    let note = "INSERT INTO ox.inventory_note (source_id, kind, view, dataset_id, detail_json) \
                VALUES (?, ?, ?, ?, ?)";
    for warning in &inventory.warnings {
        conn.execute(
            note,
            params![
                source_id,
                "warning",
                None::<String>,
                None::<String>,
                to_json(warning)?
            ],
        )?;
    }
    for dropped in &inventory.dropped_overrides {
        conn.execute(
            note,
            params![
                source_id,
                "dropped_override",
                dropped.view,
                dropped.dataset_id,
                to_json(dropped)?
            ],
        )?;
    }
    for only in &inventory.inventory_only {
        conn.execute(
            note,
            params![
                source_id,
                "inventory_only",
                None::<String>,
                only.dataset_id,
                to_json(only)?
            ],
        )?;
    }
    Ok(())
}

fn to_json<T: serde::Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|e| EngineError::Invalid(format!("the inventory does not serialize: {e}")))
}
