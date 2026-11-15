//! Account records: name, domain, SID, description, privilege and note,
//! with the columns computed from their linked entries.

use super::records::{
    agent_added, changeable_record, duplicate, next_id, non_empty, record_change, refuse_duplicate,
    row_json, seen_columns, RawSeen, RecordKind, RecordStatus, Seen,
};
use super::validate::{check_line, check_text, validate_sid, Privilege, MAX_RECORD_NAME};
use super::{has_records, require_records};
use crate::actor::Actor;
use crate::annot::record;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::query::check_offset;
use crate::time::now_iso;
use duckdb::types::Value as SqlValue;
use duckdb::{params, Connection};

/// An account record, as the Accounts sheet shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AccountRecord {
    pub id: i64,
    /// The account's name, without its domain.
    pub name: String,
    /// `None` for a local account or one whose domain is not known.
    pub domain: Option<String>,
    /// Canonical (`validate_sid`); `None` when not known.
    pub sid: Option<String>,
    /// Empty when not set.
    pub description: String,
    pub privilege: Privilege,
    /// Empty when there is none.
    pub note: String,
    pub status: RecordStatus,
    pub created_by: String,
    pub created_at: String,
    pub modified_by: String,
    pub modified_at: String,
    /// An agent created it; still true once an analyst has reviewed it.
    pub agent_added: bool,
    /// Deleted (a tombstone); only `account_record` returns one.
    pub deleted: bool,
    /// Live entries linked to it.
    pub entries: u64,
    /// The earliest time among them, undated entries left out.
    pub first_seen: Option<String>,
    /// The latest time among them.
    pub last_seen: Option<String>,
    /// The live host records those entries link, by name.
    pub hosts_seen: Vec<String>,
}

/// What `upsert_account` sets: a new record with `id: None`, else every
/// field of record `id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AccountSpec {
    pub id: Option<i64>,
    /// 1 to `MAX_RECORD_NAME` characters, not blank, no control character.
    /// With `domain`, unique among live accounts, ASCII case ignored.
    pub name: String,
    /// At most `MAX_RECORD_NAME` characters, no control character; empty is
    /// `None`.
    pub domain: Option<String>,
    /// `S-1-…` (`validate_sid`), stored canonical; empty is `None`.
    pub sid: Option<String>,
    /// At most `MAX_RECORD_NOTE` characters.
    pub description: String,
    pub privilege: Privilege,
    /// At most `MAX_RECORD_NOTE` characters.
    pub note: String,
}

/// The fields `spec` stores, checked: `(name, domain, sid)`.
fn checked(spec: &AccountSpec) -> Result<(String, Option<String>, Option<String>)> {
    check_line("account name", &spec.name, MAX_RECORD_NAME, true)?;
    let domain = non_empty(&spec.domain);
    if let Some(domain) = &domain {
        check_line("domain", domain, MAX_RECORD_NAME, false)?;
    }
    let sid = non_empty(&spec.sid).map(|s| validate_sid(&s)).transpose()?;
    check_text("description", &spec.description)?;
    check_text("note", &spec.note)?;
    Ok((spec.name.clone(), domain, sid))
}

/// The id of the live account named `name` in `domain`, ASCII case ignored
/// (no domain matches only no domain); `None` when there is none.
pub(crate) fn find_account(
    conn: &Connection,
    name: &str,
    domain: Option<&str>,
) -> Result<Option<i64>> {
    duplicate(
        conn,
        "SELECT id, name, domain FROM ox.account WHERE NOT deleted AND lower(name) = lower(?)",
        &[SqlValue::Text(name.to_string())],
        |r| {
            Ok((
                r.get(0)?,
                (r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?),
            ))
        },
        |(other_name, other_domain)| {
            other_name.eq_ignore_ascii_case(name)
                && match (other_domain.as_deref(), domain) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                    _ => false,
                }
        },
    )
}

