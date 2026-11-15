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
pub mod record;
pub mod search;
pub mod time;
#[cfg(feature = "ts")]
pub mod ts;
pub mod view;

pub use actor::Actor;
pub use annot::{
    add_note, add_tag, clear_suspicious, delete_note, history, mark_counts, mark_suspicious, marks,
    notes, remove_tag, row_detail, row_place, tags, HistoryRow, MarkCounts, MarkedRow, Note,
    RelationMarks, RowDetail, RowOutcome, RowPlace, RowStanding, TagInfo, AGENT_ROWS_PER_CALL,
    AGENT_TAG_PREFIX, MAX_NOTE, MAX_TAG,
};
pub use case::{
    display_zone, set_display_zone, Case, CaseInfo, CaseSize, CompactReport, CASE_FORMAT,
};
pub use catalog::{
    column_comments, columns, declare_type, host_zones, relation, relations, set_primary_time,
    ColumnInfo, ColumnRole, HostZone, RelationInfo, RelationKind, SqlType, TimeSemantics,
    TypeOrigin,
};
pub use error::{EngineError, Result};
pub use filter::{compile_filter, parse_filter, FilterNode, Leaf, Op, SqlFragment, MARK_COLUMN};
pub use listing::Listing;
pub use load::{
    load_file, load_run, load_run_with, relation_name_for_file, remove_source, sessions, sources,
    FileOptions, LoadReport, LoadedRelation, RemoveReport, RunOptions, Session, SessionRelation,
    SkippedDataset, SourceInfo, Superseded,
};
pub use query::{
    anchor_of, around, count, describe, find, group_filter, groups, histogram, merged_rows, page,
    query_sql, query_sql_within, stack, AroundCount, Bucket, Cell, Described, DescribedColumn,
    FindHit, FindMode, FindRequest, FindResult, GridQuery, GroupRow, Histogram, MarkState,
    MergedRow, MergedSource, Page, RowAnchor, SortKey, SqlResult, Stack, StackOrder, StackRow,
    BUCKET_LADDER, CELL_CAP, DEFAULT_LIMIT, MAX_LIMIT, MAX_VALUE, SQL_MEMORY_LIMIT, SQL_TEXT_MAX,
    SQL_TIME_LIMIT, SQL_TIME_LIMIT_MAX,
};
pub use record::{
    account_record, accounts, attach_rows, candidates, delete_entry, delete_record, detach_row,
    edit_entry, entries, entry, evidence, export_report, export_timeline_csv, export_workbook,
    guess_ioc_type, host_record, hosts, ioc_record, ioc_sightings, iocs, link, logon_summary,
    merge_records, new_entry, promote_rows, record_entries, set_entry_status, set_record_status,
    unlink, upsert_account, upsert_host, upsert_ioc, validate_attack_id, validate_ioc,
    validate_sid, AccountRecord, AccountSpec, Candidate, Entry, EntryEdit, EntryQuery, EntryStatus,
    Evidence, EvidenceStanding, HostRecord, HostSpec, IocQuery, IocRecord, IocSpec, IocType,
    LogonCount, LogonOutcome, LogonSummary, Privilege, PromoteOutcome, RecordKind, RecordStatus,
    Sightings, MAX_DESCRIPTION, MAX_IOC_VALUE, MAX_RECORD_LABEL, MAX_RECORD_NAME, MAX_RECORD_NOTE,
    MAX_TECHNIQUES, MAX_TITLE, TACTICS,
};
#[cfg(feature = "test-support")]
pub use record::{export_report_paused, export_timeline_csv_paused, export_workbook_paused};
pub use search::{compile_search, parse_search, Search, SearchExpr, SearchOptions, Term};
pub use time::{now_iso, parse_utc_text};
pub use view::{
    apply_view, around_totals, cell_value, check_export_target, count_view, create_export_partial,
    date_range_chip, default_colour_rules, delete_view, empty_columns, export_csv,
    export_csv_guarded, export_view, groups_view, histogram_view, import_view, match_count,
    matches, merged_view, page_view, row_ids_at, row_offset, row_positions, row_view, save_view,
    split_value, values, views, views_matching, AppliedView, CellValue, Chip, ChipOrigin,
    ColourRule, ColumnLayout, ColumnMeta, ExportReport, FieldView, GridCell, GridCounts, GridPage,
    GridState, GroupView, MatchPos, MatchingView, MergedView, PageOptions, Provenance, RowView,
    RuleColour, RuleScope, SavedView, SearchMode, SearchState, SkippedView, ValueCount, ValueOrder,
    ValuePart, ViewDoc, ViewKind, ViewListing, ViewWarning, WindowTotal, MAX_ACTION_ROWS,
    MAX_CHIPS, MAX_CHIP_ID, MAX_CHIP_LABEL, MAX_COLUMN_WIDTH, MAX_PARTS, MAX_PIVOT_WINDOWS,
    MAX_RULES, MAX_RULE_NAME, MAX_VIEW_BYTES, MAX_VIEW_FILE_BYTES, MAX_VIEW_NAME, MAX_VIEW_PATTERN,
    MAX_WINDOW_SECONDS, MIN_COLUMN_WIDTH,
};
