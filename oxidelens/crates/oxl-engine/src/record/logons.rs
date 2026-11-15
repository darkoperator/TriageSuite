//! An account's logon summary (ruling S6): its successful and failed
//! logons by logon type and host, counted from the event-log dataset's
//! Security 4624 and 4625 events. It is computed each time it is asked for,
//! is never stored, and makes no judgement; each count carries the filter
//! document that opens the rows behind it.
//!
//! A row is read as S6 says and only so: the account is `PayloadData1`
//! after `Target: `, split at the last `\` (`candidates::logon_target`,
//! the reading a promoted row's candidates use); the logon type is the
//! integer after `LogonType ` in `PayloadData2`; the host is the relation's
//! host column. A row that does not read this way is counted as unparsed,
//! never guessed at.

use super::accounts::{account_record, AccountRecord};
use super::candidates::logon_target;
use super::RecordKind;
use crate::case::Case;
use crate::catalog::{self, ColumnInfo};
use crate::error::{EngineError, Result};
use crate::filter::{FilterNode, Leaf, Op, MAX_IN};
use crate::query::{scope, GridQuery, ROW_ALIAS};
use duckdb::params_from_iter;
use duckdb::types::Value as SqlValue;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use triage_core::output::duckdb::sql::quote_ident;

/// The event-log dataset the summary reads.
const EVENTS: &str = "evtxtriage_events";
/// The channel whose logons are counted, as EvtxTriage spells it.
const CHANNEL: &str = "Security";
/// The event id of a successful logon, as the cell's text writes it.
const SUCCESS: &str = "4624";
/// The event id of a failed logon.
const FAILURE: &str = "4625";
/// What the logon type in `PayloadData2` follows.
const LOGON_TYPE: &str = "LogonType ";

/// Whether a logon succeeded (4624) or failed (4625).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogonOutcome {
    Success,
    Failure,
}

/// The logons of one outcome and logon type on one host.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LogonCount {
    pub outcome: LogonOutcome,
    pub logon_type: u32,
    /// The host the events are from; `None` for rows without one, which a
    /// relation with no host column only has.
    pub host: Option<String>,
    pub count: u64,
    /// A filter document over the event-log dataset selecting exactly these
    /// rows: each leaf is case-sensitive, and the account and logon type are
    /// the cell texts that were counted, so the grid it opens shows `count`
    /// rows for as long as the case does not change.
    pub filter: FilterNode,
}

/// An account's logon summary.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LogonSummary {
    /// By host (rows without one last), logon type, then success before
    /// failure. Only the account's logons; empty when it has none.
    pub rows: Vec<LogonCount>,
    /// Security 4624 and 4625 rows that do not read as S6 says, whoever's
    /// they are: no `Target: ` account, or no `LogonType ` integer.
    pub unparsed: u64,
    /// The dataset read, `evtxtriage_events`; `None` when the case has no
    /// such dataset, and then there are no rows and nothing unparsed.
    pub source: Option<String>,
}

