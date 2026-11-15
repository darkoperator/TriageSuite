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
use super::candidates::{logon_target, TARGET};
use super::validate::MAX_RECORD_NAME;
use super::RecordKind;
use crate::case::Case;
use crate::catalog::{self, ColumnInfo};
use crate::error::{EngineError, Result};
use crate::filter::{FilterNode, Leaf, Op, MAX_REGEX};
use crate::query::{scope, GridQuery, ROW_ALIAS};
use duckdb::params_from_iter;
use duckdb::types::Value as SqlValue;
use serde_json::Value;
use std::collections::BTreeMap;
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
    /// regexes that take exactly what S6 reads as this account and type
    /// (`count_filter`), so the grid it opens shows `count` rows for as long
    /// as the case does not change. Its size does not grow with the ways
    /// the rows spell the account.
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

    let mut counted: BTreeMap<Key, u64> = BTreeMap::new();
    for_each_group(case, &read, |g| {
        let parsed = g
            .account
            .as_deref()
            .and_then(logon_target)
            .zip(g.logon_type.as_deref().and_then(logon_type));
        let Some(((name, domain), logon_type)) = parsed else {
            out.unparsed += g.count;
            return;
        };
        if !matches(&account, &name, domain.as_deref()) {
            return;
        }
        let outcome = if g.event_id == SUCCESS {
            LogonOutcome::Success
        } else {
            LogonOutcome::Failure
        };
        let host = g.host.filter(|h| !h.is_empty());
        *counted
            .entry(Key {
                host_missing: host.is_none(),
                host,
                logon_type,
                outcome,
            })
            .or_default() += g.count;
    })?;
    for (key, count) in counted {
        let filter = count_filter(&key, &account, &read)?;
        out.rows.push(LogonCount {
            outcome: key.outcome,
            logon_type: key.logon_type,
            host: key.host,
            count,
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

/// Call `each` with every distinct `Group` of the dataset's Security 4624
/// and 4625 rows, as the grid sees them, one at a time as DuckDB yields
/// them: none is kept here, so what the summary holds is its counters,
/// whatever the evidence holds. The grouping is DuckDB's, which spills to
/// disk rather than outgrow its memory limit.
fn for_each_group(case: &Case, read: &Read<'_>, mut each: impl FnMut(Group)) -> Result<()> {
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
    let mut stmt = case.conn().prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;
    while let Some(r) = rows.next()? {
        each(Group {
            host: r.get(0)?,
            event_id: r.get(1)?,
            account: r.get(2)?,
            logon_type: r.get(3)?,
            count: crate::query::as_count(r.get(4)?),
        });
    }
    Ok(())
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

fn leaf(column: &ColumnInfo, op: Op, value: Value) -> FilterNode {
    FilterNode::Leaf(Leaf {
        column: column.name.clone(),
        op,
        value,
        case_sensitive: true,
    })
}

fn regex(column: &ColumnInfo, pattern: String) -> FilterNode {
    leaf(column, Op::Regex, Value::from(pattern))
}

/// The characters `str::trim` takes off, Unicode's White_Space, as a regex
/// class: a domain that trims to nothing or `-` is no domain (S6).
const WHITE_SPACE: &str =
    r"[\t-\r \x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}]";

/// A regex matching exactly `c`, or `c` in the other ASCII case for an
/// ASCII letter: `[jJ]`. A class, not `(?i)`, which folds by Unicode and
/// would take the Kelvin sign for `k`. ASCII punctuation is escaped; any
/// other character stands for itself.
fn atom(c: char) -> String {
    if c.is_ascii_alphabetic() {
        format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
    } else {
        exact(c)
    }
}

/// A regex matching exactly `c`, case and all.
fn exact(c: char) -> String {
    if c.is_ascii_punctuation() {
        format!("\\{c}")
    } else {
        c.to_string()
    }
}

/// `text` as a regex matching exactly it, case and all: the fixed parts of
/// S6's reading (`Target: `, `LogonType `), which the parse takes as
/// written.
fn exact_text(text: &str) -> String {
    text.chars().map(exact).collect()
}

/// Leaves that together match exactly the texts `atoms` match in ASCII
/// case (one atom per character), at a fixed place: from the start, or,
/// with `from_end`, ending at the end. Each leaf holds as many atoms as fit
/// in `MAX_REGEX` characters and reaches its place past the others with
/// `.{n}` (`(?s)`, so any character), so a long name never makes a pattern
/// the filter refuses. A text matching every leaf has, in each place, a
/// character its atom matches.
fn placed(column: &ColumnInfo, atoms: &[String], from_end: bool) -> Vec<FilterNode> {
    // Room for `(?s)^`, `.{1000}` and `$` around the atoms.
    const ROOM: usize = MAX_REGEX - 16;
    let mut out = Vec::new();
    let mut start = 0;
    while start < atoms.len() {
        let mut end = start;
        let mut len = 0;
        while end < atoms.len() && len + atoms[end].chars().count() <= ROOM {
            len += atoms[end].chars().count();
            end += 1;
        }
        let body = atoms[start..end].concat();
        let pattern = if from_end {
            match atoms.len() - end {
                0 => format!("(?s){body}$"),
                after => format!("(?s){body}.{{{after}}}$"),
            }
        } else {
            match start {
                0 => format!("(?s)^{body}{}", if end == atoms.len() { "$" } else { "" }),
                before => format!(
                    "(?s)^.{{{before}}}{body}{}",
                    if end == atoms.len() { "$" } else { "" }
                ),
            }
        };
        out.push(regex(column, pattern));
        start = end;
    }
    out
}

/// The leaves that select exactly the `PayloadData1` texts S6 reads as
/// `account`. With a domain: `Target: D\N`, both parts in ASCII case only.
/// Without one: the name `N` (ASCII case), at the end, after `Target: `
/// alone or after a domain and `\`, where the domain is either one that
/// trims to nothing or `-` or one `upsert_account` would take (at most
/// `MAX_RECORD_NAME` characters, no control character), as
/// `candidates::logon_target` reads it. A count exists only for an account
/// some row parsed into, so its name holds no `\` and is not `-`: the last
/// `\` of a matching text is the one before the name.
fn account_leaves(column: &ColumnInfo, account: &AccountRecord) -> Vec<FilterNode> {
    let name: Vec<String> = account.name.chars().map(atom).collect();
    match &account.domain {
        Some(domain) => {
            let atoms: Vec<String> = TARGET
                .chars()
                .map(exact)
                .chain(domain.chars().map(atom))
                .chain(std::iter::once(exact('\\')))
                .chain(name)
                .collect();
            placed(column, &atoms, false)
        }
        None => {
            let ws = WHITE_SPACE;
            let mut out = vec![regex(
                column,
                format!(
                    r"(?s)^{}(?:(?:{ws}*-?{ws}*|\P{{Cc}}{{0,{MAX_RECORD_NAME}}})\\)?.{{{}}}$",
                    exact_text(TARGET),
                    name.len()
                ),
            )];
            out.extend(placed(column, &name, true));
            out
        }
    }
}

/// The filter that selects exactly the rows of one count: the Security
/// channel, its outcome's event id, its host (empty when it has none), the
/// account as S6 matches it (`account_leaves`) and the logon type with any
/// leading zeros. Its size depends on the account and the count, never on
/// how many ways the rows spell them.
fn count_filter(key: &Key, account: &AccountRecord, read: &Read<'_>) -> Result<FilterNode> {
    // A count exists only for rows whose account and type parsed, which
    // needs both columns.
    let (Some(account_text), Some(type_text)) = (read.account, read.logon_type) else {
        return Err(EngineError::Internal(
            "a logon was counted without its account and type columns".into(),
        ));
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
    all.extend(account_leaves(account_text, account));
    all.push(regex(
        type_text,
        format!("^{}0*{}$", exact_text(LOGON_TYPE), key.logon_type),
    ));
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
