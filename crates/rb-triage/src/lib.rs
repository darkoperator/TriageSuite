use serde::Serialize;
use std::io::Read;
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_core::tool::{Scope, Tool};

/// RBCmd CsvOut-compatible record (column names and order are the contract).
/// RBTriage adds JSON output using these same fields (spec section 6.4).
#[derive(Serialize)]
pub struct RecycleRecord {
    #[serde(rename = "SourceName")]
    pub source_name: String,
    #[serde(rename = "FileType")]
    pub file_type: String, // "$I" or "INFO2"
    #[serde(rename = "FileName")]
    pub file_name: String,
    #[serde(rename = "FileSize")]
    pub file_size: i64,
    #[serde(rename = "DeletedOn")]
    pub deleted_on: WinTimestamp,
}

pub const DATASETS: &[DatasetSpec] = &[DatasetSpec {
    id: "main",
    default_basename: "RBTriage_Output",
    framing: JsonFraming::Ndjson,
    csv_only: false,
    override_suffix: None,
}];

pub struct RbTool;

impl Default for RbTool {
    fn default() -> Self {
        RbTool
    }
}

impl Tool for RbTool {
    fn binary_name(&self) -> &'static str {
        "RBTriage"
    }

    /// $I records and legacy INFO2. validate() is the real content gate.
    fn patterns(&self) -> &[&'static str] {
        &["$I*", "INFO2"]
    }

    /// Content validation (spec 3.2, never extension-only): INFO2 = name +
    /// readable 4-byte header; $I = first i64 format field is 1 or 2.
    fn validate_legacy(&self, path: &Path) -> bool {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        if name.eq_ignore_ascii_case("INFO2") {
            let mut head = [0u8; 4];
            return f.read_exact(&mut head).is_ok();
        }
        let mut head = [0u8; 8];
        if f.read_exact(&mut head).is_err() {
            return false;
        }
        matches!(i64::from_le_bytes(head), 1 | 2)
    }

    fn invalid_content_is_corrupt(&self) -> bool {
        true
    }

    fn datasets(&self) -> &'static [DatasetSpec] {
        DATASETS
    }

    /// User-specific: the Attributor already maps `$Recycle.Bin/<SID>/...` to
    /// Identity::User(<SID>) (M0). ProfileList SID->username resolution is
    /// deferred to M5 when the registry engine lands.
    fn scope(&self) -> Scope {
        Scope::UserSpecific
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        let raw = std::fs::read(path).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let source = path.display().to_string();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut n = 0u64;

        if name.eq_ignore_ascii_case("INFO2") {
            let entries =
                triage_recyclebin::info2::parse(&raw).map_err(|e| TriageError::Artifact {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                })?;
            for e in entries {
                out.write(
                    "main",
                    &RecycleRecord {
                        source_name: source.clone(),
                        file_type: "INFO2".into(),
                        file_name: e.file_name,
                        file_size: e.file_size,
                        deleted_on: WinTimestamp::from_filetime(e.deleted_on),
                    },
                )?;
                n += 1;
            }
        } else {
            // Note: RBCmd additionally expands a deleted folder's $R companion
            // directory into extra DirectoryFiles rows. Deliberately not done
            // in v1 (no deleted folders in scope; reaching into sibling $R
            // trees is filesystem-coupled behavior to design later).
            let e =
                triage_recyclebin::dollar_i::parse(&raw).map_err(|e| TriageError::Artifact {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                })?;
            out.write(
                "main",
                &RecycleRecord {
                    source_name: source,
                    file_type: "$I".into(),
                    file_name: e.file_name,
                    file_size: e.file_size,
                    deleted_on: WinTimestamp::from_filetime(e.deleted_on),
                },
            )?;
            n += 1;
        }
        Ok(n)
    }
}
