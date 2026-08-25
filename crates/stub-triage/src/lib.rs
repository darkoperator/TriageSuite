use serde::Serialize;
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};

#[derive(Serialize)]
pub struct StubRecord {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Value")]
    pub value: String,
    #[serde(rename = "SourceFile")]
    pub source_file: String,
}

pub const DATASETS: &[DatasetSpec] = &[DatasetSpec {
    id: "stub",
    default_basename: "StubTriage_Output",
    framing: JsonFraming::Ndjson,
    csv_only: false,
    override_suffix: None,
}];

pub struct StubTool;

impl Tool for StubTool {
    fn binary_name(&self) -> &'static str {
        "StubTriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &["*.stub"]
    }

    fn validate_legacy(&self, path: &Path) -> bool {
        std::fs::read(path)
            .map(|b| b.len() >= 4 && &b[..4] == b"STUB")
            .unwrap_or(false)
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
        let content = std::fs::read_to_string(path).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let body = content
            .strip_prefix("STUB\n")
            .ok_or_else(|| TriageError::Artifact {
                path: path.to_path_buf(),
                message: "malformed stub header".into(),
            })?;
        let mut n = 0u64;
        for line in body.lines() {
            if let Some((k, v)) = line.split_once('=') {
                out.write(
                    "stub",
                    &StubRecord {
                        name: k.to_string(),
                        value: v.to_string(),
                        source_file: path.display().to_string(),
                    },
                )?;
                n += 1;
            }
        }
        Ok(n)
    }
}
