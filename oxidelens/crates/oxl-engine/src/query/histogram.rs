//! The time histogram above the grid, bucketed and labelled in a display
//! zone.
//!
//! How the buckets line up:
//!
//! - **Under a day** (a second to six hours): aligned to the Unix epoch in
//!   UTC when the zone's offset over the span is a whole number of widths
//!   (any width up to an hour in a whole-hour zone), so the buckets are the
//!   same instants in every such zone and only their labels differ.
//!   Otherwise aligned to the local clock: six-hour buckets in New York
//!   start at local 00:00, 06:00, 12:00 and 18:00.
//! - **A day, a week, thirty days**: a day starts at local midnight, a week
//!   at local midnight on a Monday, a "thirty-day" bucket at local midnight
//!   on the 1st of a month (so it is a calendar month, of 28 to 31 days).
//!   Each start comes from `zone::local_midnights`, so a day the clocks
//!   change on starts where the zone says it does.
//!
//! Only a `utc` column is bucketed in the zone. An `unknown` or `local`
//! column is bucketed as written, as if its text were UTC, and its labels are
//! its times as written.
//!
//! Every offset comes from `triage_core::timestamp::zone`: the engine asks
//! it for the offset at sampled instants and narrows each change to the
//! second, then does plain arithmetic on wall-clock microseconds.

use super::group::{as_count, primary_time_semantics, typed_primary_time};
use super::{find_column, marked_sql, scope, GridQuery, ROW_ALIAS};
use crate::case::Case;
use crate::catalog::{self, SqlType, TimeSemantics};
use crate::error::{EngineError, Result};
use chrono::{DateTime, Datelike, Days, Months, NaiveDate, NaiveDateTime};
use duckdb::params_from_iter;
use triage_core::output::duckdb::sql::quote_ident;
use triage_core::timestamp::zone;

#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Bucket {
    /// UTC start of the bucket (R1).
    pub start: String,
    /// The start on the display zone's clock, `YYYY-MM-DD HH:MM:SS`; as
    /// written for a column that is not `utc`.
    pub label: String,
    /// The bucket's last whole second, in the same form: a date-range chip
    /// from `label` to `end_label` selects exactly the bucket.
    pub end_label: String,
    pub count: u64,
    pub suspicious: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Histogram {
    pub column: String,
    /// How `column` is zoned. Only a `utc` column is bucketed in `zone`; an
    /// `unknown` one is bucketed as written, as if it were UTC.
    #[cfg_attr(feature = "ts", ts(as = "Option<crate::ts::TimeSemanticsTs>"))]
    pub time_semantics: Option<TimeSemantics>,
    /// The display zone asked for.
    pub zone: String,
    /// The bucket width: one of `BUCKET_LADDER`. A day, a week and thirty
    /// days are calendar units on the local clock, so a bucket of them can
    /// be an hour shorter or longer (a DST day) or a calendar month.
    pub bucket_seconds: u64,
    /// Non-empty buckets only, oldest first.
    pub buckets: Vec<Bucket>,
    /// Filtered rows whose time is empty or did not convert.
    pub untimed: u64,
}

/// Bucket widths, in seconds: a second to thirty days (a calendar month).
pub const BUCKET_LADDER: [u64; 10] = [
    1, 10, 60, 300, 900, 3600, 21_600, 86_400, 604_800, 2_592_000,
];

const MICROS: i64 = 1_000_000;
const DAY: u64 = 86_400;
const WEEK: u64 = 604_800;
const DAY_US: i64 = 86_400 * MICROS;
const HOUR_US: i64 = 3_600 * MICROS;

