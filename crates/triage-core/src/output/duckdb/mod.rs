//! Generation of a DuckDB view layer over a run's CSV output.
//!
//! Nothing here writes evidence. It reads back what the router published,
//! describes it in an inventory, and renders SQL that is a pure function of
//! that inventory -- which is what makes the renderer testable without a
//! filesystem and the SQL regenerable after a collection moves.

pub mod build;
pub mod header;
pub mod inventory;
pub mod render;
pub mod sql;
pub mod types;