/// The logon summary of live account `account_id`, as S6 reads the
/// event-log dataset: its name matches with ASCII case ignored, and its
/// domain too when it has one (an account without a domain matches the
/// name in any domain; one with a domain does not match a row without
/// one). `NotFound` for an account the case does not have or has deleted,
/// and in a case without the record tables.
pub fn logon_summary(case: &Case, account_id: i64) -> Result<LogonSummary> {
    let account = account_record(case, account_id)?;
    if account.deleted {
        return Err(EngineError::NotFound(format!(
            "no {}: it was deleted",
            RecordKind::Account.shown(account_id)
        )));
    }
    let mut out = LogonSummary {
        rows: Vec::new(),
        unparsed: 0,
        source: None,
    };
    let relation = match catalog::relation(case, EVENTS) {
        Ok(relation) => relation,
        Err(EngineError::NotFound(_)) => return Ok(out),
        Err(e) => return Err(e),
    };
    out.source = Some(relation.name.clone());
    let columns = catalog::columns(case, EVENTS)?;
    let column = |name: &str| columns.iter().find(|c| c.name == name);
    // Without a channel or an event id no row is a Security logon.
    let (Some(channel), Some(event_id)) = (column("Channel"), column("EventId")) else {
        return Ok(out);
    };
    let host = match &relation.host_column {
        Some(name) => Some(column(name).ok_or_else(|| {
            EngineError::Internal(format!("{EVENTS}'s host column {name} is not catalogued"))
        })?),
        None => None,
    };
    let read = Read {
        channel,
        event_id,
        host,
        account: column("PayloadData1"),
        logon_type: column("PayloadData2"),
    };

    let groups = read_groups(case, &read)?;
    let mut counted: BTreeMap<Key, Opened> = BTreeMap::new();
    for g in groups {
        let parsed = g
            .account
            .as_deref()
            .and_then(logon_target)
            .zip(g.logon_type.as_deref().and_then(logon_type));
        let Some(((name, domain), logon_type)) = parsed else {
            out.unparsed += g.count;
            continue;
        };
        if !matches(&account, &name, domain.as_deref()) {
            continue;
        }
        let outcome = if g.event_id == SUCCESS {
            LogonOutcome::Success
        } else {
            LogonOutcome::Failure
        };
        let host = g.host.filter(|h| !h.is_empty());
        let opened = counted
            .entry(Key {
                host_missing: host.is_none(),
                host,
                logon_type,
                outcome,
            })
            .or_default();
        opened.count += g.count;
        // Both are `Some`: they parsed.
        opened.accounts.extend(g.account);
        opened.types.extend(g.logon_type);
    }
    for (key, opened) in counted {
        let filter = count_filter(&key, &opened, &read)?;
        out.rows.push(LogonCount {
            outcome: key.outcome,
            logon_type: key.logon_type,
            host: key.host,
            count: opened.count,
            filter,
        });
    }
    Ok(out)
}

/// Whether `account` is the one a row names as `name` in `domain` (S6).
fn matches(account: &AccountRecord, name: &str, domain: Option<&str>) -> bool {
    account.name.eq_ignore_ascii_case(name)
        && match &account.domain {
            None => true,
            Some(own) => domain.is_some_and(|d| d.eq_ignore_ascii_case(own)),
        }
}

/// The logon type `text` gives: the ASCII digits after `LogonType `, all
/// the rest of the text, as a `u32`. `None` for anything else, a value
/// past `u32::MAX` included. Total: any text gives an answer.
fn logon_type(text: &str) -> Option<u32> {
    let digits = text.strip_prefix(LOGON_TYPE)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The columns of the event-log dataset the summary reads. A column the
/// dataset lacks is `None` and reads as empty.
struct Read<'a> {
    channel: &'a ColumnInfo,
    event_id: &'a ColumnInfo,
    /// `None` for a relation with no host column.
    host: Option<&'a ColumnInfo>,
    /// `PayloadData1`.
    account: Option<&'a ColumnInfo>,
    /// `PayloadData2`.
    logon_type: Option<&'a ColumnInfo>,
}

/// One distinct `(host, event id, account text, logon type text)` of the
/// Security logons, and how many rows have it.
struct Group {
    host: Option<String>,
    event_id: String,
    account: Option<String>,
    logon_type: Option<String>,
    count: u64,
}

