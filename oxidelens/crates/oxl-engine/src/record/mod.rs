//! The case record: timeline entries, the rows attached to them as
//! evidence, and the host, account and IOC records linked to them.
//!
//! Every rule lives here, not in the CLI or the app: who may change an
//! entry (ruling S3: an agent only its own unreviewed ones, an analyst any),
//! what a field may hold, where an entry's time comes from (S8), and which
//! host record attaching a row links (S7). Every change writes its history
//! row in its own transaction.
//!
//! The record tables arrived with case format 3. A case opened read-only
//! below it has none of them: every reader here reads that absence as an
//! empty record, and every writer refuses (a read-write open has already
//! upgraded the case).

mod accounts;
mod candidates;
mod entry;
mod evidence;
mod export;
pub(crate) mod hosts;
mod iocs;
mod logons;
mod records;
mod state;
mod validate;

pub use entry::{
    delete_entry, edit_entry, entries, entry, new_entry, set_entry_status, Entry, EntryEdit,
    EntryLink, EntryOrder, EntryQuery, EntryStatus, MAX_DESCRIPTION, MAX_TECHNIQUES, MAX_TITLE,
};
pub use evidence::{
    attach_rows, detach_row, evidence, promote_rows, Evidence, EvidenceStanding, PromoteOutcome,
};

pub use export::{export_report, export_timeline_csv, export_workbook};
#[cfg(feature = "test-support")]
pub use export::{
    export_report_before_rename, export_report_paused, export_timeline_csv_before_rename,
    export_timeline_csv_paused, export_workbook_before_rename, export_workbook_paused,
};
#[cfg(feature = "test-support")]
pub use iocs::ioc_sightings_paused;

pub use accounts::{account_record, accounts, upsert_account, AccountRecord, AccountSpec};
pub use candidates::{candidates, Candidate};
pub use hosts::{host_record, hosts, upsert_host, HostRecord, HostSpec};
pub(crate) use iocs::ioc_texts;
pub use iocs::{
    ioc_record, ioc_sightings, iocs, upsert_ioc, IocQuery, IocRecord, IocSpec, Sightings,
};
pub use logons::{logon_summary, LogonCount, LogonOutcome, LogonSummary};
pub use records::{
    delete_record, link, merge_records, record_entries, set_record_status, unlink, RecordKind,
    RecordStatus,
};
pub use validate::{
    guess_ioc_type, validate_attack_id, validate_ioc, validate_sid, IocType, Privilege,
    MAX_IOC_VALUE, MAX_RECORD_LABEL, MAX_RECORD_NAME, MAX_RECORD_NOTE,
};

pub(crate) use state::{detach_there, holders, marks_relation, Holder, CURRENT_MARKS};

use crate::case::Case;
use crate::error::{EngineError, Result};

/// The first case format with the record tables.
const RECORD_FORMAT: u32 = 3;

/// The fourteen ATT&CK Enterprise tactics, by name, in the matrix's order
/// (TA0043 Reconnaissance through TA0040 Impact), as
/// <https://attack.mitre.org/tactics/enterprise/> lists them. An entry's
/// tactic is one of these, spelled exactly so.
pub const TACTICS: [&str; 14] = [
    "Reconnaissance",
    "Resource Development",
    "Initial Access",
    "Execution",
    "Persistence",
    "Privilege Escalation",
    "Defense Evasion",
    "Credential Access",
    "Discovery",
    "Lateral Movement",
    "Collection",
    "Command and Control",
    "Exfiltration",
    "Impact",
];

/// Whether `case` has the record tables.
pub(crate) fn has_records(case: &Case) -> bool {
    case.format() >= RECORD_FORMAT
}

/// `Refused` for a case without the record tables, which only a read-only
/// open of an older case leaves; a writer is never reached on one, but says
/// why if it is.
fn require_records(case: &Case) -> Result<()> {
    if has_records(case) {
        return Ok(());
    }
    Err(EngineError::Refused(format!(
        "the case is format {}, opened read-only; it has no timeline until it is \
         opened for writing, which upgrades it",
        case.format()
    )))
}

/// `Invalid` unless `tactic` is one of `TACTICS`, spelled exactly so.
fn check_tactic(tactic: &str) -> Result<()> {
    if TACTICS.contains(&tactic) {
        return Ok(());
    }
    Err(EngineError::Invalid(format!(
        "{:?} is not an ATT&CK Enterprise tactic; one of {}",
        clip(tactic, 64),
        TACTICS.join(", ")
    )))
}

/// Whether `id` is an ATT&CK technique id, `T` and four ASCII digits,
/// optionally `.` and three more: `T1059`, `T1059.001`. Total: any text
/// gives an answer, and a non-ASCII digit is not a digit.
fn is_technique_id(id: &str) -> bool {
    let b = id.as_bytes();
    let digits = |range: std::ops::Range<usize>| b[range].iter().all(u8::is_ascii_digit);
    match b.len() {
        5 => b[0] == b'T' && digits(1..5),
        9 => b[0] == b'T' && digits(1..5) && b[5] == b'.' && digits(6..9),
        _ => false,
    }
}

/// The first `max` characters of `text`, for quoting outside input in an
/// error without repeating all of it.
fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn technique_ids() {
        for good in ["T1059", "T1059.001", "T0000", "T9999.999"] {
            assert!(is_technique_id(good), "{good}");
        }
        for bad in [
            "",
            "T",
            "T105",
            "T10599",
            "T1059.",
            "T1059.01",
            "T1059.0011",
            "t1059",
            "T105a",
            "T1059x001",
            "T١٠٥٩",
            "TA0001",
        ] {
            assert!(!is_technique_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn clip_cuts_on_a_character() {
        assert_eq!(clip("abc", 2), "ab…");
        assert_eq!(clip("ab", 2), "ab");
        assert_eq!(clip("éé", 1), "é…");
    }
}
