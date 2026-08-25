//! WxTTriage: WxTCmd-compatible Windows Timeline (ActivitiesCache.db) parser.
//! Reads the Activity, ActivityOperation, and Activity_PackageId tables and
//! emits WxTCmd's three datasets in CSV + NDJSON.

pub mod appid;
pub mod cli;
pub mod decode;
pub mod record;
pub mod tables;

use std::path::Path;

use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};
use triage_sqlite::Database;

/// SQLite file magic: "SQLite format 3\0".
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

pub const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        id: "activity",
        default_basename: "WxTTriage_Activity_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    },
    DatasetSpec {
        id: "activity_operation",
        default_basename: "WxTTriage_ActivityOperation_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_ActivityOperation"),
    },
    DatasetSpec {
        id: "activity_packageid",
        default_basename: "WxTTriage_Activity_PackageId_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_Activity_PackageId"),
    },
];

pub struct WxtTool;

impl Default for WxtTool {
    fn default() -> Self {
        WxtTool
    }
}

impl Tool for WxtTool {
    fn binary_name(&self) -> &'static str {
        "WxTTriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &["ActivitiesCache.db"]
    }

    fn validate_legacy(&self, path: &Path) -> bool {
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        use std::io::Read;
        let mut buf = [0u8; 16];
        matches!(f.read_exact(&mut buf), Ok(())) && &buf == SQLITE_MAGIC
    }

    fn invalid_content_is_corrupt(&self) -> bool {
        true
    }

    fn datasets(&self) -> &'static [DatasetSpec] {
        DATASETS
    }

    fn scope(&self) -> Scope {
        Scope::UserSpecific
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        let db = Database::open(path).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let mut count = 0u64;

        let activities = tables::read_activity(&db).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: format!("Activity: {e}"),
        })?;
        for rec in &activities {
            out.write("activity", rec)?;
            count += 1;
        }

        let ops = tables::read_activity_operation(&db).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: format!("ActivityOperation: {e}"),
        })?;
        for rec in &ops {
            out.write("activity_operation", rec)?;
            count += 1;
        }

        let pkgs = tables::read_package_id(&db).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: format!("Activity_PackageId: {e}"),
        })?;
        for rec in &pkgs {
            out.write("activity_packageid", rec)?;
            count += 1;
        }

        Ok(count)
    }
}