/// The Security 4624 and 4625 rows of the dataset, as the grid sees it,
/// grouped by the cell texts the summary reads, so that a dataset of many
/// logons is read as its few distinct accounts.
fn read_groups(case: &Case, read: &Read<'_>) -> Result<Vec<Group>> {
    let scope = scope(
        case,
        &GridQuery {
            relation: EVENTS.into(),
            ..Default::default()
        },
    )?;
    let text = |c: Option<&ColumnInfo>| match c {
        Some(c) => format!(
            "CAST({ROW_ALIAS}.{} AS VARCHAR)",
            quote_ident(&c.text_column)
        ),
        None => "CAST(NULL AS VARCHAR)".to_string(),
    };
    let sql = format!(
        "SELECT {host}, {event_id}, {account}, {logon_type}, count(*) \
         FROM {from} WHERE ({where_sql}) AND {channel} = ? AND {event_id} IN (?, ?) \
         GROUP BY ALL",
        host = text(read.host),
        event_id = text(Some(read.event_id)),
        account = text(read.account),
        logon_type = text(read.logon_type),
        channel = text(Some(read.channel)),
        from = scope.from_sql,
        where_sql = scope.where_sql,
    );
    let mut params = scope.params;
    params.extend([CHANNEL, SUCCESS, FAILURE].map(|v| SqlValue::Text(v.to_string())));
    let groups = case
        .conn()
        .prepare(&sql)?
        .query_map(params_from_iter(params), |r| {
            Ok(Group {
                host: r.get(0)?,
                event_id: r.get(1)?,
                account: r.get(2)?,
                logon_type: r.get(3)?,
                count: crate::query::as_count(r.get(4)?),
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(groups)
}

/// What a count is keyed by, in the order the summary lists them.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    /// Rows without a host sort last.
    host_missing: bool,
    host: Option<String>,
    logon_type: u32,
    outcome: LogonOutcome,
}

/// A count and the cell texts behind it.
#[derive(Default)]
struct Opened {
    count: u64,
    accounts: BTreeSet<String>,
    types: BTreeSet<String>,
}

fn leaf(column: &ColumnInfo, op: Op, value: Value) -> FilterNode {
    FilterNode::Leaf(Leaf {
        column: column.name.clone(),
        op,
        value,
        case_sensitive: true,
    })
}

/// The filter that selects exactly the rows of one count: the Security
/// channel, its outcome's event id, its host (empty when it has none),
/// and the account and logon type texts that were counted for it, each
/// compared case-sensitively as the summary read them. A row it selects
/// has texts that read as this count's, so it was counted here.
fn count_filter(key: &Key, opened: &Opened, read: &Read<'_>) -> Result<FilterNode> {
    // A count exists only for rows whose account and type parsed, which
    // needs both columns.
    let (Some(account), Some(logon_type)) = (read.account, read.logon_type) else {
        return Err(EngineError::Internal(
            "a logon was counted without its account and type columns".into(),
        ));
    };
    let texts = |set: &BTreeSet<String>| -> Result<Value> {
        if set.len() > MAX_IN {
            return Err(EngineError::Invalid(format!(
                "a logon count is written {} ways, more than a filter lists ({MAX_IN})",
                set.len()
            )));
        }
        Ok(Value::from(set.iter().cloned().collect::<Vec<_>>()))
    };
    let id = match key.outcome {
        LogonOutcome::Success => SUCCESS,
        LogonOutcome::Failure => FAILURE,
    };
    let mut all = vec![
        leaf(read.channel, Op::Eq, Value::from(CHANNEL)),
        leaf(read.event_id, Op::Eq, Value::from(id)),
    ];
    if let Some(host) = read.host {
        all.push(match &key.host {
            Some(h) => leaf(host, Op::Eq, Value::from(h.as_str())),
            None => leaf(host, Op::IsEmpty, Value::Null),
        });
    }
    all.push(leaf(account, Op::In, texts(&opened.accounts)?));
    all.push(leaf(logon_type, Op::In, texts(&opened.types)?));
    Ok(FilterNode::And(all))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logon_types_are_the_integer_after_the_prefix() {
        assert_eq!(logon_type("LogonType 10"), Some(10));
        assert_eq!(logon_type("LogonType 3"), Some(3));
        assert_eq!(logon_type("LogonType 03"), Some(3));
        assert_eq!(logon_type("LogonType 4294967295"), Some(u32::MAX));
        for none in [
            "",
            "LogonType ",
            "LogonType 4294967296",
            "LogonType -3",
            "LogonType +3",
            "LogonType 3 ",
            "LogonType 3x",
            "logontype 3",
            "LogonType  3",
            "3",
            "LogonType ٣",
        ] {
            assert_eq!(logon_type(none), None, "{none:?}");
        }
    }
}
