//! The one error type every engine call returns.
//!
//! Each variant carries a stable machine code (`code()`), which is what an
//! agent or the CLI's JSON envelope branches on. The display text is for a
//! person and may change; the code may not.

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, EngineError>;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid: {0}")]
    Invalid(String),
    /// A rule said no; the request itself was well formed.
    #[error("refused: {0}")]
    Refused(String),
    #[error("the case file {} is open in another process", .0.display())]
    CaseLocked(PathBuf),
    #[error("the case file is format {found}; this build reads format {expected}")]
    NeedsUpgrade { found: u32, expected: u32 },
    #[error("engine: {0}")]
    Db(#[from] duckdb::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl EngineError {
    /// The stable machine code for this error.
    pub fn code(&self) -> &'static str {
        match self {
            EngineError::NotFound(_) => "not_found",
            EngineError::Invalid(_) => "invalid",
            EngineError::Refused(_) => "refused",
            EngineError::CaseLocked(_) => "case_locked",
            EngineError::NeedsUpgrade { .. } => "needs_upgrade",
            EngineError::Db(_) => "engine",
            EngineError::Io(_) => "io",
        }
    }
}
