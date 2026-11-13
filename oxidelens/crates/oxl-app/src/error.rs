//! The one error every command answers with.
//!
//! `code` is what the front end branches on and never changes for a given
//! cause; `message` is for a person. The codes are every
//! `EngineError::code()` plus the app's own: `no_case` (no case is open),
//! `busy` (a write is already running), `superseded` (a newer query took the
//! slot) and `interrupted` (the query was cancelled).

use oxl_engine::EngineError;

/// No case is open.
pub const NO_CASE: &str = "no_case";
/// A write is already running; the app runs one at a time.
pub const BUSY: &str = "busy";
/// A newer query on the same slot interrupted this one.
pub const SUPERSEDED: &str = "superseded";
/// `query_cancel` named this query.
pub const INTERRUPTED: &str = "interrupted";

/// What a command that fails rejects with.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
pub struct CmdError {
    pub code: String,
    pub message: String,
}

impl CmdError {
    pub fn new(code: &str, message: impl Into<String>) -> CmdError {
        CmdError {
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn no_case() -> CmdError {
        CmdError::new(NO_CASE, "no case is open")
    }

    pub fn busy() -> CmdError {
        CmdError::new(
            BUSY,
            "another change to the case is running; try again when it finishes",
        )
    }

    pub fn superseded() -> CmdError {
        CmdError::new(SUPERSEDED, "a newer query replaced this one")
    }

    pub fn interrupted() -> CmdError {
        CmdError::new(INTERRUPTED, "the query was cancelled")
    }

    pub fn invalid(message: impl Into<String>) -> CmdError {
        CmdError::new("invalid", message)
    }
}

impl From<EngineError> for CmdError {
    fn from(e: EngineError) -> CmdError {
        CmdError::new(e.code(), e.to_string())
    }
}

impl std::fmt::Display for CmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CmdError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_engine_error_keeps_its_code() {
        let e: CmdError = EngineError::NotFound("relation x".into()).into();
        assert_eq!(e.code, "not_found");
        assert_eq!(e.message, "not found: relation x");
        let e: CmdError = EngineError::Internal("broken".into()).into();
        assert_eq!(e.code, "internal");
    }
}
