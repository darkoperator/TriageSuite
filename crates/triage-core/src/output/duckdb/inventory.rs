//! The `datasets.json` model.
//!
//! This is the single source of truth for the view layer: `views.sql` is a
//! pure function of it, which is what lets the SQL be regenerated after a
//! collection moves and lets the renderer be tested without a filesystem.
//!
//! Every path here is **relative to `out_root`**. Absolute paths appear only
//! in the rendered SQL, because `read_csv` has no notion of a base
//! directory, and storing them would make a moved collection's inventory
//! wrong rather than merely stale.

use crate::output::duckdb::types::{SqlType, TimeSemantics};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Inventory schema version, independent of `run_manifest.json`'s.
pub const SCHEMA_VERSION: u32 = 1;

/// Why this inventory looks the way it does.
///
/// A run that produced nothing queryable still writes the pair, carrying the
/// reason. Writing nothing would leave a *previous* run's artifacts sitting
/// in a reused `--out` looking current, which is the failure this field
/// exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// The run published CSV and the views describe it.
    Ok,
    /// A JSON-only run: nothing to build views over.
    NoCsvOutput,
    /// The input was refused; there are no hosts.
    RunRejected,
    /// Rendering or reading failed; `warnings` says why.
    GenerationFailed,
}

/// The `read_csv` options every generated call passes, recorded so a
/// consumer can reproduce a scan by hand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReaderOptions {
    pub header: bool,
    pub all_varchar: bool,
    pub union_by_name: bool,
    /// Always false. An explicit file list under a `key=value` directory
    /// still injects a phantom column otherwise -- measured, not assumed.
    pub hive_partitioning: bool,
    pub delim: String,
    pub quote: String,
    pub escape: String,
    pub ignore_errors: bool,
    pub null_policy: String,
}

impl Default for ReaderOptions {
    fn default() -> Self {
        ReaderOptions {
            header: true,
            all_varchar: true,
            union_by_name: true,
            hive_partitioning: false,
            delim: ",".into(),
            quote: "\"".into(),
            escape: "\"".into(),
            ignore_errors: false,
            null_policy: "blank cell reads as NULL; CSV cannot distinguish \
                          an empty string from an absent value"
                .into(),
        }
    }
}

/// What one file is to its dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileRole {
    /// The router published it and nothing derives from it.
    Primary,
    /// A Velo category file merged from per-user slices.
    Merged,
    /// A per-user slice that a successful merge consumed.
    Slice,
}

/// The identity a file's rows belong to, or `None` for a merged file, whose
/// rows span every user whose slice fed it and carry their own `TriageUser`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FileIdentity {
    User { name: String },
    System,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetFile {
    /// Relative to `Inventory::out_root`.
    pub path: PathBuf,
    pub host: String,
    pub identity: Option<FileIdentity>,
    pub role: FileRole,
    pub included_in_view: bool,
    /// Relative path of the merged file this slice was folded into.
    pub derived_into: Option<PathBuf>,
    /// The file's own header, after BOM stripping and DuckDB rename replay.
    /// `None` when it could not be read.
    pub header: Option<Vec<String>>,
    pub warnings: Vec<String>,
}

/// A column that survived into the typed view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectiveType {
    pub sql_type: String,
    /// The companion column holding the original cell text.
    pub text_column: String,
    /// Set only for timestamps.
    pub precision: Option<String>,
    /// Set only for timestamps.
    pub time_semantics: Option<String>,
}

impl EffectiveType {
    pub fn new(sql_type: SqlType, text_column: String, semantics: Option<TimeSemantics>) -> Self {
        EffectiveType {
            sql_type: sql_type.sql().to_string(),
            text_column,
            precision: matches!(sql_type, SqlType::Timestamp).then(|| {
                "microsecond; source text carries 100ns resolution and is \
                 preserved verbatim in the text column"
                    .to_string()
            }),
            time_semantics: semantics.map(|s| s.as_str().to_string()),
        }
    }
}

/// The names the injected metadata columns actually got, after collision
/// resolution against every header in the dataset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetadataColumns {
    pub run_id: String,
    pub host: String,
    pub identity: String,
    pub output_file: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dataset {
    pub view: String,
    pub raw_view: String,
    pub tool: String,
    pub dataset_id: String,
    /// `"internal"` or `"external"`.
    pub source: String,
    pub metadata_columns: MetadataColumns,
    /// The dataset's own column naming the *evidence* path, when it has one.
    /// Never invented: `None` when no known column is present.
    pub evidence_path_column: Option<String>,
    /// Union of the headers of every included file.
    pub columns: Vec<String>,
    pub effective_types: BTreeMap<String, EffectiveType>,
    pub files: Vec<DatasetFile>,
}

/// A published file that is inventoried but gets no view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventoryOnly {
    pub tool: String,
    pub dataset_id: String,
    pub format: String,
    pub path: PathBuf,
    pub reason: String,
}

/// A declared override naming a column no file actually has.
///
/// The override is dropped from the projection. This does **not** turn the
/// column into VARCHAR -- the column does not exist at all, so no typed
/// column and no text companion appear for it. The only thing preserved is
/// that `views.sql` still loads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DroppedOverride {
    pub view: String,
    pub dataset_id: String,
    pub column: String,
    pub declared_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    pub schema_version: u32,
    /// Shared with `views.sql`'s header comment. Mismatched generations mean
    /// a torn pair that must not be trusted.
    pub generation: String,
    pub run_id: String,
    pub generated_utc: String,
    pub out_root: PathBuf,
    pub status: Status,
    pub reader_options: ReaderOptions,
    pub datasets: Vec<Dataset>,
    pub inventory_only: Vec<InventoryOnly>,
    pub dropped_overrides: Vec<DroppedOverride>,
    pub warnings: Vec<String>,
}

impl Inventory {
    /// An inventory with no datasets, for a run that produced nothing
    /// queryable. `status` says which kind of nothing.
    pub fn empty(
        run_id: &str,
        generation: &str,
        generated_utc: &str,
        out_root: &Path,
        status: Status,
    ) -> Self {
        Inventory {
            schema_version: SCHEMA_VERSION,
            generation: generation.to_string(),
            run_id: run_id.to_string(),
            generated_utc: generated_utc.to_string(),
            out_root: out_root.to_path_buf(),
            status,
            reader_options: ReaderOptions::default(),
            datasets: Vec::new(),
            inventory_only: Vec::new(),
            dropped_overrides: Vec::new(),
            warnings: Vec::new(),
        }
    }
}
