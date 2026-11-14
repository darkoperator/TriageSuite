//! Loading a TriageSuite run: its `duckdb/datasets.json` inventory and the
//! CSV files it names.
//!
//! The suite's own `render_sql` builds the views, in a staging schema, so a
//! run reads exactly as it does in the suite's `views.sql`; the engine then
//! copies each typed view into the case. The inventory's paths are relative
//! to the output root, and the root is taken from where the run is now, so a
//! collection that moved since it was written loads without regenerating.

use super::sessions::{read_sessions, store_sessions, RunSessions};
use super::stage::{materialize, rebuild_relations, source_schema, HashColumn};
use super::supersede::supersede;
use super::{fold_name, LoadReport, LoadThreads, LoadedRelation, RunOptions, SkippedDataset};
use crate::actor::Actor;
use crate::case::{Case, FORMAT_2};
use crate::catalog::{catalog_dataset, check_sql_type, check_typing, column_names};
use crate::error::{EngineError, Result};
use crate::time::now_iso;
use duckdb::{params, Connection};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use triage_core::output::duckdb::inventory::{
    Dataset, DatasetFile, FileIdentity, Inventory, MetadataColumns, Status,
};
use triage_core::output::duckdb::render::render_sql;
use triage_core::output::duckdb::sql::{quote_ident, quote_literal};

/// The schema the suite's views are rendered into during a load.
const STAGE: &str = "ox_stage";

#[cfg(test)]
thread_local! {
    /// Set by a unit test to make `load_in` fail right after `supersede`
    /// has moved annotations, the one place a failure cannot be had from
    /// outside. Compiled only under `cfg(test)`.
    static FAIL_AFTER_SUPERSEDE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Load the run whose output root is `out_root` into `case`.
///
/// Refused when the run's output, by its generation, is already in the case
/// (a removed run may be loaded again; another generation of the same run
/// id loads, and supersedes as a later run does) or a dataset's view or its
/// `raw_` view would take a name another relation uses (see
/// `check_relation_names`, which comes before every other check of a
/// dataset); `Invalid` when the
/// inventory's status is not `ok`, when the inventory
/// declares a SQL type the suite never writes, types a column differently
/// from the case's catalog, or names a relation's metadata columns
/// differently from the live sources already feeding it. For each relation and host the run has rows for,
/// an earlier run's rows are superseded (see `supersede`). Any failure leaves
/// the case exactly as it was. EvtxTriage's per-channel copies are left out
/// (see `RunOptions`).
///
/// The run's Timeline Explorer sessions (`Sessions/*.tle_sess` in each
/// host's `Processed-*` folder or at the root; same-named ones merged) are
/// kept with it: at most `MAX_SESSION_FILES` files across those folders,
/// each at most `MAX_SESSION_BYTES`. The rest, and any that is not a
/// session, are left out and named (by path below the run root) under
/// `sessions_skipped` in the load's history row; a session file never fails
/// a load.
pub fn load_run(case: &mut Case, out_root: &Path, actor: &Actor) -> Result<LoadReport> {
    load_run_with(case, out_root, RunOptions::default(), actor)
}

/// `load_run`, with `options`.
pub fn load_run_with(
    case: &mut Case,
    out_root: &Path,
    options: RunOptions,
    actor: &Actor,
) -> Result<LoadReport> {
    let root = canonical_path(out_root)?;
    let mut inventory = read_inventory(&root)?;
    check_inventory(&inventory)?;
    inventory.out_root = root.clone();
    let manifest = match std::fs::read_to_string(root.join("run_manifest.json")) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };

    let skipped = copies(&inventory, options);
    // Read before the transaction: files, not the case. A format-1 case
    // (only the upgrade tests load into one) has nowhere to keep them.
    let sessions = (case.format() >= FORMAT_2).then(|| read_sessions(&root));
    let load = Load {
        inventory: &inventory,
        skipped,
        root: &root,
        actor,
        sessions: sessions.as_ref(),
    };
    let conn = case.conn_mut();
    let mut source_id = None;
    let tx = conn.transaction()?;
    // Within the transaction, so a failure to bound rolls back nothing.
    let threads = LoadThreads::bound(&tx)?;
    match load_in(&tx, &load, manifest, &mut source_id) {
        Ok(report) => {
            // Before the commit: a failure here rolls the load back.
            threads.restore(&tx)?;
            tx.commit()?;
            Ok(report)
        }
        Err(e) => {
            drop(tx);
            discard(conn, source_id);
            let _ = threads.restore(conn);
            Err(e)
        }
    }
}

