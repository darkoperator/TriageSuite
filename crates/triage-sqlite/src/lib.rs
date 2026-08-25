//! Read-only SQLite access for the suite. See `db::Database` for the
//! evidence-integrity open discipline and `value` for typed row reading.

mod db;
mod value;

pub use db::Database;
pub use value::SqliteValue;
