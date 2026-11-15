//! The account and IOC candidates a row offers when it is promoted: the
//! values of the columns `MAP` names for the row's relation, and its
//! evidence path, each validated as its record would be. The Timeline
//! sheet shows them for one-click linking; this module only reads. Nothing
//! is added to the case here.
//!
//! The row is read through `row_detail`, from its own source table, so a
//! superseded row offers what it holds and a removed source's row offers
//! nothing.

use super::accounts::find_account;
use super::iocs::find_ioc;
use super::validate::{check_line, validate_ioc, validate_sid, MAX_RECORD_NAME};
use super::{has_records, IocType, RecordKind};
use crate::annot::row_detail;
use crate::case::Case;
use crate::catalog;
use crate::error::Result;
use duckdb::Connection;

/// A record the row names, for the analyst to link: an account or an IOC.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Candidate {
    /// `Account` or `Ioc`.
    pub kind: RecordKind,
    /// The IOC's type; `None` for an account.
    pub ioc_type: Option<IocType>,
    /// The IOC's value in canonical form (`validate_ioc`), or the account's
    /// name without its domain. Text from evidence.
    pub value: String,
    /// The account's domain, the part before the last `\`; `None` for an
    /// IOC and for an account written without one.
    pub domain: Option<String>,
    /// The account's SID, canonical (`validate_sid`), when the row gives
    /// one that validates; `None` otherwise and for an IOC.
    pub sid: Option<String>,
    /// The column the value was read from.
    pub column: String,
    /// The live record of this kind the case already has for the value (an
    /// account by name and domain, an IOC by type and value, ASCII case
    /// ignored); `None` when it has none, and always below case format 3,
    /// which has no records.
    pub existing_id: Option<i64>,
}

