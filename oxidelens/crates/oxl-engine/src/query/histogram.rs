//! The time histogram above the grid.

use super::group::{as_count, primary_time_semantics, typed_primary_time};
use super::{find_column, marked_sql, scope, GridQuery, ISO_FORMAT, ROW_ALIAS};
use crate::case::Case;
use crate::catalog::{self, SqlType, TimeSemantics};
use crate::error::{EngineError, Result};
use duckdb::params_from_iter;
use triage_core::output::duckdb::sql::quote_ident;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Bucket {
    /// UTC start of the bucket.
    pub start: String,
    pub count: u64,
    pub suspicious: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Histogram {
    pub column: String,
    /// How `column` is zoned. Buckets of an `unknown` time are aligned to
    /// the epoch as if it were UTC.
    #[serde(serialize_with = "crate::catalog::serialize_semantics")]
    pub time_semantics: Option<TimeSemantics>,
    pub bucket_seconds: u64,
    /// Non-empty buckets only, oldest first.
    pub buckets: Vec<Bucket>,
    /// Filtered rows whose time is empty or did not convert.
    pub untimed: u64,
}

/// Bucket widths, in seconds: a second to thirty days.
pub const BUCKET_LADDER: [u64; 10] = [
    1, 10, 60, 300, 900, 3600, 21_600, 86_400, 604_800, 2_592_000,
];

const MICROS: u64 = 1_000_000;

/// `column` None = the relation's primary time. Picks the smallest ladder
/// step giving at most `max_buckets` buckets over the filtered span, or the
/// widest step when none does. Buckets are aligned to the Unix epoch, in UTC.
/// `Invalid` for a relation with no timestamp column to bucket, a column that
/// is not a typed timestamp, or `max_buckets` of 0.
pub fn histogram(
    case: &Case,
    q: &GridQuery,
    column: Option<&str>,
    max_buckets: u32,
) -> Result<Histogram> {
    if max_buckets == 0 {
        return Err(EngineError::Invalid(
            "a histogram needs at least one bucket".into(),
        ));
    }
    let scope = scope(case, q)?;
    let all = catalog::columns(case, &q.relation)?;
    let relation = &scope.relation.name;
    let (name, typed, time_semantics) = match column {
        Some(name) => {
            let c = find_column(&all, relation, name)?;
            match (&c.typed_column, c.sql_type) {
                (Some(typed), Some(SqlType::Timestamp)) => {
                    (c.name.clone(), typed.clone(), c.time_semantics)
                }
                _ => {
                    return Err(EngineError::Invalid(format!(
                        "{relation}.{name} is not a timestamp column"
                    )))
                }
            }
        }
        None => match (
            &scope.relation.primary_time,
            typed_primary_time(&scope.relation.primary_time, &all),
        ) {
            (Some(name), Some(typed)) => (
                name.clone(),
                typed.to_string(),
                primary_time_semantics(&scope.relation.primary_time, &all),
            ),
            _ => {
                return Err(EngineError::Invalid(format!(
                    "{relation} has no timestamp column to bucket"
                )))
            }
        },
    };
    let t = format!("{ROW_ALIAS}.{}", quote_ident(&typed));
    let conn = case.conn();

    // The span of the filtered rows in microseconds since the epoch, as SQL
    // reports it, and how many rows have no time.
    let span_sql = format!(
        "SELECT min(epoch_us({t})), max(epoch_us({t})), \
         count(*) FILTER (WHERE {t} IS NULL) FROM {} WHERE {}",
        scope.from_sql, scope.where_sql
    );
    let (lo, hi, untimed): (Option<i64>, Option<i64>, i64) =
        conn.query_row(&span_sql, params_from_iter(&scope.params), |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    let mut out = Histogram {
        column: name,
        time_semantics,
        bucket_seconds: BUCKET_LADDER[0],
        buckets: Vec::new(),
        untimed: as_count(untimed),
    };
    let (Some(lo), Some(hi)) = (lo, hi) else {
        return Ok(out);
    };

    // The smallest step whose epoch-aligned buckets cover the span in at
    // most `max_buckets`. The bucket arithmetic stays in SQL.
    let steps = BUCKET_LADDER
        .iter()
        .map(|s| format!("(CAST({s} AS BIGINT))"))
        .collect::<Vec<_>>()
        .join(", ");
    let fits = format!(
        "SELECT min(s) FROM (VALUES {steps}) AS ox_l(s), \
         (SELECT CAST(? AS BIGINT) AS hi, CAST(? AS BIGINT) AS lo) AS ox_sp \
         WHERE ({} - {}) / (CAST(s AS BIGINT) * {MICROS}) + 1 <= ?",
        floor_us("ox_sp.hi", "s"),
        floor_us("ox_sp.lo", "s"),
    );
    let step: Option<i64> = conn.query_row(
        &fits,
        duckdb::params![hi, lo, i64::from(max_buckets)],
        |r| r.get(0),
    )?;
    let step = step
        .and_then(|s| u64::try_from(s).ok())
        .unwrap_or(BUCKET_LADDER[BUCKET_LADDER.len() - 1]);
    out.bucket_seconds = step;

    let sql = format!(
        "SELECT strftime(ox_b, '{ISO_FORMAT}'), count(*), \
         count(*) FILTER (WHERE ox_marked) FROM (\
           SELECT make_timestamp({}) AS ox_b, {} AS ox_marked \
           FROM {} WHERE ({}) AND {t} IS NOT NULL) \
         GROUP BY ox_b ORDER BY ox_b",
        floor_us(&format!("epoch_us({t})"), &step.to_string()),
        marked_sql(),
        scope.from_sql,
        scope.where_sql,
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(&scope.params))?;
    while let Some(row) = rows.next()? {
        out.buckets.push(Bucket {
            start: row.get(0)?,
            count: as_count(row.get(1)?),
            suspicious: as_count(row.get(2)?),
        });
    }
    Ok(out)
}

/// `us` rounded down to a multiple of `step_seconds`, in microseconds. The
/// modulo is made non-negative first, so instants before 1970 round down too.
fn floor_us(us: &str, step_seconds: &str) -> String {
    let m = format!("(CAST({step_seconds} AS BIGINT) * {MICROS})");
    format!("({us} - ((({us} % {m}) + {m}) % {m}))")
}
