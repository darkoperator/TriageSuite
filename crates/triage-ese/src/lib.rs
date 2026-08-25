//! Read-only ESE/JET-Blue access for the suite. `header` sniffs the format
//! revision without loading; `db` wraps `ese_parser_lib` for parsing.

pub mod db;
pub mod header;

pub use db::{Database, EseColumn, EseError, EseValue};