/// `column` None = the relation's primary time. Picks the smallest ladder
/// step giving at most `max_buckets` buckets over the filtered span, or the
/// widest step when none does; the buckets are aligned as the module says.
/// `Invalid` for a relation with no timestamp column to bucket, a column that
/// is not a typed timestamp, `max_buckets` of 0, or a zone this build does
/// not know.
pub fn histogram(
    case: &Case,
    q: &GridQuery,
    column: Option<&str>,
    max_buckets: u32,
    zone: &str,
) -> Result<Histogram> {
    if max_buckets == 0 {
        return Err(EngineError::Invalid(
            "a histogram needs at least one bucket".into(),
        ));
    }
    super::check_zone(zone)?;
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
    // The clock the buckets follow: the zone for a UTC column; UTC, which is
    // to say the text as written, for any other.
    let clock = if time_semantics == Some(TimeSemantics::Utc) {
        zone
    } else {
        "UTC"
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
        zone: zone.to_string(),
        bucket_seconds: BUCKET_LADDER[0],
        buckets: Vec::new(),
        untimed: as_count(untimed),
    };
    let (Some(lo), Some(hi)) = (lo, hi) else {
        return Ok(out);
    };

    let offsets = Offsets::over(lo, hi, clock);
    let step = BUCKET_LADDER
        .iter()
        .copied()
        .find(|&s| bucket_count(s, lo, hi, &offsets) <= u64::from(max_buckets))
        .unwrap_or(BUCKET_LADDER[BUCKET_LADDER.len() - 1]);
    out.bucket_seconds = step;
    let step_us = step_us(step);
    let epoch = offsets.epoch_aligned(lo, hi, step);

    // Each row's bucket key: its UTC instant floored to the width (epoch
    // alignment), or its wall-clock time floored to the width, or to the
    // day for a day and wider (days are gathered into weeks and months
    // below). The offset in force at each row is looked up by an ASOF join
    // on the offset changes over the span; the values are numbers the
    // engine computed, never input.
    let span = offsets.within(lo, hi);
    let (wall, join) = if epoch {
        ("ox_x.ox_us".to_string(), String::new())
    } else if let [(_, only)] = span.as_slice() {
        (
            format!("(ox_x.ox_us + CAST({only} AS BIGINT))"),
            String::new(),
        )
    } else {
        let changes = span
            .iter()
            .map(|(at, offset)| format!("(CAST({at} AS BIGINT), CAST({offset} AS BIGINT))"))
            .collect::<Vec<_>>()
            .join(", ");
        (
            "(ox_x.ox_us + ox_z.zo)".to_string(),
            format!(" ASOF JOIN (VALUES {changes}) AS ox_z(zs, zo) ON ox_x.ox_us >= ox_z.zs"),
        )
    };
    let unit = if step < DAY { step_us } else { DAY_US };
    let sql = format!(
        "SELECT ox_k, count(*), count(*) FILTER (WHERE ox_marked) FROM (\
           SELECT {key} AS ox_k, ox_x.ox_marked AS ox_marked FROM (\
             SELECT epoch_us({t}) AS ox_us, {marked} AS ox_marked \
             FROM {from} WHERE ({where_sql}) AND {t} IS NOT NULL) AS ox_x{join}) AS ox_y \
         GROUP BY ox_k ORDER BY ox_k",
        key = floor_us(&wall, unit),
        marked = marked_sql(),
        from = scope.from_sql,
        where_sql = scope.where_sql,
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(&scope.params))?;
    let mut keyed: Vec<(i64, u64, u64)> = Vec::new();
    while let Some(row) = rows.next()? {
        let key: i64 = row.get(0)?;
        let (count, suspicious) = (as_count(row.get(1)?), as_count(row.get(2)?));
        // Days gather into the week or month that holds them; the keys come
        // in order, so a bucket's days are adjacent.
        let key = if step > DAY {
            calendar_start(key, step).unwrap_or(key)
        } else {
            key
        };
        match keyed.last_mut() {
            Some(last) if last.0 == key => {
                last.1 += count;
                last.2 += suspicious;
            }
            _ => keyed.push((key, count, suspicious)),
        }
    }

    for (key, count, suspicious) in keyed {
        let (start, next) = if epoch {
            (key, key.saturating_add(step_us))
        } else if step < DAY {
            (
                offsets.start_of_local(key),
                offsets.start_of_local(key.saturating_add(step_us)),
            )
        } else {
            day_bounds(key, step, &offsets, clock)
        };
        out.buckets.push(Bucket {
            start: r1(start),
            label: label(start, &offsets, clock),
            end_label: label(next.saturating_sub(MICROS).max(start), &offsets, clock),
            count,
            suspicious,
        });
    }
    Ok(out)
}

fn step_us(step: u64) -> i64 {
    i64::try_from(step).map_or(i64::MAX, |s| s.saturating_mul(MICROS))
}

