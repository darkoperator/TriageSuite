//! Writing the DuckDB view layer at the end of a run.
//!
//! Two properties matter here and neither is about SQL.
//!
//! The pair is *matched*: both files carry the same generation string, so a
//! consumer that finds them disagreeing knows the write was torn and the SQL
//! does not describe the inventory beside it.
//!
//! The pair always describes *this* run. A run that produced nothing
//! queryable still writes it, carrying the reason -- because over a reused
//! `--out`, writing nothing leaves the previous run's artifacts sitting
//! there looking current, which is a worse answer than an honest empty one.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use triage_core::output::duckdb::inventory::{Inventory, Status};
use triage_core::output::duckdb::render::render_sql;

/// Breaks a tie the clock cannot: two calls inside one process can read the
/// same nanosecond on a platform whose clock is coarser than its unit.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A generation id: the run id plus 16 hex characters of clock, unique per
/// call so two runs starting in the same second cannot collide.
pub fn generation_id(run_id: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    // Truncating to 64 bits is fine: this is a collision breaker within one
    // output directory, not an identifier anything else resolves.
    let low = (nanos as u64) ^ (nanos >> 64) as u64;
    let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    // Rotated rather than added, so the sequence cannot simply cancel a
    // clock difference between two calls and hand back the same id.
    let mixed = low ^ seq.rotate_left(32);
    format!("{run_id}-{mixed:016x}")
}

/// Write `duckdb/datasets.json` and `duckdb/views.sql` under `out_root`,
/// replacing any earlier generation.
///
/// Both files are rendered in full before either is written, and each is
/// staged next to its destination and renamed into place -- inventory
/// first, so a reader that gets the SQL always has the inventory that
/// produced it.
pub fn write_pair(out_root: &Path, inventory: &Inventory) -> std::io::Result<()> {
    let dir = out_root.join("duckdb");
    std::fs::create_dir_all(&dir)?;

    let (json, sql) = match serde_json::to_string_pretty(inventory) {
        Ok(json) => (json, render_sql(inventory)),
        Err(error) => {
            // The one thing here that is not I/O and can still fail: serde_json
            // refuses a `PathBuf` that is not valid UTF-8, and on Unix every
            // path in the inventory -- `out_root` included, which is whatever
            // the operator passed to `--out` -- can be arbitrary bytes.
            //
            // Giving up silently would leave a reused `--out` holding the
            // PREVIOUS run's pair, looking current: the exact failure the pair
            // exists to prevent. So write an honest one instead, saying it
            // failed and why. This is the only site that can produce
            // `Status::GenerationFailed`.
            let degraded = generation_failed(out_root, inventory, &error.to_string());
            let json = serde_json::to_string_pretty(&degraded).map_err(std::io::Error::other)?;
            (json, render_sql(&degraded))
        }
    };

    stage_and_publish(&dir, "datasets.json", json.as_bytes())?;
    stage_and_publish(&dir, "views.sql", sql.as_bytes())?;
    Ok(())
}

/// The inventory written when the real one cannot be serialized: empty,
/// `GenerationFailed`, and carrying the serializer's own message.
///
/// `out_root` is re-derived lossily rather than reused, because it is a
/// candidate for the un-serializable path itself -- and a replacement that
/// fails the same way as the thing it replaces is no replacement. Every
/// other field here is already a `String`, so this inventory always
/// serializes.
fn generation_failed(out_root: &Path, inventory: &Inventory, error: &str) -> Inventory {
    let lossy = PathBuf::from(out_root.to_string_lossy().into_owned());
    let mut degraded = Inventory::empty(
        &inventory.run_id,
        &inventory.generation,
        &inventory.generated_utc,
        &lossy,
        Status::GenerationFailed,
    );
    degraded.warnings.push(format!(
        "the inventory for this run could not be serialized, so no views were \
         generated: {error}"
    ));
    degraded
}

fn stage_and_publish(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let staged = dir.join(format!("{name}.tmp"));
    std::fs::write(&staged, bytes)?;
    // Derived data: a stale one is never correct, so this replaces
    // unconditionally. The --overwrite guard protects evidence output and
    // does not apply here.
    std::fs::rename(&staged, dir.join(name))
}

/// Reject an inventory whose `sql_type` is not one of the known spellings.
///
/// `sql_type` is a `String` the renderer interpolates UNQUOTED into
/// `TRY_CAST(... AS {sql_type})`. On a fresh run it can only have come from
/// `SqlType::sql()` and is safe by construction -- but `regenerate` reads it
/// back off disk, where a hand-edited or tampered `datasets.json` turns it
/// into arbitrary SQL inside a file an analyst runs against evidence. Same
/// threat that already justified sanitising values interpolated into the
/// `--` comment lines; this one is worse, because it is not in a comment.
///
/// The allow-list is `SqlType::from_sql`, derived from `SqlType::ALL` rather
/// than duplicated here, so a new `SqlType` variant cannot silently produce
/// an inventory that this check then refuses on sight.
///
/// Refusing is the whole fix: there is no safe repair, and silently dropping
/// the override would hand back a `views.sql` that quietly disagrees with
/// the inventory beside it.
fn check_sql_types(
    inventory: &Inventory,
    path: &Path,
) -> Result<(), triage_core::error::TriageError> {
    use triage_core::output::duckdb::types::SqlType;

    for dataset in &inventory.datasets {
        for (column, effective) in &dataset.effective_types {
            if SqlType::from_sql(&effective.sql_type).is_none() {
                let allowed = SqlType::ALL
                    .iter()
                    .map(|t| t.sql())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(triage_core::error::TriageError::Output {
                    path: path.to_path_buf(),
                    message: format!(
                        "view {} column {} declares sql_type {:?}, which is not one of {}. \
                         Refusing to regenerate: this value is interpolated into SQL.",
                        dataset.view, column, effective.sql_type, allowed
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Re-render `views.sql` for a collection that has moved.
///
/// The inventory stores every path relative to the output root and only the
/// rendered SQL is absolute, so relocating a collection invalidates the SQL
/// and nothing else. This reads the inventory back, re-points it at
/// `out_root`, and writes a fresh matched generation.
///
/// It parses no evidence and runs no tools -- but it does trust a file on
/// disk, so every `sql_type` it reads back is checked against the four known
/// spellings before anything is rendered. See `check_sql_types`.
pub fn regenerate(out_root: &Path) -> Result<(), triage_core::error::TriageError> {
    let path = out_root.join("duckdb/datasets.json");
    let text =
        std::fs::read_to_string(&path).map_err(|e| triage_core::error::TriageError::Output {
            path: path.clone(),
            message: format!("cannot read inventory: {e}"),
        })?;
    let mut inventory: Inventory =
        serde_json::from_str(&text).map_err(|e| triage_core::error::TriageError::Output {
            path: path.clone(),
            message: format!("cannot parse inventory: {e}"),
        })?;
    check_sql_types(&inventory, &path)?;
    inventory.out_root = out_root.to_path_buf();
    inventory.generation = generation_id(&inventory.run_id);
    inventory.generated_utc = crate::manifest::now_iso();
    write_pair(out_root, &inventory).map_err(|e| triage_core::error::TriageError::Output {
        path: out_root.join("duckdb"),
        message: e.to_string(),
    })
}
