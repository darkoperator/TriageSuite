//! TypeScript stand-ins for the suite's own types that the app's contract
//! carries (feature `ts` only).
//!
//! `SqlType` and `TimeSemantics` live in `triage-core`, which has no `ts-rs`
//! and gets none: a field of either type names its stand-in here with
//! `#[ts(as = …)]`. Each stand-in is the union of the spellings the suite's
//! own `Serialize` writes (`SqlType::sql`, `TimeSemantics::as_str`), and the
//! tests below hold it to that `Serialize`, not to a list written beside it.

/// A column's declared SQL type, as `triage_core`'s `SqlType` serialises.
#[derive(ts_rs::TS)]
#[ts(rename = "SqlType")]
pub enum SqlTypeTs {
    #[ts(rename = "TIMESTAMP")]
    Timestamp,
    #[ts(rename = "BIGINT")]
    BigInt,
    #[ts(rename = "UBIGINT")]
    UBigInt,
    #[ts(rename = "BOOLEAN")]
    Boolean,
}

/// How a timestamp column is zoned, as `triage_core`'s `TimeSemantics`
/// serialises.
#[derive(ts_rs::TS)]
#[ts(rename = "TimeSemantics", rename_all = "lowercase")]
pub enum TimeSemanticsTs {
    Utc,
    Local,
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use triage_core::output::duckdb::types::{SqlType, TimeSemantics};
    use ts_rs::{Config, TS};

    /// The string literals of a TypeScript union of them, as ts-rs writes
    /// one (`"A" | "B"`).
    fn union_members(union: &str) -> BTreeSet<String> {
        union
            .split('|')
            .map(|m| {
                let m = m.trim();
                assert!(
                    m.len() >= 2 && m.starts_with('"') && m.ends_with('"'),
                    "{m:?} in {union:?} is not a string literal"
                );
                m[1..m.len() - 1].to_string()
            })
            .collect()
    }

    /// What `Serialize` writes for each of `all`, as plain strings.
    fn serialised<T: serde::Serialize>(all: &[T]) -> BTreeSet<String> {
        all.iter()
            .map(|t| match serde_json::to_value(t).unwrap() {
                serde_json::Value::String(s) => s,
                other => panic!("serialises as {other}, not a string"),
            })
            .collect()
    }

    // The source of truth is triage-core's own `Serialize` impls
    // (crates/triage-core/src/output/duckdb/types.rs: `SqlType` writes
    // `sql()`, `TimeSemantics` writes `as_str()`), run over each type's
    // `ALL`, which lists every variant once.
    #[test]
    fn sql_type_matches_the_suites_serialisation() {
        let cfg = Config::new();
        let ts = union_members(&SqlTypeTs::inline(&cfg));
        assert_eq!(ts, serialised(&SqlType::ALL));
        assert_eq!(ts.len(), SqlType::ALL.len());
    }

    #[test]
    fn time_semantics_matches_the_suites_serialisation() {
        let cfg = Config::new();
        let ts = union_members(&TimeSemanticsTs::inline(&cfg));
        assert_eq!(ts, serialised(&TimeSemantics::ALL));
        assert_eq!(ts.len(), TimeSemantics::ALL.len());
    }
}
