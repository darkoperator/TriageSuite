//! What a host, account or IOC record's fields may hold: IOC values by type
//! (ruling S4), SIDs, ATT&CK ids and privileges, and the text bounds every
//! record field keeps.
//!
//! Every function here takes text from outside (a CLI flag, an agent, a
//! cell of evidence) and is total: any string gives a value or `Invalid`,
//! never a panic. Each is bounded before it parses.

use super::{clip, is_technique_id};
use crate::error::{EngineError, Result};
use std::net::IpAddr;

/// The most characters an IOC value holds (S4).
pub const MAX_IOC_VALUE: usize = 4_096;
/// The most characters of a host or account name, or an account's domain.
pub const MAX_RECORD_NAME: usize = 256;
/// The most characters of a host's role or an IOC's tool.
pub const MAX_RECORD_LABEL: usize = 200;
/// The most characters of a record's note or an account's description.
pub const MAX_RECORD_NOTE: usize = 10_000;
/// The most characters of a SID as written: past any valid one.
const MAX_SID: usize = 200;
/// The most sub-authorities a SID carries (`SID_MAX_SUB_AUTHORITIES`).
const MAX_SUB_AUTHORITIES: usize = 15;
/// A SID's identifier authority is six bytes.
const MAX_AUTHORITY: u64 = (1 << 48) - 1;
/// The most characters of a domain or host name (S4).
const MAX_DOMAIN: usize = 253;
/// The most characters of one label of it (S4).
const MAX_LABEL: usize = 63;
/// The most characters of an email's local part.
const MAX_LOCAL_PART: usize = 64;

/// An IOC's type (ruling S4). Each has its own validation in `validate_ioc`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum IocType {
    Ip,
    Domain,
    Hostname,
    Url,
    Email,
    Md5,
    Sha1,
    Sha256,
    FileName,
    FilePath,
    RegistryKey,
    Service,
    ScheduledTask,
    Other,
}

impl IocType {
    /// Every type, in S4's order.
    pub const ALL: [IocType; 14] = [
        IocType::Ip,
        IocType::Domain,
        IocType::Hostname,
        IocType::Url,
        IocType::Email,
        IocType::Md5,
        IocType::Sha1,
        IocType::Sha256,
        IocType::FileName,
        IocType::FilePath,
        IocType::RegistryKey,
        IocType::Service,
        IocType::ScheduledTask,
        IocType::Other,
    ];

    /// The spelling stored in `ox.ioc.type`.
    pub fn as_str(&self) -> &'static str {
        match self {
            IocType::Ip => "ip",
            IocType::Domain => "domain",
            IocType::Hostname => "hostname",
            IocType::Url => "url",
            IocType::Email => "email",
            IocType::Md5 => "md5",
            IocType::Sha1 => "sha1",
            IocType::Sha256 => "sha256",
            IocType::FileName => "file_name",
            IocType::FilePath => "file_path",
            IocType::RegistryKey => "registry_key",
            IocType::Service => "service",
            IocType::ScheduledTask => "scheduled_task",
            IocType::Other => "other",
        }
    }

    /// The type `as_str` spells `text`; `Invalid` for any other text.
    pub fn parse(text: &str) -> Result<IocType> {
        IocType::ALL
            .into_iter()
            .find(|t| t.as_str() == text)
            .ok_or_else(|| {
                EngineError::Invalid(format!(
                    "{:?} is not an IOC type; one of {}",
                    clip(text, 64),
                    IocType::ALL.map(|t| t.as_str()).join(", ")
                ))
            })
    }
}

/// An account's privilege.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Privilege {
    Standard,
    LocalAdmin,
    DomainAdmin,
    Service,
    /// Not known: what a new account is unless it is told otherwise.
    #[default]
    Unknown,
}

impl Privilege {
    const ALL: [Privilege; 5] = [
        Privilege::Standard,
        Privilege::LocalAdmin,
        Privilege::DomainAdmin,
        Privilege::Service,
        Privilege::Unknown,
    ];

    /// The spelling stored in `ox.account.privilege`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Privilege::Standard => "standard",
            Privilege::LocalAdmin => "local_admin",
            Privilege::DomainAdmin => "domain_admin",
            Privilege::Service => "service",
            Privilege::Unknown => "unknown",
        }
    }

    /// The privilege `as_str` spells `text`, exactly; `Invalid` otherwise.
    pub fn parse(text: &str) -> Result<Privilege> {
        Privilege::ALL
            .into_iter()
            .find(|p| p.as_str() == text)
            .ok_or_else(|| {
                EngineError::Invalid(format!(
                    "{:?} is not a privilege; one of {}",
                    clip(text, 64),
                    Privilege::ALL.map(|p| p.as_str()).join(", ")
                ))
            })
    }
}

