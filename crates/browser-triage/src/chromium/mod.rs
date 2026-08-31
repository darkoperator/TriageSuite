//! Chromium-family parsers and the enum decoders they share.
//!
//! Chrome, Edge, Brave, Opera, Vivaldi and Arc are all the same schema; the
//! brand only affects attribution, never the SQL.

pub mod autofill;
pub mod bookmarks;
pub mod cookies;
pub mod downloads;
pub mod extensions;
pub mod history;
pub mod keywords;
pub mod logins;

/// Core page-transition types, the low byte of `visits.transition`.
/// <https://source.chromium.org/chromium/chromium/src/+/main:ui/base/page_transition_types.h>
const TRANSITION_CORE: &[(i64, &str)] = &[
    (0, "Link"),
    (1, "Typed"),
    (2, "Auto Bookmark"),
    (3, "Auto Subframe"),
    (4, "Manual Subframe"),
    (5, "Generated"),
    (6, "Start Page"),
    (7, "Form Submit"),
    (8, "Reload"),
    (9, "Keyword"),
    (10, "Keyword Generated"),
];

/// Qualifier bits in the high bits of `visits.transition`. The redirect and
/// address-bar bits are what let a chain be told apart from a click.
const TRANSITION_QUALIFIERS: &[(i64, &str)] = &[
    (0x0100_0000, "Blocked"),
    (0x0200_0000, "Forward Back"),
    (0x0400_0000, "From Address Bar"),
    (0x0800_0000, "Home Page"),
    (0x1000_0000, "From API"),
    (0x2000_0000, "Chain Start"),
    (0x4000_0000, "Chain End"),
    (0x8000_0000, "Client Redirect"),
    (0x0080_0000, "Server Redirect"),
];

/// The decoded core type, or `Unknown (<n>)` so an unrecognized value is still
/// visible in the output rather than silently blank.
pub fn transition_core(transition: i64) -> String {
    let core = transition & 0xFF;
    TRANSITION_CORE
        .iter()
        .find(|(value, _)| *value == core)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| format!("Unknown ({core})"))
}

/// The set qualifier bits, `|`-joined, in table order.
pub fn transition_qualifiers(transition: i64) -> String {
    TRANSITION_QUALIFIERS
        .iter()
        .filter(|(bit, _)| transition & bit != 0)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join("|")
}

/// `downloads.state`. 3 is Chrome's historical `BUG_140687` slot, which real
/// profiles do contain, so it is named rather than reported as unknown.
const DOWNLOAD_STATES: &[(i64, &str)] = &[
    (0, "In Progress"),
    (1, "Complete"),
    (2, "Cancelled"),
    (3, "Interrupted (legacy)"),
    (4, "Interrupted"),
];

/// `downloads.danger_type` — the Safe Browsing verdict recorded at download
/// time, which is often the most interesting column in the table.
const DANGER_TYPES: &[(i64, &str)] = &[
    (0, "Not Dangerous"),
    (1, "Dangerous File"),
    (2, "Dangerous URL"),
    (3, "Dangerous Content"),
    (4, "Maybe Dangerous Content"),
    (5, "Uncommon Content"),
    (6, "User Validated"),
    (7, "Dangerous Host"),
    (8, "Potentially Unwanted"),
    (9, "Allowlisted By Policy"),
    (10, "Async Scanning"),
    (11, "Blocked Password Protected"),
    (12, "Blocked Too Large"),
    (13, "Sensitive Content Warning"),
    (14, "Sensitive Content Block"),
    (15, "Deep Scanned Safe"),
    (16, "Deep Scanned Opened Dangerous"),
    (17, "Prompt For Scanning"),
    (18, "Blocked Unsupported File Type"),
    (19, "Dangerous Account Compromise"),
];

