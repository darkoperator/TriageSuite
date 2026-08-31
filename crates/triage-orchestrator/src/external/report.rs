use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct ExternalToolReport {
    pub tool: String,
    pub found: bool,
    pub invoked: bool,
    pub exit_code: Option<i32>,
    pub output_paths: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub(super) fn not_found(tool: &str) -> ExternalToolReport {
    ExternalToolReport {
        tool: tool.to_string(),
        found: false,
        invoked: false,
        exit_code: None,
        output_paths: Vec::new(),
        error: None,
    }
}

/// A tool that was enabled and whose binary we never even looked for, because a
/// prerequisite from an earlier tool wasn't there. Reported as `found: true`:
/// the reason it didn't run has nothing to do with whether it is installed, and
/// silently omitting it from the manifest would be worse than saying so.
pub(super) fn skipped(tool: &str, reason: &str) -> ExternalToolReport {
    ExternalToolReport {
        tool: tool.to_string(),
        found: true,
        invoked: false,
        exit_code: None,
        output_paths: Vec::new(),
        error: Some(reason.to_string()),
    }
}