/// `raw` as an IOC of type `t` is stored (S4): checked against the rules
/// every value keeps (1 to `MAX_IOC_VALUE` characters, not blank, no
/// control character but tab) and its type's own, then in canonical form:
/// an ip as the standard library writes it (`::1`), a domain, hostname or
/// hash lower-case, anything else as written. `Invalid` naming the rule
/// otherwise.
pub fn validate_ioc(t: IocType, raw: &str) -> Result<String> {
    let invalid = |rule: &str| {
        EngineError::Invalid(format!(
            "{:?} is not a valid {} IOC: {rule}",
            clip(raw, 64),
            t.as_str()
        ))
    };
    let chars = raw.chars().count();
    if chars > MAX_IOC_VALUE {
        return Err(invalid(&format!(
            "a value is at most {MAX_IOC_VALUE} characters, not {chars}"
        )));
    }
    if raw.trim().is_empty() {
        return Err(invalid("a value is not blank"));
    }
    if raw.chars().any(|c| c.is_control() && c != '\t') {
        return Err(invalid("a value holds no control character but tab"));
    }
    match t {
        IocType::Ip => raw
            .parse::<IpAddr>()
            .map(|ip| ip.to_string())
            .map_err(|_| invalid("an IPv4 or IPv6 address")),
        IocType::Domain | IocType::Hostname => {
            if is_domain(raw) {
                Ok(raw.to_ascii_lowercase())
            } else {
                Err(invalid(
                    "1 to 253 characters of labels of 1 to 63 letters, digits or hyphens, \
                     joined by dots",
                ))
            }
        }
        IocType::Url => {
            if is_url(raw) {
                Ok(raw.to_string())
            } else {
                Err(invalid(
                    "a URL with scheme http, https or ftp and a host (a domain or an IP address)",
                ))
            }
        }
        IocType::Email => {
            if is_email(raw) {
                Ok(raw.to_string())
            } else {
                Err(invalid("local@domain, the domain a valid one"))
            }
        }
        IocType::Md5 => hash(raw, 32).ok_or_else(|| invalid("32 hexadecimal digits")),
        IocType::Sha1 => hash(raw, 40).ok_or_else(|| invalid("40 hexadecimal digits")),
        IocType::Sha256 => hash(raw, 64).ok_or_else(|| invalid("64 hexadecimal digits")),
        IocType::FileName => {
            if raw.contains(['\\', '/']) {
                Err(invalid(
                    "a file name holds no \\ or /; a path is a file_path",
                ))
            } else {
                Ok(raw.to_string())
            }
        }
        IocType::FilePath
        | IocType::RegistryKey
        | IocType::Service
        | IocType::ScheduledTask
        | IocType::Other => Ok(raw.to_string()),
    }
}

/// The type the I key preselects for `raw`: the first that `raw` validates
/// as, in the order sha256, sha1, md5, ip, url, email, domain; `None` when
/// it is none of them. The analyst confirms it.
pub fn guess_ioc_type(raw: &str) -> Option<IocType> {
    [
        IocType::Sha256,
        IocType::Sha1,
        IocType::Md5,
        IocType::Ip,
        IocType::Url,
        IocType::Email,
        IocType::Domain,
    ]
    .into_iter()
    .find(|&t| validate_ioc(t, raw).is_ok())
}

/// `raw` as a SID is stored: `S-1-<authority>-<sub>…`, with one to fifteen
/// sub-authorities, the authority at most 2^48 - 1 and each sub-authority
/// at most 2^32 - 1, all ASCII decimal. Stored canonical: `S` upper-case,
/// leading zeros dropped. `Invalid` otherwise.
pub fn validate_sid(raw: &str) -> Result<String> {
    let invalid = || {
        EngineError::Invalid(format!(
            "{:?} is not a SID: S-1-<authority>-<sub-authority>, with 1 to \
             {MAX_SUB_AUTHORITIES} sub-authorities, all decimal",
            clip(raw, 64)
        ))
    };
    if raw.len() > MAX_SID {
        return Err(invalid());
    }
    let mut parts = raw.split('-');
    if !matches!(parts.next(), Some("S" | "s")) || parts.next() != Some("1") {
        return Err(invalid());
    }
    let authority = parts.next().and_then(decimal).ok_or_else(invalid)?;
    if authority > MAX_AUTHORITY {
        return Err(invalid());
    }
    let mut subs = Vec::new();
    for part in parts {
        let sub = decimal(part)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(invalid)?;
        subs.push(sub);
    }
    if subs.is_empty() || subs.len() > MAX_SUB_AUTHORITIES {
        return Err(invalid());
    }
    let mut out = format!("S-1-{authority}");
    for sub in subs {
        out.push('-');
        out.push_str(&sub.to_string());
    }
    Ok(out)
}

