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
    /// The input is wrong: malformed, out of range, naming something of the
    /// wrong kind, or a type that does not fit. Changing it can fix it.
    #[error("invalid: {0}")]
    Invalid(String),
    /// A rule said no to this actor or to this state of the case: an agent
    /// doing what only an analyst may, a source already in the case, a
    /// colliding relation, a removal that needs confirmation. The input's
    /// shape is not the problem.
    #[error("refused: {0}")]
    Refused(String),
    #[error("the case file {} is open in another process", .0.display())]
    CaseLocked(PathBuf),
    #[error("the case file is format {found}; this build reads format {expected}")]
    NeedsUpgrade { found: u32, expected: u32 },
    /// The case or DuckDB broke an invariant the engine relies on: a value
    /// the engine never writes, a row it listed and could not read back, a
    /// lockdown that did not hold. Nothing the caller changes fixes it.
    #[error("internal: {0}")]
    Internal(String),
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
            EngineError::Internal(_) => "internal",
            EngineError::Db(_) => "engine",
            EngineError::Io(_) => "io",
        }
    }
}
