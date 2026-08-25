use std::path::PathBuf;

/// Process exit codes per spec section 9.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunExit {
    Success,
    Usage,
    InputMissing,
    OutputFailure,
    Partial,
    Fatal,
}

impl RunExit {
    pub fn code(self) -> i32 {
        match self {
            RunExit::Success => 0,
            RunExit::Usage => 2,
            RunExit::InputMissing => 3,
            RunExit::OutputFailure => 4,
            RunExit::Partial => 5,
            RunExit::Fatal => 6,
        }
    }
}

/// Suite-wide error type. Every variant that concerns a file carries its path.
#[derive(Debug, thiserror::Error)]
pub enum TriageError {
    #[error("usage error: {0}")]
    Usage(String),
    #[error("input not found or unreadable: {path}")]
    InputMissing { path: PathBuf },
    #[error("output failure at {path}: {message}")]
    Output { path: PathBuf, message: String },
    #[error("artifact failure in {path}: {message}")]
    Artifact { path: PathBuf, message: String },
    #[error("fatal: {0}")]
    Fatal(String),
}

impl TriageError {
    /// Fallback exit-code mapping for an error that terminates a run on its
    /// own. Aggregate policy (partial vs fatal across many artifacts) is owned
    /// by the runner's RunSummary, not this method; the runner never calls
    /// this for per-artifact failures it has already counted.
    pub fn run_exit(&self) -> RunExit {
        match self {
            TriageError::Usage(_) => RunExit::Usage,
            TriageError::InputMissing { .. } => RunExit::InputMissing,
            TriageError::Output { .. } => RunExit::OutputFailure,
            TriageError::Artifact { .. } => RunExit::Partial,
            TriageError::Fatal(_) => RunExit::Fatal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_spec_section_9() {
        assert_eq!(RunExit::Success.code(), 0);
        assert_eq!(RunExit::Usage.code(), 2);
        assert_eq!(RunExit::InputMissing.code(), 3);
        assert_eq!(RunExit::OutputFailure.code(), 4);
        assert_eq!(RunExit::Partial.code(), 5);
        assert_eq!(RunExit::Fatal.code(), 6);
    }

    #[test]
    fn errors_carry_artifact_path() {
        let e = TriageError::Artifact {
            path: "/cap/C/Users/a/x.pf".into(),
            message: "truncated header".into(),
        };
        assert!(e.to_string().contains("/cap/C/Users/a/x.pf"));
        assert_eq!(e.run_exit(), RunExit::Partial);
        assert_eq!(TriageError::Usage("bad".into()).run_exit(), RunExit::Usage);

        let out_err = TriageError::Output {
            path: "/out/report.json".into(),
            message: "permission denied".into(),
        };
        assert!(out_err.to_string().contains("/out/report.json"));
        assert!(out_err.to_string().contains("permission denied"));
        assert_eq!(out_err.run_exit(), RunExit::OutputFailure);

        let missing = TriageError::InputMissing {
            path: "/in/sample.pf".into(),
        };
        assert!(missing.to_string().contains("/in/sample.pf"));
        assert_eq!(missing.run_exit(), RunExit::InputMissing);
    }
}