/// `raw` as an ATT&CK technique id, `T1234` or `T1234.001`, unchanged;
/// `Invalid` otherwise.
pub fn validate_attack_id(raw: &str) -> Result<String> {
    if is_technique_id(raw) {
        return Ok(raw.to_string());
    }
    Err(EngineError::Invalid(format!(
        "{:?} is not an ATT&CK technique id (T1234 or T1234.001)",
        clip(raw, 64)
    )))
}

/// A record's one-line text field `what`, checked: at most `max`
/// characters and no control character; `required` refuses a blank one.
pub(super) fn check_line(what: &str, text: &str, max: usize, required: bool) -> Result<()> {
    if required && text.trim().is_empty() {
        return Err(EngineError::Invalid(format!("a {what} is not blank")));
    }
    let n = text.chars().count();
    if n > max {
        return Err(EngineError::Invalid(format!(
            "a {what} is at most {max} characters, not {n}"
        )));
    }
    if text.chars().any(char::is_control) {
        return Err(EngineError::Invalid(format!(
            "a {what} holds no control character"
        )));
    }
    Ok(())
}

/// A record's free text field `what` (a note, a description), checked: at
/// most `MAX_RECORD_NOTE` characters, and no control character but tab and
/// line breaks.
pub(super) fn check_text(what: &str, text: &str) -> Result<()> {
    let n = text.chars().count();
    if n > MAX_RECORD_NOTE {
        return Err(EngineError::Invalid(format!(
            "a {what} is at most {MAX_RECORD_NOTE} characters, not {n}"
        )));
    }
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
        return Err(EngineError::Invalid(format!(
            "a {what} holds no control character but tab and line breaks"
        )));
    }
    Ok(())
}

/// The value of `text` as 1 to 20 ASCII decimal digits; `None` otherwise
/// (a sign, a space, a non-ASCII digit, or a value past `u64`).
fn decimal(text: &str) -> Option<u64> {
    if text.is_empty() || text.len() > 20 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Whether `text` is 1 to 253 characters of labels `[A-Za-z0-9-]{1,63}`
/// joined by dots.
fn is_domain(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_DOMAIN
        && text.split('.').all(|label| {
            (1..=MAX_LABEL).contains(&label.len())
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// Whether `text` is `<scheme>://<authority>…` with scheme http, https or
/// ftp (ASCII case ignored) and an authority whose host, after any
/// `user@` and before any `:port`, is a domain, an IPv4 address or a
/// bracketed IPv6 one. Nothing in it is whitespace.
fn is_url(text: &str) -> bool {
    if text.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((scheme, rest)) = text.split_once("://") else {
        return false;
    };
    if !["http", "https", "ftp"]
        .iter()
        .any(|s| scheme.eq_ignore_ascii_case(s))
    {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if let Some(bracketed) = host_port.strip_prefix('[') {
        let Some((v6, after)) = bracketed.split_once(']') else {
            return false;
        };
        return v6.parse::<std::net::Ipv6Addr>().is_ok() && port_ok(after);
    }
    let (host, after) = match host_port.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (host_port, None),
    };
    let port_valid = after.is_none_or(|p| decimal(p).is_some_and(|n| n <= u64::from(u16::MAX)));
    port_valid && (host.parse::<std::net::Ipv4Addr>().is_ok() || is_domain(host))
}

/// Whether `after`, what follows a bracketed IPv6 host, is nothing or
/// `:<port>`.
fn port_ok(after: &str) -> bool {
    match after.strip_prefix(':') {
        None => after.is_empty(),
        Some(port) => decimal(port).is_some_and(|n| n <= u64::from(u16::MAX)),
    }
}

/// Whether `text` is `local@domain`: one `@`, a local part of 1 to 64
/// characters with no whitespace, and a valid domain.
fn is_email(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    !domain.contains('@')
        && (1..=MAX_LOCAL_PART).contains(&local.chars().count())
        && !local.chars().any(char::is_whitespace)
        && is_domain(domain)
}

/// `text` lower-cased when it is exactly `len` hexadecimal digits.
fn hash(text: &str, len: usize) -> Option<String> {
    (text.len() == len && text.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| text.to_ascii_lowercase())
}