/// How many buckets of `step` the span `lo..=hi` (UTC microseconds) touches,
/// aligned as the histogram would align them.
fn bucket_count(step: u64, lo: i64, hi: i64, offsets: &Offsets) -> u64 {
    let (wall_lo, wall_hi) = (
        lo.saturating_add(offsets.at(lo)),
        hi.saturating_add(offsets.at(hi)),
    );
    if step < DAY {
        let width = i128::from(step_us(step));
        let (a, b) = if offsets.epoch_aligned(lo, hi, step) {
            (lo, hi)
        } else {
            (wall_lo, wall_hi)
        };
        let floor = |v: i64| i128::from(v).div_euclid(width);
        let n = (floor(b) - floor(a) + 1).max(1);
        return u64::try_from(n).unwrap_or(u64::MAX);
    }
    let (Some(first), Some(last)) = (wall_date(wall_lo), wall_date(wall_hi)) else {
        return u64::MAX;
    };
    let n = match step {
        DAY => (last - first).num_days() + 1,
        WEEK => (monday(last) - monday(first)).num_days() / 7 + 1,
        _ => {
            let month = |d: NaiveDate| i64::from(d.year()) * 12 + i64::from(d.month0());
            month(last) - month(first) + 1
        }
    };
    u64::try_from(n.max(1)).unwrap_or(u64::MAX)
}

/// The wall-clock start (microseconds) of the week or month holding the
/// local day that starts at the wall-clock microseconds `day`: its Monday,
/// or its 1st.
fn calendar_start(day: i64, step: u64) -> Option<i64> {
    let date = wall_date(day)?;
    let start = if step == WEEK {
        monday(date)
    } else {
        date.with_day(1)?
    };
    date_us(start)
}

/// The UTC start of the day, week or month whose first local day starts at
/// the wall-clock microseconds `key`, and the start of the one after it.
///
/// Both come from `zone::local_midnights` over the bucket alone: from an
/// instant on its first day (local noon, which is on that day whatever the
/// clock did at midnight) to an instant on the next bucket's first day. The
/// first start listed is the bucket's, the last the next bucket's. Should
/// that not hold (a zone that skips a whole day), the starts are worked out
/// from the offsets instead.
fn day_bounds(key: i64, step: u64, offsets: &Offsets, clock: &str) -> (i64, i64) {
    let fallback =
        |key: i64, next: i64| (offsets.start_of_local(key), offsets.start_of_local(next));
    let Some(first) = wall_date(key) else {
        return fallback(key, key.saturating_add(DAY_US));
    };
    let next_day = match step {
        DAY => first.checked_add_days(Days::new(1)),
        WEEK => first.checked_add_days(Days::new(7)),
        _ => first.checked_add_months(Months::new(1)),
    };
    let Some(next_key) = next_day.and_then(date_us) else {
        return fallback(key, key.saturating_add(DAY_US));
    };
    let noon = |wall: i64| offsets.start_of_local(wall.saturating_add(12 * HOUR_US));
    let days = usize::try_from((next_key - key) / DAY_US).unwrap_or(0) + 2;
    let starts = zone::local_midnights(&r1(noon(key)), &r1(noon(next_key)), clock, 1, days);
    let instant = |text: Option<&String>| text.and_then(|t| parse_r1(t));
    match (instant(starts.first()), instant(starts.last())) {
        (Some(start), Some(next))
            if starts.len() >= 2
                && wall_date(start.saturating_add(offsets.at(start))) == Some(first) =>
        {
            (start, next)
        }
        _ => fallback(key, next_key),
    }
}

/// The instant `us` on `clock`, `YYYY-MM-DD HH:MM:SS`.
fn label(us: i64, offsets: &Offsets, clock: &str) -> String {
    zone::render_utc_text(&r1(us), clock)
        .map(|r| r.local.chars().take(19).collect())
        .unwrap_or_else(|| wall_text(us.saturating_add(offsets.at(us))))
}

/// R1 text of the UTC instant `us` microseconds after the epoch; empty for
/// an instant chrono cannot hold (past year 262143).
fn r1(us: i64) -> String {
    DateTime::from_timestamp_micros(us)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string())
        .unwrap_or_default()
}

fn parse_r1(text: &str) -> Option<i64> {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.fZ")
        .ok()
        .map(|t| t.and_utc().timestamp_micros())
}

