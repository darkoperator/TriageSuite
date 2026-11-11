//! The OxideLens engine: a case file over TriageSuite output.
//!
//! A case is one DuckDB database. The engine loads a run's CSV output into
//! it, keeps the analyst's marks, tags and notes beside the rows, and
//! answers the grid, timeline and search queries the CLI exposes. Evidence
//! and run output are only ever read; the case file is the one thing the
//! engine writes.

pub mod actor;
pub mod annot;
pub mod case;
pub mod catalog;
pub mod error;
pub mod filter;
pub mod listing;
pub mod load;
pub mod query;
pub mod search;
pub mod time;

pub use actor::Actor;
pub use annot::{
    add_note, add_tag, clear_suspicious, delete_note, history, mark_suspicious, marks, notes,
    remove_tag, row_detail, tags, HistoryRow, MarkedRow, Note, RowDetail, RowOutcome, TagInfo,
    AGENT_ROWS_PER_CALL, AGENT_TAG_PREFIX, MAX_NOTE, MAX_TAG,
};
pub use case::{Case, CaseInfo, CASE_FORMAT};
pub use catalog::{
    columns, declare_type, relation, relations, set_primary_time, ColumnInfo, ColumnRole,
    RelationInfo, RelationKind, SqlType, TimeSemantics, TypeOrigin,
};
pub use error::{EngineError, Result};
pub use filter::{compile_filter, parse_filter, FilterNode, Leaf, Op, SqlFragment, MARK_COLUMN};
pub use listing::Listing;
pub use load::{
    load_file, load_run, relation_name_for_file, remove_source, sources, FileOptions, LoadReport,
    LoadedRelation, RemoveReport, SourceInfo, Superseded,
};
pub use query::{
    anchor_of, around, count, describe, find, group_filter, groups, histogram, merged_rows, page,
    query_sql, stack, AroundCount, Bucket, Cell, Described, DescribedColumn, FindHit, FindMode,
    FindRequest, FindResult, GridQuery, GroupRow, Histogram, MarkState, MergedRow, MergedSource,
    Page, SortKey, SqlResult, Stack, StackOrder, StackRow, BUCKET_LADDER, CELL_CAP, DEFAULT_LIMIT,
    MAX_LIMIT, MAX_VALUE, SQL_MEMORY_LIMIT,
};
pub use search::{compile_search, parse_search, Search, SearchExpr, SearchOptions, Term};
pub use time::{now_iso, parse_utc_text};
