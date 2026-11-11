//! What the app knows about datasets that the suite does not say itself:
//! columns the suite leaves as text but whose shape the app can type, and
//! which column a relation's timeline runs on.

use super::{SqlType, TimeSemantics};

/// A column the app types at load, beside the suite's own typing.
#[derive(Debug, Clone, Copy)]
pub struct AppType {
    pub relation: &'static str,
    pub column: &'static str,
    pub sql_type: SqlType,
    pub rule: AppRule,
}

/// Which values of the column the app casts, and what it claims about them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppRule {
    /// Cast only a value whose text ends in `Z`, and catalogue the column as
    /// UTC. A value without the `Z` names no zone, so it stays untyped
    /// rather than being read as UTC on a guess.
    UtcWhenTextEndsInZ,
}

impl AppRule {
    /// What the rule claims about the values it casts.
    pub fn time_semantics(&self) -> Option<TimeSemantics> {
        match self {
            AppRule::UtcWhenTextEndsInZ => Some(TimeSemantics::Utc),
        }
    }
}

/// Every column the app types. Hayabusa writes its timestamps as text the
/// suite cannot declare a zone for in general; with a trailing `Z` it is UTC.
pub const APP_TYPES: &[AppType] = &[AppType {
    relation: "ext_hayabusa_csv_timeline",
    column: "Timestamp",
    sql_type: SqlType::Timestamp,
    rule: AppRule::UtcWhenTextEndsInZ,
}];

/// The column each relation's timeline runs on by default, when it is a
/// catalogued timestamp. A relation not listed here takes its first
/// catalogued timestamp column.
pub const PRIMARY_TIME: &[(&str, &str)] = &[
    ("evtxtriage_events", "TimeCreated"),
    ("mftriage_mft", "Created0x10"),
    ("mftriage_usn", "UpdateTimestamp"),
    ("petriage_timeline", "RunTime"),
    ("rbtriage_main", "DeletedOn"),
    ("ext_hayabusa_csv_timeline", "Timestamp"),
];

/// The rule the app types `relation.column` by, if it does.
pub(crate) fn rule_for(relation: &str, column: &str) -> Option<AppRule> {
    APP_TYPES
        .iter()
        .find(|t| t.relation == relation && t.column == column)
        .map(|t| t.rule)
}

/// The `PRIMARY_TIME` entry for `relation`, if it has one.
pub(crate) fn primary_time_for(relation: &str) -> Option<&'static str> {
    PRIMARY_TIME
        .iter()
        .find(|(r, _)| *r == relation)
        .map(|(_, c)| *c)
}
