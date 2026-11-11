//! The OxideLens engine: a case file over TriageSuite output.
//!
//! A case is one DuckDB database. The engine loads a run's CSV output into
//! it, keeps the analyst's marks, tags and notes beside the rows, and
//! answers the grid, timeline and search queries the CLI exposes. Evidence
//! and run output are only ever read; the case file is the one thing the
//! engine writes.

pub mod actor;
pub mod case;
pub mod catalog;
pub mod error;
pub mod filter;
pub mod load;
pub mod search;
pub mod time;

pub use actor::Actor;
pub use case::{Case, CaseInfo, CASE_FORMAT};
pub use catalog::{
    columns, declare_type, relation, relations, set_primary_time, ColumnInfo, ColumnRole,
    RelationInfo, RelationKind, SqlType, TimeSemantics, TypeOrigin,
};
pub use error::{EngineError, Result};
pub use filter::{compile_filter, parse_filter, FilterNode, Leaf, Op, SqlFragment, MARK_COLUMN};
pub use load::{
    load_file, load_run, relation_name_for_file, remove_source, sources, FileOptions, LoadReport,
    LoadedRelation, RemoveReport, SourceInfo, Superseded,
};
pub use search::{compile_search, parse_search, Search, SearchExpr, SearchOptions, Term};
pub use time::{now_iso, parse_utc_text};
