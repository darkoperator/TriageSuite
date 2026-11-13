//! Time in the display zone, for the grid: the histogram and the group
//! spans it shows, and the date-range chip an analyst types in local time.
//!
//! Storage stays UTC. Times go out rendered in the zone and come in typed
//! in it, and every conversion goes through `triage_core::timestamp::zone`.
//! Only a `utc` column is converted; an `unknown` or `local` one is shown
//! and compared as written, never guessed into a zone.

use super::{Chip, ChipOrigin, GridState, MAX_CHIP_LABEL};
use crate::case::Case;
use crate::catalog::{self, SqlType, TimeSemantics};
use crate::error::{EngineError, Result};
use crate::filter::{FilterNode, Leaf, Op};
use crate::listing::Listing;
use crate::query::{
    check_zone, find_column, group_spans, histogram, primary_time_semantics, Histogram,
};
use chrono::{NaiveDateTime, TimeDelta};
use serde_json::Value;
use triage_core::timestamp::zone::{self, LocalError};

/// One group of the grid's group bar.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GroupView {
    /// The column's text, cut to `CELL_CAP` characters; `None` for an empty
    /// cell.
    pub key: Option<String>,
    /// `key` was cut. A cut key selects nothing; drill with `full_key`.
    pub key_clipped: bool,
    /// The whole key, only when `key_clipped`: what the app passes back as
    /// a parent key or to `group_filter` to open a group whose key was cut.
    pub full_key: Option<String>,
    pub count: u64,
    /// The earliest and latest primary time in the group, as display text:
    /// in the zone with the fraction as written for a `utc` time, as written
    /// for any other; `None` without a typed primary time or a timed row.
    pub first: Option<String>,
    pub last: Option<String>,
    pub suspicious: u64,
}

/// `histogram` of what `state` shows (in find mode the search does not
/// narrow it), in `zone`.
pub fn histogram_view(
    case: &Case,
    state: &GridState,
    column: Option<&str>,
    max_buckets: u32,
    zone: &str,
) -> Result<Histogram> {
    histogram(case, &state.query()?, column, max_buckets, zone)
}

/// `groups` of what `state` shows, with each group's span as display text
/// in `zone`. `Invalid` as `groups` is, and for an unknown zone.
// `groups`'s six inputs plus the zone, each one independent part of the
// group bar's request; this signature is the interface the app's command is
// built on, and a struct around them would only move the same names.
#[allow(clippy::too_many_arguments)]
pub fn groups_view(
    case: &Case,
    state: &GridState,
    by: &[String],
    parent: &[Option<String>],
    limit: u32,
    offset: u64,
    zone: &str,
) -> Result<Listing<GroupView>> {
    check_zone(zone)?;
    let q = state.query()?;
    let spans = group_spans(case, &q, by, parent, limit, offset)?;
    let relation = catalog::relation(case, &q.relation)?;
    let all = catalog::columns(case, &q.relation)?;
    let converts = primary_time_semantics(&relation.primary_time, &all) == Some(TimeSemantics::Utc);
    let shown = |text: Option<String>, utc: Option<String>| -> Option<String> {
        if !converts {
            return text.or(utc);
        }
        text.as_deref()
            .and_then(|t| zone::render_utc_text(t, zone))
            .or_else(|| utc.as_deref().and_then(|u| zone::render_utc_text(u, zone)))
            .map(|r| r.local)
    };
    Ok(Listing {
        items: spans
            .items
            .into_iter()
            .map(|s| GroupView {
                full_key: if s.row.key_clipped {
                    s.row.full_key
                } else {
                    None
                },
                key: s.row.key,
                key_clipped: s.row.key_clipped,
                count: s.row.count,
                first: shown(s.first_text, s.row.first),
                last: shown(s.last_text, s.row.last),
                suspicious: s.row.suspicious,
            })
            .collect(),
        truncated: spans.truncated,
    })
}