/// Which relations a `MAP` entry reads.
#[derive(Debug, Clone, Copy)]
enum Relations {
    /// The relation of this name.
    Named(&'static str),
    /// Every relation whose name starts with this.
    Prefix(&'static str),
    /// Every relation whose name is this prefix, at least one character,
    /// and this suffix.
    Around(&'static str, &'static str),
}

impl Relations {
    fn hold(&self, relation: &str) -> bool {
        match *self {
            Relations::Named(name) => relation == name,
            Relations::Prefix(prefix) => relation.starts_with(prefix),
            Relations::Around(prefix, suffix) => {
                relation.len() > prefix.len() + suffix.len()
                    && relation.starts_with(prefix)
                    && relation.ends_with(suffix)
            }
        }
    }
}

/// What a `MAP` entry reads its column as.
#[derive(Debug, Clone, Copy)]
enum Take {
    /// An account written `D\N` or `N`; its SID from the column named, when
    /// there is one.
    Account { sid: Option<&'static str> },
    /// An account written `Target: D\N`, as EvtxTriage writes a logon's
    /// target account (ruling S6).
    LogonTarget,
    /// An ip written `NAME (IP)`, as EvtxECmd's logon maps write a remote
    /// host, or a bare address.
    RemoteAddress,
    /// An IOC of this type, when the value validates as one.
    Ioc(IocType),
}

/// The columns each relation's candidates come from, in the order they are
/// offered, by the names a real run's datasets carry. Beside these, every
/// relation's evidence path column offers a `file_path`.
const MAP: &[(Relations, &str, Take)] = &[
    (
        Relations::Named("evtxtriage_events"),
        "UserName",
        Take::Account { sid: None },
    ),
    (
        Relations::Named("evtxtriage_events"),
        "PayloadData1",
        Take::LogonTarget,
    ),
    (
        Relations::Named("evtxtriage_events"),
        "RemoteHost",
        Take::RemoteAddress,
    ),
    (
        Relations::Around("amcachetriage_", "_file_entries"),
        "SHA1",
        Take::Ioc(IocType::Sha1),
    ),
    // PETriage's `Hash` is the prefetch header hash, eight digits as the
    // tool writes it; only a 40-digit value is a SHA1.
    (
        Relations::Named("petriage_main"),
        "Hash",
        Take::Ioc(IocType::Sha1),
    ),
    (
        Relations::Named("browsertriage_downloads"),
        "Hash (SHA-256)",
        Take::Ioc(IocType::Sha256),
    ),
    (
        Relations::Named("browsertriage_downloads"),
        "Download URL",
        Take::Ioc(IocType::Url),
    ),
    (
        Relations::Named("browsertriage_history"),
        "URL",
        Take::Ioc(IocType::Url),
    ),
    (
        Relations::Prefix("retriage_typedurls_"),
        "Url",
        Take::Ioc(IocType::Url),
    ),
    (
        Relations::Prefix("srumetriage_"),
        "UserName",
        Take::Account { sid: Some("Sid") },
    ),
];

/// What logon target text starts with (ruling S6).
pub(super) const TARGET: &str = "Target: ";

/// The account and IOC candidates row `row_id` offers, in `MAP`'s order
/// and its evidence path last, each once (the first column to offer it
/// names it). A value that does not validate as its kind is left out.
/// Read only: nothing is added, linked or changed. `NotFound` for a row
/// the case does not know.
pub fn candidates(case: &Case, row_id: i64) -> Result<Vec<Candidate>> {
    let row = row_detail(case, row_id)?;
    let text = |column: &str| -> Option<&str> {
        row.fields
            .iter()
            .find(|(name, _)| name == column)
            .and_then(|(_, cell)| cell.text.as_deref())
    };
    let mut out: Vec<Candidate> = Vec::new();
    for &(relations, column, take) in MAP {
        if !relations.hold(&row.relation) {
            continue;
        }
        let Some(value) = text(column) else {
            continue;
        };
        let found = match take {
            Take::Account { sid } => account(value).map(|(name, domain)| {
                let sid = sid.and_then(text).and_then(|s| validate_sid(s).ok());
                account_candidate(name, domain, sid, column)
            }),
            Take::LogonTarget => logon_target(value)
                .map(|(name, domain)| account_candidate(name, domain, None, column)),
            Take::RemoteAddress => ioc_candidate(IocType::Ip, value, column).or_else(|| {
                parenthesised(value).and_then(|ip| ioc_candidate(IocType::Ip, ip, column))
            }),
            Take::Ioc(t) => ioc_candidate(t, value, column),
        };
        offer(&mut out, found);
    }
    if let Some(path) = &row.evidence_path {
        if let Some(column) = catalog::relation(case, &row.relation)?.evidence_path_column {
            offer(&mut out, ioc_candidate(IocType::FilePath, path, &column));
        }
    }
    if has_records(case) {
        let conn = case.conn();
        for c in &mut out {
            c.existing_id = match c.ioc_type {
                None => existing_account(conn, &c.value, c.domain.as_deref())?,
                Some(t) => find_ioc(conn, t, &c.value)?,
            };
        }
    }
    Ok(out)
}

/// The live account an account candidate names, as S6 matches accounts:
/// the one with its name and domain, else one with its name and no
/// domain, ASCII case ignored. `find_account` alone, the uniqueness rule,
/// would not let a domainless record stand for `D\N`.
fn existing_account(conn: &Connection, name: &str, domain: Option<&str>) -> Result<Option<i64>> {
    match find_account(conn, name, domain)? {
        Some(id) => Ok(Some(id)),
        None if domain.is_some() => find_account(conn, name, None),
        None => Ok(None),
    }
}

/// What the last ` (` … `)` of `text` holds, when `text` ends in one:
/// `203.0.113.7` of `WS01 (203.0.113.7)`.
fn parenthesised(text: &str) -> Option<&str> {
    let inner = text.strip_suffix(')')?;
    let (_, inside) = inner.rsplit_once(" (")?;
    Some(inside)
}

/// A logon's target account as EvtxTriage writes it, `Target: D\N` or
/// `Target: N` (ruling S6), read as `account` reads the part after
/// `Target: `; `None` when the text does not start so or that part is no
/// account (blank or `-`). The logon summary counts by this same reading.
pub(super) fn logon_target(text: &str) -> Option<(String, Option<String>)> {
    text.strip_prefix(TARGET).and_then(account)
}

/// `text` as an account: `(name, domain)` split at the last `\`, the
/// domain `None` when there is no `\` or it is blank or `-`. `None` when
/// the name is blank or `-` (EvtxTriage's "no account"), or either part
/// is one `upsert_account` would refuse.
fn account(text: &str) -> Option<(String, Option<String>)> {
    let (domain, name) = match text.rsplit_once('\\') {
        Some((domain, name)) => (Some(domain), name),
        None => (None, text),
    };
    if name.trim() == "-" {
        return None;
    }
    check_line("account name", name, MAX_RECORD_NAME, true).ok()?;
    let domain = domain.filter(|d| !d.trim().is_empty() && d.trim() != "-");
    if let Some(domain) = domain {
        check_line("domain", domain, MAX_RECORD_NAME, false).ok()?;
    }
    Some((name.to_string(), domain.map(str::to_string)))
}

fn account_candidate(
    name: String,
    domain: Option<String>,
    sid: Option<String>,
    column: &str,
) -> Candidate {
    Candidate {
        kind: RecordKind::Account,
        ioc_type: None,
        value: name,
        domain,
        sid,
        column: column.to_string(),
        existing_id: None,
    }
}

/// `raw` as an IOC of type `t`, canonical; `None` when it does not
/// validate.
fn ioc_candidate(t: IocType, raw: &str, column: &str) -> Option<Candidate> {
    let value = validate_ioc(t, raw).ok()?;
    Some(Candidate {
        kind: RecordKind::Ioc,
        ioc_type: Some(t),
        value,
        domain: None,
        sid: None,
        column: column.to_string(),
        existing_id: None,
    })
}

/// Add `found` to `out` unless `out` already offers the same record: the
/// same kind and type, and the same value and domain with ASCII case
/// ignored, as the case's own uniqueness rules compare them.
fn offer(out: &mut Vec<Candidate>, found: Option<Candidate>) {
    let Some(found) = found else {
        return;
    };
    let same = |a: &Option<String>, b: &Option<String>| match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    };
    let offered = out.iter().any(|c| {
        c.kind == found.kind
            && c.ioc_type == found.ioc_type
            && c.value.eq_ignore_ascii_case(&found.value)
            && same(&c.domain, &found.domain)
    });
    if !offered {
        out.push(found);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_split_at_the_last_backslash() {
        let a = |t: &str| account(t);
        assert_eq!(a("jdoe"), Some(("jdoe".into(), None)));
        assert_eq!(a(r"ACME\jdoe"), Some(("jdoe".into(), Some("ACME".into()))));
        assert_eq!(a(r"a\b\jdoe"), Some(("jdoe".into(), Some(r"a\b".into()))));
        assert_eq!(a(r"-\jdoe"), Some(("jdoe".into(), None)));
        assert_eq!(a(r"\jdoe"), Some(("jdoe".into(), None)));
        for none in ["", " ", "-", r"-\-", r"ACME\", r"ACME\ ", "a\u{0}b"] {
            assert_eq!(a(none), None, "{none:?}");
        }
        assert_eq!(a(&"x".repeat(MAX_RECORD_NAME + 1)), None);
        assert!(a(&"x".repeat(MAX_RECORD_NAME)).is_some());
    }

    #[test]
    fn parenthesised_text() {
        assert_eq!(parenthesised("WS01 (203.0.113.7)"), Some("203.0.113.7"));
        assert_eq!(parenthesised("a (b) (::1)"), Some("::1"));
        assert_eq!(parenthesised("- (-)"), Some("-"));
        for none in ["", "WS01", "(1.2.3.4)", "WS01 (1.2.3.4", "WS01 (1.2.3.4) x"] {
            assert_eq!(parenthesised(none), None, "{none:?}");
        }
    }

    #[test]
    fn relation_patterns() {
        let around = Relations::Around("amcachetriage_", "_file_entries");
        assert!(around.hold("amcachetriage_associated_file_entries"));
        assert!(around.hold("amcachetriage_unassociated_file_entries"));
        assert!(!around.hold("amcachetriage_file_entries"));
        assert!(!around.hold("amcachetriage_program_entries"));
        let prefix = Relations::Prefix("retriage_typedurls_");
        assert!(prefix.hold("retriage_typedurls_ntuser_dat_default"));
        assert!(!prefix.hold("retriage_typedurls"));
        assert!(Relations::Named("petriage_main").hold("petriage_main"));
        assert!(!Relations::Named("petriage_main").hold("petriage_main_x"));
    }

    #[test]
    fn one_offer_per_record() {
        let mut out = Vec::new();
        offer(
            &mut out,
            Some(account_candidate(
                "jdoe".into(),
                Some("ACME".into()),
                None,
                "a",
            )),
        );
        offer(
            &mut out,
            Some(account_candidate(
                "JDOE".into(),
                Some("acme".into()),
                None,
                "b",
            )),
        );
        offer(
            &mut out,
            Some(account_candidate("jdoe".into(), None, None, "c")),
        );
        offer(
            &mut out,
            ioc_candidate(IocType::Url, "http://a.example/", "d"),
        );
        offer(
            &mut out,
            ioc_candidate(IocType::Url, "HTTP://A.example/", "e"),
        );
        offer(&mut out, None);
        let columns: Vec<&str> = out.iter().map(|c| c.column.as_str()).collect();
        assert_eq!(columns, ["a", "c", "d"]);
    }
}
