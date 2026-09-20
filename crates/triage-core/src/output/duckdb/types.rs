//! Declared SQL types for CSV columns.
//!
//! Only four types exist here, and each was measured against DuckDB 1.5.4 on
//! this suite's own output shapes before being admitted:
//!
//! - `TIMESTAMP` parses `1601-01-01 00:00:00.0000000`, which `TIMESTAMP_NS`
//!   does not -- its range starts around 1677, so it nulls the FILETIME
//!   epoch silently. Microsecond precision truncates the seventh fractional
//!   digit, which is why every typed timestamp column keeps its original
//!   text alongside it.
//! - `UBIGINT` holds `u64::MAX`; signed `BIGINT` does not and yields NULL.
//! - `BOOLEAN` parses the `True`/`False` the suite emits, and a blank cell.
//!
//! A column whose CSV shape cannot be proven to be one of these stays
//! VARCHAR. There is no inference: an inferred type is a guess that can
//! differ between hosts and mangle evidence in silence.

/// A SQL type a column can be declared as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlType {
    Timestamp,
    BigInt,
    UBigInt,
    Boolean,
}

impl SqlType {
    /// Every variant, exactly once, with the array length spelled out.
    ///
    /// The length is deliberate: adding a fifth `SqlType` variant without
    /// adding it here fails to compile, instead of silently producing an
    /// inventory that `from_sql` -- and therefore `regenerate`'s read-back
    /// check -- would reject on sight. That check exists because a tampered
    /// `datasets.json` is untrusted input to a `TRY_CAST` string; it must
    /// stay in lockstep with `sql()`, and this is what keeps it there.
    pub const ALL: [SqlType; 4] = [
        SqlType::Timestamp,
        SqlType::BigInt,
        SqlType::UBigInt,
        SqlType::Boolean,
    ];

    /// The spelling pasted into a `TRY_CAST`.
    pub fn sql(&self) -> &'static str {
        match self {
            SqlType::Timestamp => "TIMESTAMP",
            SqlType::BigInt => "BIGINT",
            SqlType::UBigInt => "UBIGINT",
            SqlType::Boolean => "BOOLEAN",
        }
    }

    /// The inverse of `sql()`, derived from `ALL` rather than a second
    /// `match` over string literals -- a hand-written parser is exactly the
    /// duplication that let `sql()` and the read-back allow-list drift apart
    /// in the first place. Returns `None` for anything else, which is what
    /// lets a caller reading `EffectiveType::sql_type` back off disk reject
    /// a spelling that was never produced by `sql()`, tampered or not.
    pub fn from_sql(spelling: &str) -> Option<SqlType> {
        Self::ALL.into_iter().find(|t| t.sql() == spelling)
    }
}

/// Declared SQL types for datasets whose id is built at run time.
///
/// `DatasetColumnTypes` names one exact `dataset_id`, which a dynamic
/// dataset does not have: RETriage writes a plugin detail file per
/// `<Plugin>_<hive stem>` and EvtxTriage writes a per-channel export per
/// `Individual/<Channel>`, so the id varies with the evidence rather than
/// with the code. There is no constant to name, and a declaration keyed on
/// one would simply never match.
///
/// The prefix carries its own separator -- `"Services_"`, `"Individual/"` --
/// instead of the builder inferring one. The tools do not agree on a
/// separator, and writing the boundary explicitly is what stops a
/// `"Services"` declaration from also claiming a future `ServicesHub_SYSTEM`.
/// When several prefixes match one id the longest wins, so a specific
/// declaration can sit alongside a general one.
#[derive(Debug, Clone, Copy)]
pub struct DynamicColumnTypes {
    /// Matches a dynamic `dataset_id` that starts with this string.
    pub prefix: &'static str,
    pub columns: &'static [ColumnType],
}

/// What a timestamp column's values are relative to.
///
/// Declared by the tool that emits the column, never inferred. A column
/// whose zone cannot be established is left VARCHAR rather than typed as if
/// it were UTC, because a wrong zone on a forensic timeline is worse than no
/// zone at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeSemantics {
    Utc,
    Local,
    Unknown,
}

impl TimeSemantics {
    pub fn as_str(&self) -> &'static str {
        match self {
            TimeSemantics::Utc => "utc",
            TimeSemantics::Local => "local",
            TimeSemantics::Unknown => "unknown",
        }
    }
}

/// One declared column of one dataset.
#[derive(Debug, Clone, Copy)]
pub struct ColumnType {
    /// The column header exactly as the CSV writer emits it.
    pub column: &'static str,
    pub sql_type: SqlType,
    /// Set only for `SqlType::Timestamp`; `None` for every other type.
    pub time_semantics: Option<TimeSemantics>,
}

/// Every declared column of one dataset.
#[derive(Debug, Clone, Copy)]
pub struct DatasetColumnTypes {
    /// A `DatasetSpec::id`.
    pub dataset_id: &'static str,
    pub columns: &'static [ColumnType],
}

#[cfg(test)]
mod tests {
    use super::SqlType;

    #[test]
    fn from_sql_inverts_sql_for_every_variant() {
        for t in SqlType::ALL {
            assert_eq!(SqlType::from_sql(t.sql()), Some(t), "{t:?}");
        }
    }

    #[test]
    fn from_sql_rejects_an_unknown_spelling() {
        assert_eq!(SqlType::from_sql("NOT_A_TYPE"), None);
        assert_eq!(SqlType::from_sql("TIMESTAMP) AS x; DROP TABLE y; --"), None);
    }
}