/// After a rolled-back load: put the session back on `main` and make sure
/// no schema the load created survives. The rollback has already undone
/// them; this is the belt to its braces, so its own errors are ignored.
pub(super) fn discard(conn: &Connection, source_id: Option<i64>) {
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
/// the path is what the analyst sees and what the case records. A read goes
/// through `read_path`, which puts the prefix back where it is needed.
pub(crate) fn canonical_path(out_root: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(out_root).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            // Both loaders come through here: a run's root, or a file.
            EngineError::NotFound(format!("no file or folder at {}", out_root.display()))
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

/// `path`, a canonical path's text, as DuckDB is told to read it.
///
/// DuckDB opens files through the classic Windows API, which refuses a path
/// past `WINDOWS_PATH_LIMIT` however real the file is ("No files found").
/// Such a path is handed over in its verbatim `\\?\` form, which has no
/// limit. Every other path, and every path on another system, is unchanged,
/// and so is everything the analyst sees: only the read uses this text.
pub(super) fn read_path(path: &str) -> String {
    if cfg!(windows) {
        verbatim_when_long(path)
    } else {
        path.to_string()
    }
}

/// The classic Windows API takes 260 characters with the terminator, and 248
/// for a directory; the lower figure leaves room for what DuckDB appends.
const WINDOWS_PATH_LIMIT: usize = 248;

/// The Windows rule behind `read_path`, as text so it is tested on every
/// system.
///
/// A path that is already verbatim (or a device path) is left alone, and so
/// is one that is not absolute, which has no verbatim form. The verbatim
/// form takes `/` as a character of a name, so each becomes a separator.
fn verbatim_when_long(path: &str) -> String {
    if path.encode_utf16().count() < WINDOWS_PATH_LIMIT
        || path.starts_with(r"\\?\")
        || path.starts_with(r"\\.\")
    {
        return path.to_string();
    }
    let separated = path.replace('/', r"\");
    if let Some(share) = separated.strip_prefix(r"\\") {
        return format!(r"\\?\UNC\{share}");
    }
    let drive =
        matches!(separated.as_bytes(), [letter, b':', b'\\', ..] if letter.is_ascii_alphabetic());
    if drive {
        format!(r"\\?\{separated}")
    } else {
        path.to_string()
    }
}

/// The suite's own view SQL for `inventory`, with every file whose path is
/// past the Windows limit read through `read_path`. Only the path inside
/// `read_csv([...])` changes; the `_triage_output_file` literal beside it
/// stays the ordinary path, which is what the analyst is shown.
fn view_sql(inventory: &Inventory) -> String {
    let mut sql = render_sql(inventory);
    let included = inventory
        .datasets
        .iter()
        .flat_map(|d| &d.files)
        .filter(|f| f.included_in_view);
    for file in included {
        // The path exactly as `render_sql` wrote it.
        let written = inventory.out_root.join(&file.path).display().to_string();
        let read = read_path(&written);
        if read != written {
            sql = sql.replace(
                &format!("read_csv([{}]", quote_literal(&written)),
                &format!("read_csv([{}]", quote_literal(&read)),
            );
        }
    }
    sql
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
        // A property of the run named, not of the actor or the case.
        return Err(EngineError::Invalid(message));
    }
    // A type spelling is pasted into a TRY_CAST, so a tampered inventory is
    // untrusted SQL text: only the spellings the suite writes get through.
    for dataset in &inventory.datasets {
        for (column, effective) in &dataset.effective_types {
            check_sql_type(&dataset.view, column, &effective.sql_type)?;
        }
    }
    Ok(())
}

/// What a load with `options` leaves out of `inventory`: EvtxTriage's
/// per-channel copies (`Individual/<channel>`), unless asked for, host by
/// host. A per-channel file is a copy only when the aggregate (`events`)
/// holds an included file for the same host; for any other host it holds
/// that host's only event rows, and loads.
fn copies(inventory: &Inventory, options: RunOptions) -> Vec<SkippedDataset> {
    const TOOL: &str = "EvtxTriage";
    if options.per_channel {
        return Vec::new();
    }
    let covered: BTreeSet<&str> = inventory
        .datasets
        .iter()
        .filter(|d| d.tool == TOOL && d.dataset_id == "events")
        .flat_map(|d| &d.files)
        .filter(|f| f.included_in_view)
        .map(|f| f.host.as_str())
        .collect();
    inventory
        .datasets
        .iter()
        .filter(|d| d.tool == TOOL && d.dataset_id.starts_with("Individual/"))
        .filter_map(|d| {
            let hosts: BTreeSet<&str> = d
                .files
                .iter()
                .filter(|f| f.included_in_view && covered.contains(f.host.as_str()))
                .map(|f| f.host.as_str())
                .collect();
            (!hosts.is_empty()).then(|| SkippedDataset {
                relation: d.view.clone(),
                tool: d.tool.clone(),
                dataset_id: d.dataset_id.clone(),
                hosts: hosts.into_iter().map(str::to_string).collect(),
            })
        })
        .collect()
}

/// What one load works from.
struct Load<'a> {
    inventory: &'a Inventory,
    /// What of the inventory not to load.
    skipped: Vec<SkippedDataset>,
    root: &'a Path,
    actor: &'a Actor,
    /// The run's session files; `None` for a case that keeps none.
    sessions: Option<&'a RunSessions>,
}

impl Load<'_> {
    /// What this load leaves out of `dataset`, if anything. Matched on the
    /// dataset's tool, id and view together, so another dataset that happens
    /// to share one of them is untouched.
    fn skipped_of(&self, dataset: &Dataset) -> Option<&SkippedDataset> {
        self.skipped.iter().find(|s| {
            s.tool == dataset.tool
                && s.dataset_id == dataset.dataset_id
                && s.relation == dataset.view
        })
    }

    /// The inventory as this load reads it: a left-out file is no longer
    /// included, and a dataset left with no included file is dropped. The
    /// suite's views are rendered from this, so a left-out file is not even
    /// read.
    fn loaded(&self) -> Inventory {
        let mut inventory = self.inventory.clone();
        inventory.datasets.retain_mut(|d| {
            let Some(skipped) = self.skipped_of(d) else {
                return true;
            };
            let hosts = skipped.hosts.clone();
            for file in &mut d.files {
                if hosts.contains(&file.host) {
                    file.included_in_view = false;
                }
            }
            loads(d)
        });
        inventory
    }
}

/// Everything a load writes, inside the caller's transaction. `source_id`
/// is set as soon as the id is allocated, so the caller can clean up.
fn load_in(
    conn: &Connection,
    load: &Load<'_>,
    manifest: Option<String>,
    source_id: &mut Option<i64>,
) -> Result<LoadReport> {
    let (full, root, actor) = (load.inventory, load.root, load.actor);
    let inventory = &load.loaded();
    // One writing of a run's output is named by its generation, which the
    // suite makes new each time and derives from the run id. The run id
    // alone is a stamp with no host in it, so two collections processed in
    // the same millisecond share one. A run recorded without a generation,
    // or one that has none, is matched on its run id instead.
    let generation = Some(inventory.generation.as_str()).filter(|g| !g.is_empty());
    let present: i64 = conn.query_row(
        "SELECT count(*) FROM ox.source WHERE kind = 'run' AND NOT removed \
         AND (generation = ? \
              OR ((generation IS NULL OR CAST(? AS VARCHAR) IS NULL) AND run_id = ?))",
        params![generation, generation, inventory.run_id],
        |r| r.get(0),
    )?;
    if present > 0 {
        return Err(EngineError::Refused(format!(
            "run {} (generation {}) is already in this case",
            inventory.run_id, inventory.generation
        )));
    }
    // The name first: a dataset taking another relation's name is refused
    // for that, before its columns are compared with a relation that is not
    // its own.
    for dataset in inventory.datasets.iter().filter(|d| loads(d)) {
        check_relation_names(conn, &dataset.view)?;
        check_metadata_columns(conn, dataset)?;
        check_typing(conn, dataset)?;
    }

    // The rendered SQL names its views unqualified; the search path puts
    // them in the staging schema. SET is not transactional: on success it
    // is reset here; on failure the transaction is aborted and refuses even
    // a RESET, so `discard` resets it after the rollback.
    conn.execute_batch(&format!(
        "CREATE SCHEMA {stage}; SET schema = '{STAGE}';",
        stage = quote_ident(STAGE)
    ))?;
    conn.execute_batch(&view_sql(inventory))?;
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

    // The whole inventory: the case says what the run held, loaded or not.
    record_inventory(conn, id, full)?;
    if let Some(sessions) = load.sessions {
        store_sessions(conn, id, sessions)?;
    }

    let mut after = serde_json::json!({
        "kind": "run",
        "path": path,
        "run_id": inventory.run_id,
        "generation": inventory.generation,
        "relations": relations.iter().map(|r| serde_json::json!({
            "name": r.name, "hosts": r.hosts, "rows": r.rows,
        })).collect::<Vec<_>>(),
        "skipped": load.skipped.iter().map(|s| serde_json::json!({
            "relation": s.relation, "tool": s.tool, "dataset_id": s.dataset_id, "hosts": s.hosts,
        })).collect::<Vec<_>>(),
    });
    if let (Some(sessions), Some(fields)) = (load.sessions, after.as_object_mut()) {
        let (kept, skipped) = sessions.history_json();
        fields.insert("sessions".to_string(), kept);
        fields.insert("sessions_skipped".to_string(), skipped);
    }
    conn.execute(
        "INSERT INTO ox.history \
         (id, \"at\", \"by\", target_kind, target_id, action, before_json, after_json) \
         VALUES (nextval('ox.history_seq'), ?, ?, 'source', ?, 'source_load', NULL, ?)",
        params![loaded_at, loaded_by, id.to_string(), after.to_string()],
    )?;

    let superseded = supersede(conn, id, &loaded_at, actor)?;
    #[cfg(test)]
    if FAIL_AFTER_SUPERSEDE.with(std::cell::Cell::get) {
        return Err(EngineError::Internal(
            "failpoint: the load failed after supersede".to_string(),
        ));
    }
    rebuild_relations(conn)?;
    conn.execute_batch(&format!("DROP SCHEMA {} CASCADE;", quote_ident(STAGE)))?;

    Ok(LoadReport {
        source_id: id,
        relations,
        superseded,
        warnings,
        skipped: load.skipped.clone(),
    })
}

/// Refuse `dataset` when a live source already feeding its relation named
/// the metadata columns (run id, host, identity, output file) differently.
/// The suite renames one (`_triage_host_1` ...) when a dataset has a column
/// of that name; merging two namings would give the relation two host
/// columns, so the disagreement is refused, as a typing one is. Live sources
/// of a relation always agree, so checking against any one of them is enough.
/// Only a source with a table for the relation feeds it: one whose dataset
/// was left out (a per-channel copy) or had no file to load is in the
/// inventory and binds nothing.
fn check_metadata_columns(conn: &Connection, dataset: &Dataset) -> Result<()> {
    let recorded: Option<String> = conn
        .query_row(
            "SELECT i.metadata_columns_json FROM ox.inventory_dataset i \
             JOIN ox.source s ON s.id = i.source_id \
             JOIN ox.source_table st ON st.source_id = i.source_id AND st.relation = i.view \
             WHERE i.view = ? AND NOT s.removed ORDER BY i.source_id LIMIT 1",
            params![dataset.view],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            duckdb::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    let Some(json) = recorded else {
        return Ok(());
    };
    // The engine wrote this record from an inventory it had parsed.
    let ours: MetadataColumns = serde_json::from_str(&json).map_err(|e| {
        EngineError::Internal(format!(
            "the case's inventory record for {} is unreadable: {e}",
            dataset.view
        ))
    })?;
    let theirs = &dataset.metadata_columns;
    for (role, ours, theirs) in [
        ("run id", &ours.run_id, &theirs.run_id),
        ("host", &ours.host, &theirs.host),
        ("identity", &ours.identity, &theirs.identity),
        ("output file", &ours.output_file, &theirs.output_file),
    ] {
        if ours != theirs {
            return Err(EngineError::Invalid(format!(
                "{}: this run names its {role} column {theirs:?}, but the case has it as \
                 {ours:?}; a relation's metadata columns keep one name across sources",
                dataset.view
            )));
        }
    }
    Ok(())
}

/// Refuse dataset `view` when it, or the `raw_<view>` view it brings, would
/// take a name the case's relations already use: a relation's own name or
/// its `raw_` view's, compared without regard to case, as DuckDB resolves
/// names (`fold_name`, as the file loader compares them).
/// `rebuild_relations` would otherwise replace one view with the other
/// without a word. The one allowed match is the dataset relation of exactly
/// this name, which a later run of the same tool feeds. It runs before a
/// dataset's columns are checked, and again as each dataset loads, so that
/// datasets loaded earlier in the same run, relations by then, are checked
/// too.
fn check_relation_names(conn: &Connection, view: &str) -> Result<()> {
    let existing: Vec<(String, String)> = conn
        .prepare("SELECT name, kind FROM ox.relation ORDER BY name")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let ours = [fold_name(view), fold_name(&format!("raw_{view}"))];
    for (name, kind) in existing {
        if name == view && kind == "dataset" {
            continue;
        }
        let theirs = [fold_name(&name), fold_name(&format!("raw_{name}"))];
        if ours.iter().any(|o| theirs.contains(o)) {
            return Err(EngineError::Refused(format!(
                "dataset {view} (and its view raw_{view}) would take a name the case's \
                 {kind} relation {name} (and its view raw_{name}) already uses; remove that \
                 source or load the file under another name"
            )));
        }
    }
    Ok(())
}

/// Whether any of `dataset`'s files is included in a view, so that loading
/// it adds rows.
fn loads(dataset: &Dataset) -> bool {
    dataset.files.iter().any(|f| f.included_in_view)
}

/// Materialize one dataset's typed view and record its table, its hosts and
/// its relation.
fn load_dataset(
    conn: &Connection,
    source_id: i64,
    dataset: &Dataset,
    included: &[&DatasetFile],
) -> Result<LoadedRelation> {
    check_relation_names(conn, &dataset.view)?;

    let meta = &dataset.metadata_columns;
    // Host and identity, then every column by name, each read as its text.
    // The run id and the output file are left out: they differ between two
    // runs that read the same event.
    //
    // A column the relation already catalogues under another case of the
    // same name (`EventId`, this run's `eventid`) is one column, as DuckDB
    // and `check_typing` take it, so it is hashed, and sorted, under the
    // catalog's spelling: the same row then has the same hash in both runs,
    // and supersession matches it. Only an unambiguous respelling is mapped:
    // a name the catalog has exactly keeps its own spelling (a dataset whose
    // files spell a column two ways has both names catalogued, and each must
    // keep hashing as it did), and so does a name two or more catalogued
    // names match without regard to case. A column the catalog lacks, and
    // every column of a first load, is hashed under its own name as before.
    let catalogued = column_names(conn, &dataset.view)?;
    let mut by_fold: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    for name in &catalogued {
        by_fold.entry(name.to_lowercase()).or_default().push(name);
    }
    let hashed_as = |name: &String| -> String {
        if catalogued.contains(name) {
            return name.clone();
        }
        match by_fold.get(&name.to_lowercase()).map(Vec::as_slice) {
            Some([only]) => (*only).clone(),
            _ => name.clone(),
        }
    };
    let mut columns: Vec<(String, &String)> = dataset
        .columns
        .iter()
        .map(|name| (hashed_as(name), name))
        .collect();
    columns.sort();
    let hash_columns: Vec<HashColumn> = [&meta.host, &meta.identity]
        .into_iter()
        .map(|name| (hashed_as(name), name))
        .chain(columns)
        .map(|(hashed, name)| HashColumn {
            name: hashed,
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

/// `value`, a part of an inventory that was parsed from JSON, as JSON again;
/// failing that is a fault of the engine's, not of the inventory.
fn to_json<T: serde::Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|e| EngineError::Internal(format!("the inventory does not serialize: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `C:\` then enough 40-character folders to pass the limit, then `tail`.
    fn long(tail: &str) -> String {
        format!(r"C:\{}{tail}", format!("{}\\", "d".repeat(40)).repeat(7))
    }

    #[test]
    fn a_short_path_is_read_as_written() {
        for path in [
            r"C:\out\run\a.csv",
            r"C:\out\run/mixed/a.csv",
            r"\\srv\share\a.csv",
        ] {
            assert_eq!(verbatim_when_long(path), path);
        }
    }

    #[test]
    fn a_long_drive_path_is_read_in_verbatim_form() {
        let path = long("a.csv");
        assert_eq!(verbatim_when_long(&path), format!(r"\\?\{path}"));
    }

    /// The suite's inventory joins its relative paths with `/`, and the
    /// verbatim form takes `/` as a character of a name, not a separator.
    #[test]
    fn a_long_path_has_its_forward_slashes_made_separators() {
        let path = long("Processed-WS01/EventLogs/evtx.csv");
        assert_eq!(
            verbatim_when_long(&path),
            format!(r"\\?\{}", long(r"Processed-WS01\EventLogs\evtx.csv"))
        );
    }

    #[test]
    fn a_long_share_path_is_read_in_verbatim_unc_form() {
        let path = long("a.csv").replacen(r"C:\", r"\\srv\share\", 1);
        assert_eq!(
            verbatim_when_long(&path),
            format!(r"\\?\UNC\{}", &path[2..])
        );
    }

    #[test]
    fn a_path_already_verbatim_or_not_absolute_is_left_alone() {
        let verbatim = format!(r"\\?\{}", long("a/b.csv"));
        assert_eq!(verbatim_when_long(&verbatim), verbatim);
        let relative = long("a.csv").replacen(r"C:\", "", 1);
        assert_eq!(verbatim_when_long(&relative), relative);
    }

    /// The limit counts UTF-16 units, as Windows does, not bytes.
    #[test]
    fn the_limit_counts_characters_not_bytes() {
        let path = format!(r"C:\{}\a.csv", "é".repeat(200));
        assert!(path.len() > WINDOWS_PATH_LIMIT);
        assert_eq!(verbatim_when_long(&path), path);
    }

    fn count(case: &Case, sql: &str) -> i64 {
        case.scalar_i64(sql)
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
    }

    /// A load that fails after `supersede` has run, with the earlier run's
    /// annotations already moved onto the new rows, rolls all of it back:
    /// the earlier rows are still the ones in view, the annotations are on
    /// them, and no source, history row or schema of the failed load is left.
    /// (Only the tree comes from `oxl_fixtures`; the case is this crate's.)
    #[test]
    fn a_load_that_fails_after_supersede_rolls_back() {
        use crate::annot::{add_note, add_tag, mark_suspicious};
        let d = tempfile::tempdir().unwrap();
        let analyst = Actor::Analyst("tester".into());
        let mut case = Case::create(&d.path().join("t.case.duckdb"), "T", &analyst).unwrap();
        load_run(
            &mut case,
            &oxl_fixtures::two_hosts(&d.path().join("a")),
            &analyst,
        )
        .unwrap();
        let svch0st = "SELECT _ox_row FROM petriage_timeline \
                       WHERE ExecutableName = 'SVCH0ST.EXE' AND _triage_host = 'WS01'";
        let row = count(&case, svch0st);
        assert!(mark_suspicious(&mut case, &[row], &analyst).unwrap()[0].done);
        assert!(add_tag(&mut case, &[row], "lateral", &analyst).unwrap()[0].done);
        let note = add_note(&mut case, row, "look here", &analyst).unwrap();
        let history = count(&case, "SELECT count(*) FROM ox.history");
        let active = count(&case, "SELECT count(*) FROM ox.source_dataset WHERE active");

        let rerun = oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b"));
        FAIL_AFTER_SUPERSEDE.with(|f| f.set(true));
        let failed = load_run(&mut case, &rerun, &analyst);
        FAIL_AFTER_SUPERSEDE.with(|f| f.set(false));
        let e = failed.unwrap_err();
        assert!(e.to_string().contains("after supersede"), "{e}");

        // Still the earlier run's row, with its annotations on it.
        assert_eq!(count(&case, svch0st), row);
        for table in ["row_mark", "row_tag", "note"] {
            assert_eq!(
                count(
                    &case,
                    &format!("SELECT count(*) FROM ox.{table} WHERE row_id = {row}")
                ),
                1,
                "{table}"
            );
        }
        assert_eq!(
            count(
                &case,
                &format!("SELECT id FROM ox.note WHERE row_id = {row}")
            ),
            note.id
        );
        // No new source, history row, schema or superseded dataset.
        assert_eq!(count(&case, "SELECT count(*) FROM ox.source"), 1);
        assert_eq!(count(&case, "SELECT count(*) FROM ox.history"), history);
        assert_eq!(
            count(&case, "SELECT count(*) FROM ox.source_dataset WHERE active"),
            active
        );
        assert_eq!(
            count(
                &case,
                "SELECT count(*) FROM duckdb_schemas() \
                 WHERE schema_name IN ('src_2', 'ox_stage')"
            ),
            0
        );
        assert_eq!(
            count(
                &case,
                "SELECT count(*) FROM petriage_timeline WHERE _triage_run_id <> '20260101000000-aaaa'"
            ),
            0
        );

        // The failpoint was the only thing wrong: without it the same load
        // supersedes and moves the annotations off the earlier row.
        let report = load_run(&mut case, &rerun, &analyst).unwrap();
        // The id sequence is not transactional, so the id is not 2.
        assert!(report.source_id > 1);
        assert_eq!(count(&case, "SELECT count(*) FROM ox.source"), 2);
        assert_ne!(count(&case, svch0st), row);
        assert_eq!(
            count(
                &case,
                &format!("SELECT count(*) FROM ox.row_mark WHERE row_id = {row}")
            ),
            0
        );
    }
}