/// Make or change an account record, as `spec` says: a new one
/// (`reviewed` for an analyst, `unreviewed` for an agent) when `spec.id`
/// is `None`, else every field of record `spec.id`. An edit that changes
/// nothing writes nothing. `Invalid` for a field that does not hold;
/// `Refused` for a name and domain another live account has (ASCII case
/// ignored), a deleted record, or one the actor may not change (S3);
/// `NotFound` for an unknown id.
pub fn upsert_account(case: &mut Case, spec: &AccountSpec, actor: &Actor) -> Result<AccountRecord> {
    require_records(case)?;
    let (name, domain, sid) = checked(spec)?;
    let kind = RecordKind::Account;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    if let Some(id) = spec.id {
        changeable_record(&tx, kind, id, actor)?;
    }
    if let Some(existing) =
        find_account(&tx, &name, domain.as_deref())?.filter(|&e| Some(e) != spec.id)
    {
        return Err(refuse_duplicate(
            kind,
            existing,
            "names that account in that domain",
        ));
    }
    let label = actor.label();
    let id = match spec.id {
        None => {
            let id = next_id(&tx, kind)?;
            tx.execute(
                "INSERT INTO ox.account \
                 (id, name, domain, sid, description, privilege, note, status, \
                  created_by, created_at, modified_by, modified_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    id,
                    name,
                    domain,
                    sid,
                    spec.description,
                    spec.privilege.as_str(),
                    spec.note,
                    RecordStatus::initial(actor).as_str(),
                    label,
                    at,
                    label,
                    at
                ],
            )?;
            let after = row_json(&tx, kind, id)?;
            let action = kind.action("create");
            record(
                &tx,
                actor,
                &at,
                record_change(kind, id, &action, None, Some(after)),
            )?;
            id
        }
        Some(id) => {
            let current = read_account(&tx, id)?;
            let unchanged = current.name == name
                && current.domain == domain
                && current.sid == sid
                && current.description == spec.description
                && current.privilege == spec.privilege
                && current.note == spec.note;
            if unchanged {
                return Ok(current);
            }
            let before = row_json(&tx, kind, id)?;
            tx.execute(
                "UPDATE ox.account SET name = ?, domain = ?, sid = ?, description = ?, \
                 privilege = ?, note = ?, modified_by = ?, modified_at = ? WHERE id = ?",
                params![
                    name,
                    domain,
                    sid,
                    spec.description,
                    spec.privilege.as_str(),
                    spec.note,
                    label,
                    at,
                    id
                ],
            )?;
            let after = row_json(&tx, kind, id)?;
            let action = kind.action("edit");
            record(
                &tx,
                actor,
                &at,
                record_change(kind, id, &action, Some(before), Some(after)),
            )?;
            id
        }
    };
    let out = read_account(&tx, id)?;
    tx.commit()?;
    Ok(out)
}

/// What every read of an account selects, in `raw_account`'s order.
fn account_select() -> String {
    format!(
        "SELECT r.id, r.name, r.domain, r.sid, r.description, r.privilege, r.note, r.status, \
         r.created_by, r.created_at, r.modified_by, r.modified_at, r.deleted, {} \
         FROM ox.account r",
        seen_columns(RecordKind::Account)
    )
}

/// An account as `account_select` reads it, before decoding.
type RawAccount = (
    (
        i64,
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        String,
        String,
    ),
    (String, String, String, String, bool),
    RawSeen,
);

fn raw_account(r: &duckdb::Row<'_>) -> duckdb::Result<RawAccount> {
    Ok((
        (
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
            r.get(7)?,
        ),
        (r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?, r.get(12)?),
        Seen::raw(r, 13)?,
    ))
}