/// A date-range chip on the timestamp `column` of `relation`, from `from`
/// to `to` inclusive of its whole last second, both `YYYY-MM-DD HH:MM:SS`
/// on the display zone's clock; at least one is needed. A `utc` column's
/// bounds are converted to UTC through `zone::local_to_utc`; a time the
/// clock showed twice, or skipped, is refused, never resolved by guessing.
/// An `unknown` or `local` column is compared as written and the label says
/// so. The filter is `gte from` and `lt to + 1 s`; the id is `dr-` and 16
/// random hex digits, so two chips never share one.
///
/// `Invalid` for neither bound, a bound not in that form or not a real
/// time, a range that ends before it starts, an unknown zone or a column
/// that is not a timestamp; `NotFound` for an unknown relation or column.
pub fn date_range_chip(
    case: &Case,
    relation: &str,
    column: &str,
    from: Option<&str>,
    to: Option<&str>,
    zone: &str,
) -> Result<Chip> {
    check_zone(zone)?;
    let all = catalog::columns(case, relation)?;
    let c = find_column(&all, relation, column)?;
    if c.typed_column.is_none() || c.sql_type != Some(SqlType::Timestamp) {
        return Err(EngineError::Invalid(format!(
            "{relation}.{column} is not a timestamp column, so it takes no date range"
        )));
    }
    if from.is_none() && to.is_none() {
        return Err(EngineError::Invalid(
            "a date range needs a from time, a to time or both".to_string(),
        ));
    }
    let converts = c.time_semantics == Some(TimeSemantics::Utc);
    let lower = from.map(|t| instant(t, converts, zone)).transpose()?;
    let upper = to.map(|t| instant(t, converts, zone)).transpose()?;
    // R1 text is fixed-width, so it compares in time order.
    if let (Some(lower), Some(upper)) = (&lower, &upper) {
        if lower > upper {
            return Err(EngineError::Invalid(format!(
                "the date range ends ({}) before it starts ({})",
                to.unwrap_or_default(),
                from.unwrap_or_default()
            )));
        }
    }
    let leaf = |op: Op, value: String| {
        FilterNode::Leaf(Leaf {
            column: c.name.clone(),
            op,
            value: Value::String(value),
            case_sensitive: false,
        })
    };
    let mut bounds = Vec::new();
    if let Some(lower) = lower {
        bounds.push(leaf(Op::Gte, lower));
    }
    if let Some(upper) = upper {
        // Up to the end of the last second; at 9999-12-31 23:59:59 there is
        // no next second to write, so the second itself is the bound.
        bounds.push(match next_second(&upper) {
            Some(next) => leaf(Op::Lt, next),
            None => leaf(Op::Lte, upper),
        });
    }
    let filter = if bounds.len() == 1 {
        bounds.remove(0)
    } else {
        FilterNode::And(bounds)
    };
    let clock = if converts { zone } else { "(as written)" };
    Ok(Chip {
        id: format!("dr-{}", random_hex()?),
        label: label(&c.name, from, to, clock),
        enabled: true,
        origin: ChipOrigin::DateRange,
        filter,
    })
}

/// The UTC instant (R1) a typed bound stands for: on `zone`'s clock for a
/// column that converts, else as written (as if UTC, which has no gap and
/// no repeated hour).
fn instant(text: &str, converts: bool, zone: &str) -> Result<String> {
    let clock = if converts { zone } else { "UTC" };
    zone::local_to_utc(text, clock).map_err(|e| match e {
        LocalError::Ambiguous { earlier, later } => EngineError::Invalid(format!(
            "{text} happened twice in {zone}, at {earlier} and at {later} UTC, as the clocks \
             went back; choose a time outside the repeated hour"
        )),
        LocalError::Nonexistent => EngineError::Invalid(format!(
            "{text} does not exist in {zone}: the clocks went forward over it"
        )),
        LocalError::BadText => {
            let cut: String = text.chars().take(40).collect();
            EngineError::Invalid(format!(
                "{cut:?} is not a time written YYYY-MM-DD HH:MM:SS between years 0000 and 9999"
            ))
        }
        LocalError::UnknownZone => {
            EngineError::Invalid(format!("{zone} is not a time zone this build knows"))
        }
    })
}

/// R1 text one second after `r1`; `None` past year 9999.
fn next_second(r1: &str) -> Option<String> {
    let at = NaiveDateTime::parse_from_str(r1, "%Y-%m-%dT%H:%M:%S%.fZ").ok()?;
    let next = at.checked_add_signed(TimeDelta::seconds(1))?;
    let text = next.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    crate::time::parse_utc_text(&text)
}

/// `<column> <from> → <to> <clock>`, an open end written `…`, the column
/// name cut so the whole fits `MAX_CHIP_LABEL` characters.
fn label(column: &str, from: Option<&str>, to: Option<&str>, clock: &str) -> String {
    let tail = format!(" {} → {} {clock}", from.unwrap_or("…"), to.unwrap_or("…"));
    let room = MAX_CHIP_LABEL.saturating_sub(tail.chars().count());
    let name: String = if column.chars().count() > room {
        let mut cut: String = column.chars().take(room.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        column.to_string()
    };
    format!("{name}{tail}")
}

/// 16 lowercase hex digits from the operating system's random source.
fn random_hex() -> Result<String> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|e| {
        EngineError::Io(std::io::Error::other(format!(
            "the operating system's random source failed: {e}"
        )))
    })?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