/// Wall-clock microseconds as `YYYY-MM-DD HH:MM:SS`.
fn wall_text(us: i64) -> String {
    DateTime::from_timestamp_micros(us)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

fn wall_date(us: i64) -> Option<NaiveDate> {
    DateTime::from_timestamp_micros(us).map(|d| d.date_naive())
}

fn date_us(date: NaiveDate) -> Option<i64> {
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_micros())
}

fn monday(date: NaiveDate) -> NaiveDate {
    let back = u64::from(date.weekday().num_days_from_monday());
    date.checked_sub_days(Days::new(back)).unwrap_or(date)
}

/// `us` (SQL) rounded down to a multiple of `unit_us` microseconds. The
/// modulo is made non-negative first, so instants before 1970 round down
/// too.
fn floor_us(us: &str, unit_us: i64) -> String {
    let m = format!("CAST({unit_us} AS BIGINT)");
    format!("({us} - ((({us} % {m}) + {m}) % {m}))")
}

/// A zone's offset from UTC over a span, as the changes `zone` reports: each
/// entry is the UTC instant (microseconds) from which an offset (also
/// microseconds) holds, in order. The first holds from the beginning of
/// time as far as this span is concerned.
struct Offsets {
    changes: Vec<(i64, i64)>,
}

/// Instants sampled for offset changes, at most, however long the span.
const MAX_SAMPLES: i64 = 200_000;
/// How far past each end of the span the offsets are read, so a bucket that
/// starts before the first row or ends after the last (a month) is covered.
const MARGIN_SECONDS: i64 = 40 * 86_400;

impl Offsets {
    /// The offsets of `clock` over `lo..=hi` (UTC microseconds) and a margin
    /// either side. The offset is read once a day (more sparsely over a
    /// span of more than `MAX_SAMPLES` days); a change between two readings
    /// is narrowed to the second by bisection, as tzdb's transitions fall
    /// on whole seconds. Two changes within one day that cancel out are not
    /// seen; no zone has had such a day.
    fn over(lo: i64, hi: i64, clock: &str) -> Offsets {
        if clock == "UTC" {
            return Offsets {
                changes: vec![(i64::MIN, 0)],
            };
        }
        let at = |second: i64| -> Option<i64> {
            let text = DateTime::from_timestamp(second, 0)?
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string();
            zone::utc_offset_seconds(&text, clock).map(i64::from)
        };
        let from = lo.div_euclid(MICROS).saturating_sub(MARGIN_SECONDS);
        let to = hi.div_euclid(MICROS).saturating_add(MARGIN_SECONDS);
        let step = (to.saturating_sub(from) / MAX_SAMPLES).max(86_400);
        let mut current = at(from).unwrap_or(0);
        let mut changes = vec![(from, current)];
        let mut t = from;
        while t < to {
            let next = t.saturating_add(step).min(to);
            match at(next) {
                Some(offset) if offset != current => {
                    let (mut low, mut high) = (t, next);
                    while high - low > 1 {
                        let middle = low + (high - low) / 2;
                        if at(middle).is_none_or(|o| o == current) {
                            low = middle;
                        } else {
                            high = middle;
                        }
                    }
                    current = at(high).unwrap_or(offset);
                    changes.push((high, current));
                    t = high;
                }
                _ => t = next,
            }
        }
        let mut changes: Vec<(i64, i64)> = changes
            .into_iter()
            .map(|(s, o)| (s.saturating_mul(MICROS), o.saturating_mul(MICROS)))
            .collect();
        changes[0].0 = i64::MIN;
        Offsets { changes }
    }

    /// The offset in force at the UTC instant `us`.
    fn at(&self, us: i64) -> i64 {
        let i = self.changes.partition_point(|(from, _)| *from <= us);
        self.changes[i.saturating_sub(1)].1
    }

    /// The changes in force at some instant of `lo..=hi`, the first from `lo`.
    fn within(&self, lo: i64, hi: i64) -> Vec<(i64, i64)> {
        let mut out = Vec::new();
        for (i, &(from, offset)) in self.changes.iter().enumerate() {
            let until = self.changes.get(i + 1).map_or(i64::MAX, |n| n.0);
            if until <= lo || from > hi {
                continue;
            }
            out.push((from.max(lo), offset));
        }
        out
    }