fn decode_account(raw: RawAccount) -> Result<AccountRecord> {
    let ((id, name, domain, sid, description, privilege, note, status), stamps, seen) = raw;
    let (created_by, created_at, modified_by, modified_at, deleted) = stamps;
    let seen = Seen::decode(RecordKind::Account, id, seen)?;
    let privilege = Privilege::parse(&privilege)
        .map_err(|_| EngineError::Internal(format!("A-{id} holds the privilege {privilege:?}")))?;
    Ok(AccountRecord {
        id,
        name,
        domain,
        sid,
        description,
        privilege,
        note,
        status: RecordStatus::stored(&status)?,
        agent_added: agent_added(&created_by),
        created_by,
        created_at,
        modified_by,
        modified_at,
        deleted,
        entries: seen.entries,
        first_seen: seen.first_seen,
        last_seen: seen.last_seen,
        hosts_seen: seen.hosts_seen,
    })
}

fn read_account(conn: &Connection, id: i64) -> Result<AccountRecord> {
    let raw = conn
        .prepare(&format!("{} WHERE r.id = ?", account_select()))?
        .query_map(params![id], raw_account)?
        .next()
        .transpose()?
        .ok_or_else(|| EngineError::NotFound(format!("no {}", RecordKind::Account.shown(id))))?;
    decode_account(raw)
}

/// Account record `id`, deleted or not. `NotFound` for an unknown id, and
/// in a case without the record tables.
pub fn account_record(case: &Case, id: i64) -> Result<AccountRecord> {
    if !has_records(case) {
        return Err(EngineError::NotFound(format!(
            "no {}",
            RecordKind::Account.shown(id)
        )));
    }
    read_account(case.conn(), id)
}

/// Up to `limit` live account records from `offset`, by name, then domain
/// (case ignored, no domain first), then id. A case without the record
/// tables has none. `Invalid` for a `limit` of 0 or over `MAX_LIMIT`, or an
/// `offset` past `i64::MAX`.
pub fn accounts(case: &Case, limit: u32, offset: u64) -> Result<Listing<AccountRecord>> {
    accounts_matching(case, None, limit, offset)
}

/// An account as the sheets and the link picker show it, in SQL over `r`:
/// `DOMAIN\name`, or the name alone for an account without a domain.
const SHOWN_NAME: &str =
    "CASE WHEN r.domain IS NULL THEN r.name ELSE r.domain || chr(92) || r.name END";

/// [`accounts`] narrowed to the accounts whose `DOMAIN\name` holds `text`
/// (case ignored), so a name, a domain, or both typed as shown all find
/// one. The closest first: `match_rank` of the name or of `DOMAIN\name`,
/// whichever is closer, so `ACME\jdoe` typed in full is first however many
/// other domains have a `jdoe`. `None` lists every one. `Invalid` as
/// `accounts` is, and for a text over 1,000 characters.
pub fn accounts_matching(
    case: &Case,
    text: Option<&str>,
    limit: u32,
    offset: u64,
) -> Result<Listing<AccountRecord>> {
    check_limit(limit)?;
    check_offset(offset)?;
    if let Some(text) = text {
        super::check_name_filter(text)?;
    }
    if !has_records(case) {
        return Ok(Listing {
            items: Vec::new(),
            truncated: false,
        });
    }
    let raw: Vec<RawAccount> = case
        .conn()
        .prepare(&format!(
            "{} WHERE NOT r.deleted AND (CAST(? AS VARCHAR) IS NULL \
             OR contains(lower({SHOWN_NAME}), lower(?))) \
             ORDER BY least({}, {}), lower(r.name), lower(r.domain) NULLS FIRST, r.id \
             LIMIT ? OFFSET ?",
            account_select(),
            super::match_rank("r.name"),
            super::match_rank(SHOWN_NAME)
        ))?
        .query_map(
            // The text: twice for the filter, then for each `match_rank`.
            params![
                text,
                text,
                text,
                text,
                text,
                text,
                text,
                text,
                fetch_limit(limit),
                offset
            ],
            raw_account,
        )?
        .collect::<std::result::Result<_, _>>()?;
    let items = raw
        .into_iter()
        .map(decode_account)
        .collect::<Result<Vec<_>>>()?;
    Ok(Listing::from_fetched(items, limit))
}