/// `downloads.interrupt_reason`. The common values; anything else is reported
/// with its number rather than hidden.
const INTERRUPT_REASONS: &[(i64, &str)] = &[
    (0, "None"),
    (1, "File Failed"),
    (2, "File Access Denied"),
    (3, "File No Space"),
    (5, "File Name Too Long"),
    (6, "File Too Large"),
    (7, "File Virus Infected"),
    (10, "File Transient Error"),
    (11, "File Blocked"),
    (12, "File Security Check Failed"),
    (13, "File Too Short"),
    (14, "File Hash Mismatch"),
    (15, "File Same As Source"),
    (20, "Network Failed"),
    (21, "Network Timeout"),
    (22, "Network Disconnected"),
    (23, "Network Server Down"),
    (24, "Network Invalid Request"),
    (30, "Server Failed"),
    (31, "Server No Range"),
    (32, "Server Precondition"),
    (33, "Server Bad Content"),
    (34, "Server Unauthorized"),
    (35, "Server Certificate Problem"),
    (36, "Server Forbidden"),
    (37, "Server Unreachable"),
    (38, "Server Content Length Mismatch"),
    (39, "Server Cross Origin Redirect"),
    (40, "User Cancelled"),
    (41, "User Shutdown"),
    (50, "Crash"),
];

fn decode(table: &[(i64, &'static str)], value: i64) -> String {
    table
        .iter()
        .find(|(candidate, _)| *candidate == value)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| format!("Unknown ({value})"))
}

pub fn download_state(value: i64) -> String {
    decode(DOWNLOAD_STATES, value)
}

pub fn danger_type(value: i64) -> String {
    decode(DANGER_TYPES, value)
}

pub fn interrupt_reason(value: i64) -> String {
    decode(INTERRUPT_REASONS, value)
}

/// A `time_t` column where 0 means "never", not 1970-01-01.
///
/// `WinTimestamp::from_unix` deliberately accepts 0 as a real instant, because
/// for some artifacts it is one. Chromium's autofill columns are not among
/// them: `date_last_used = 0` means the entry was never reused, and rendering
/// that as the Unix epoch would invent a timestamp — the one thing the
/// timestamp module says never to do.
pub fn time_t_or_none(value: Option<i64>) -> triage_core::timestamp::WinTimestamp {
    match value {
        Some(0) | None => triage_core::timestamp::WinTimestamp::none(),
        Some(secs) => triage_core::timestamp::WinTimestamp::from_unix(secs),
    }
}

/// Lowercase hex for a blob column such as `downloads.hash`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_enums_decode_and_report_the_unknown() {
        assert_eq!(download_state(1), "Complete");
        assert_eq!(download_state(4), "Interrupted");
        assert_eq!(download_state(99), "Unknown (99)");
        assert_eq!(danger_type(0), "Not Dangerous");
        assert_eq!(danger_type(7), "Dangerous Host");
        assert_eq!(interrupt_reason(0), "None");
        assert_eq!(interrupt_reason(40), "User Cancelled");
        assert_eq!(interrupt_reason(1234), "Unknown (1234)");
    }

    #[test]
    fn hex_renders_a_hash_blob_lowercase() {
        assert_eq!(hex(&[0x00, 0x0f, 0xab, 0xff]), "000fabff");
        assert_eq!(hex(&[]), "");
    }

    #[test]
    fn core_transition_types_decode() {
        assert_eq!(transition_core(0), "Link");
        assert_eq!(transition_core(1), "Typed");
        assert_eq!(transition_core(7), "Form Submit");
    }

    /// The core type lives in the low byte, so qualifier bits must not disturb
    /// it — a typed visit with a redirect is still Typed.
    #[test]
    fn qualifier_bits_do_not_corrupt_the_core_type() {
        let typed_with_redirect = 1 | 0x8000_0000u32 as i64;
        assert_eq!(transition_core(typed_with_redirect), "Typed");
    }

    /// An unrecognized value stays visible instead of becoming an empty cell.
    #[test]
    fn an_unknown_core_type_is_reported_not_hidden() {
        assert_eq!(transition_core(200), "Unknown (200)");
    }

    #[test]
    fn qualifiers_decode_and_join() {
        let value = 0x0400_0000 | 0x2000_0000;
        assert_eq!(transition_qualifiers(value), "From Address Bar|Chain Start");
        assert_eq!(transition_qualifiers(0), "");
    }
}