    /// Whether buckets of `step` (under a day) keep the epoch's alignment
    /// over `lo..=hi`: every offset in force is a whole number of them.
    fn epoch_aligned(&self, lo: i64, hi: i64, step: u64) -> bool {
        let width = step_us(step);
        step < DAY && self.within(lo, hi).iter().all(|(_, o)| o % width == 0)
    }

    /// The first UTC instant at which the clock reads the wall-clock
    /// microseconds `wall` or later: the earlier one when it read `wall`
    /// twice, the end of the gap when it skipped it.
    fn start_of_local(&self, wall: i64) -> i64 {
        for (i, &(from, offset)) in self.changes.iter().enumerate() {
            if i > 0 && wall < from.saturating_add(offset) {
                return from;
            }
            match self.changes.get(i + 1) {
                Some(&(until, _)) if wall >= until.saturating_add(offset) => {}
                _ => return wall.saturating_sub(offset),
            }
        }
        wall
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn us(text: &str) -> i64 {
        parse_r1(text).unwrap()
    }

    #[test]
    fn offsets_change_on_the_second_the_zone_says() {
        // New York in 2026: EDT from 07:00Z on Mar 8, EST from 06:00Z on Nov 1.
        let o = Offsets::over(
            us("2026-01-01T00:00:00Z"),
            us("2026-12-31T00:00:00Z"),
            "America/New_York",
        );
        let within = o.within(us("2026-01-01T00:00:00Z"), us("2026-12-31T00:00:00Z"));
        assert_eq!(
            within,
            [
                (us("2026-01-01T00:00:00Z"), -5 * HOUR_US),
                (us("2026-03-08T07:00:00Z"), -4 * HOUR_US),
                (us("2026-11-01T06:00:00Z"), -5 * HOUR_US),
            ]
        );
        assert_eq!(o.at(us("2026-03-08T06:59:59Z")), -5 * HOUR_US);
        assert_eq!(o.at(us("2026-03-08T07:00:00Z")), -4 * HOUR_US);
    }

    #[test]
    fn a_local_time_starts_at_its_first_instant() {
        let o = Offsets::over(
            us("2026-03-01T00:00:00Z"),
            us("2026-11-30T00:00:00Z"),
            "America/New_York",
        );
        let wall = |t: &str| us(t);
        // An ordinary time; the gap (02:30 never happened: the clock read
        // 03:00 at 07:00Z); the repeated hour (the first 01:30, in EDT).
        assert_eq!(
            o.start_of_local(wall("2026-03-07T12:00:00Z")),
            us("2026-03-07T17:00:00Z")
        );
        assert_eq!(
            o.start_of_local(wall("2026-03-08T02:30:00Z")),
            us("2026-03-08T07:00:00Z")
        );
        assert_eq!(
            o.start_of_local(wall("2026-11-01T01:30:00Z")),
            us("2026-11-01T05:30:00Z")
        );
        assert_eq!(
            o.start_of_local(wall("2026-11-01T02:00:00Z")),
            us("2026-11-01T07:00:00Z")
        );
    }

    #[test]
    fn half_hour_zones_are_not_epoch_aligned_to_the_hour() {
        let lo = us("2026-01-05T00:00:00Z");
        let o = Offsets::over(lo, lo, "Asia/Kolkata");
        assert!(!o.epoch_aligned(lo, lo, 3_600));
        assert!(o.epoch_aligned(lo, lo, 1_800));
        assert!(o.epoch_aligned(lo, lo, 60));
        let utc = Offsets::over(lo, lo, "UTC");
        assert!(BUCKET_LADDER
            .iter()
            .filter(|s| **s < DAY)
            .all(|s| utc.epoch_aligned(lo, lo, *s)));
    }

    #[test]
    fn monday_and_month_starts() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        assert_eq!(monday(d(2026, 1, 4)), d(2025, 12, 29));
        assert_eq!(monday(d(2026, 1, 5)), d(2026, 1, 5));
        let key = date_us(d(2026, 1, 4)).unwrap();
        assert_eq!(calendar_start(key, WEEK), date_us(d(2025, 12, 29)));
        assert_eq!(calendar_start(key, 2_592_000), date_us(d(2026, 1, 1)));
    }
}
