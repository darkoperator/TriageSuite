//! SQLETriage: SQLECmd-compatible SQLite map engine. Runs the bundled `.smap`
//! corpus against SQLite databases and emits one CSV + NDJSON per map query.

pub mod cli;
pub mod engine;
pub mod map;
pub mod output;
pub mod render;
pub mod sync;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use triage_core::error::TriageError;
use triage_core::output::dataset::DatasetSpec;
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool, Validation};
use triage_sqlite::Database;

use map::SqlMap;

/// SQLite file magic: "SQLite format 3\0".
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// SQLETriage writes all output dynamically (one file per map query), so it
/// declares no static datasets.
pub const DATASETS: &[DatasetSpec] = &[];

pub struct SqleTool {
    pub hunt: bool,
    pub dedupe: bool,
    pub noblob: bool,
    maps: Vec<(String, SqlMap)>,
    seen: Mutex<HashSet<[u8; 20]>>,
}

impl SqleTool {
    pub fn new(hunt: bool, dedupe: bool, noblob: bool) -> Self {
        SqleTool {
            hunt,
            dedupe,
            noblob,
            maps: map::load_bundled_maps(),
            seen: Mutex::new(HashSet::new()),
        }
    }
}

impl Default for SqleTool {
    fn default() -> Self {
        SqleTool::new(false, true, false)
    }
}

impl Tool for SqleTool {
    fn binary_name(&self) -> &'static str {
        "SQLETriage"
    }

    fn patterns(&self) -> &[&'static str] {
        if self.hunt {
            &["*"]
        } else {
            &[
                "*.db",
                "*.sqlite",
                "*.sqlite3",
                "History",
                "Cookies",
                "Web Data",
                "places.sqlite",
                "favicons",
                "shortcuts",
                "ActivitiesCache.db",
                "wpndatabase.db",
            ]
        }
    }

    fn validate_legacy(&self, path: &Path) -> bool {
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        use std::io::Read;
        let mut buf = [0u8; 16];
        matches!(f.read_exact(&mut buf), Ok(())) && &buf == SQLITE_MAGIC
    }

    fn validate(&self, path: &Path) -> Validation {
        if let Err(error) = std::fs::File::open(path) {
            return Validation::Unreadable {
                error: error.to_string(),
            };
        }
        if self.validate_legacy(path) {
            Validation::Supported
        } else if self.hunt {
            Validation::Unsupported {
                reason: "not a SQLite database".into(),
            }
        } else {
            Validation::Corrupt {
                reason: "known SQLite database name has invalid header".into(),
            }
        }
    }

    fn datasets(&self) -> &'static [DatasetSpec] {
        DATASETS
    }

    fn scope(&self) -> Scope {
        Scope::UserElseSystem
    }

    fn resource_class(&self) -> triage_core::tool::ResourceClass {
        triage_core::tool::ResourceClass::Heavy
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        if self.dedupe {
            if let Some(h) = engine::file_sha1(path) {
                let mut seen = self.seen.lock().expect("dedupe mutex");
                if !seen.insert(h) {
                    return Ok(0);
                }
            }
        }

        let db = Database::open(path).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let db_filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let mut results = engine::run_maps(&db, &self.maps, &db_filename, self.hunt, self.noblob);
        if results.is_empty() {
            return Ok(0);
        }

        // SQLECmd appends a trailing SourceFile column (the source DB path) to
        // every output. Mirror it — this also disambiguates rows when multiple
        // databases merge into one per-identity output file.
        let source = path.to_string_lossy().to_string();
        for r in &mut results {
            r.columns.push("SourceFile".to_string());
            for row in &mut r.rows {
                row.push(source.clone());
            }
        }

        let mut count = 0u64;
        for r in &results {
            for row in &r.rows {
                out.write_dynamic_row(&r.basename, &r.columns, row)?;
                count += 1;
            }
        }
        Ok(count)
    }
}
