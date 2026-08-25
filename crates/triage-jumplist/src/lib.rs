//! Windows Jump List parser (Automatic + Custom Destinations).

pub mod appid;
pub mod automatic;
pub mod custom;
pub mod destlist;

#[allow(dead_code)] // variants used in Tasks 3-4
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("not a valid jump list ({0})")]
    BadFormat(&'static str),
    #[error("truncated or corrupt structure: {0}")]
    Corrupt(&'static str),
    #[error("compound file error: {0}")]
    Compound(String),
}
