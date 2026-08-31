//! Chromium `cookies`.
//!
//! # On not decrypting
//!
//! Chromium encrypts cookie values from version 80 onward. This parser reports
//! that a value exists, which scheme protects it and how long it is, and stops
//! there. Three reasons, in order of weight:
//!
//! 1. Chrome 127+ App-Bound Encryption (`v20`) binds the key to the machine and
//!    to SYSTEM, so a dead capture cannot be decrypted at all — claiming
//!    otherwise would be the bug.
//! 2. Emitting ciphertext, hex or base64 would put credential material into a
//!    triage CSV, moving the *output* into a different handling class than the
//!    evidence it came from.
//! 3. An empty `Value` next to `Value Encrypted = True` is honest. A blank cell
//!    with no explanation would read as "this cookie had no value".

use crate::profile::BrowserId;
use crate::records::CookieRecord;
use crate::sql::{self, Notes};
use crate::timeline::{artifact_name, kind, Timeline};
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_sqlite::Database;

/// The DPAPI blob magic, used on Chromium profiles predating the v10 scheme.
const DPAPI_MAGIC: &[u8] = &[0x01, 0x00, 0x00, 0x00, 0xD0, 0x8C, 0x9D, 0xDF];

const SAME_SITE: &[(i64, &str)] = &[(-1, "Unspecified"), (0, "None"), (1, "Lax"), (2, "Strict")];

const PRIORITY: &[(i64, &str)] = &[(0, "Low"), (1, "Medium"), (2, "High")];

const SOURCE_SCHEME: &[(i64, &str)] = &[(0, "Unset"), (1, "Non-Secure"), (2, "Secure")];

fn decode(table: &[(i64, &'static str)], value: Option<i64>) -> String {
    match value {
        None => String::new(),
        Some(v) => table
            .iter()
            .find(|(candidate, _)| *candidate == v)
            .map(|(_, name)| (*name).to_string())
            .unwrap_or_else(|| format!("Unknown ({v})")),
    }
}

/// Identify the protection scheme from the ciphertext prefix alone.
pub fn encryption_scheme(blob: &[u8]) -> &'static str {
    if blob.is_empty() {
        return "";
    }
    if blob.starts_with(DPAPI_MAGIC) {
        return "DPAPI";
    }
    match blob.get(..3) {
        Some(b"v10") => "v10",
        Some(b"v11") => "v11",
        Some(b"v20") => "v20",
        _ => "Unknown",
    }
}

pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    if !db.table_exists("cookies").unwrap_or(false) {
        return Ok(0);
    }
    let cols = sql::columns(db, "cookies");
    let source = path.display().to_string();
    let mut written = 0u64;

    // `secure`/`httponly`/`persistent` were renamed with an `is_` prefix, so
    // each is resolved against the live schema rather than assumed.
    let sql_text = format!(
        "SELECT creation_utc, host_key, name, path, value, encrypted_value, \
                expires_utc, last_access_utc, {secure}, {httponly}, {persistent}, \
                {samesite}, {priority}, {source_scheme}, {source_port}, \
                {last_update}, {top_frame} \
         FROM cookies ORDER BY creation_utc",
        secure = sql::alternatives(&cols, &["is_secure", "secure"], None),
        httponly = sql::alternatives(&cols, &["is_httponly", "httponly"], None),
        persistent =
            sql::alternatives(&cols, &["is_persistent", "persistent", "has_expires"], None),
        samesite = sql::alternatives(&cols, &["samesite", "firstpartyonly"], None),
        priority = sql::alternatives(&cols, &["priority"], None),
        source_scheme = sql::alternatives(&cols, &["source_scheme"], None),
        source_port = sql::alternatives(&cols, &["source_port"], None),
        last_update = sql::alternatives(&cols, &["last_update_utc"], None),
        top_frame = sql::alternatives(&cols, &["top_frame_site_key"], None),
    );

    let rows = db.query(&sql_text).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: format!("cookies: {e}"),
    })?;

    for row in &rows {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }

        let plaintext = sql::text(sql::cell(row, 4));
        let ciphertext = sql::bytes(sql::cell(row, 5));
        let scheme = encryption_scheme(ciphertext);
        if scheme == "Unknown" {
            notes.push("encrypted_value has an unrecognized prefix".to_string());
        }
        let value_length = if ciphertext.is_empty() {
            Some(plaintext.len() as i64)
        } else {
            Some(ciphertext.len() as i64)
        };

        let host = sql::text(sql::cell(row, 1));
        let name = sql::text(sql::cell(row, 2));
        notes.note_if_lossy("Host", &host);
        notes.note_if_lossy("Name", &name);

        let created =
            WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 0)).unwrap_or_default());
        let expires =
            WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 6)).unwrap_or_default());
        let last_accessed =
            WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 7)).unwrap_or_default());
        let last_updated =
            WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 15)).unwrap_or_default());

        // A session cookie is one that is not persistent. Reported from
        // whichever column this schema carries rather than inferred from a
        // zero expiry, which a persistent cookie can also have.
        let session_cookie = match sql::int(sql::cell(row, 10)) {
            Some(0) => "True".to_string(),
            Some(_) => "False".to_string(),
            None => String::new(),
        };

        let label = format!("{host} {name}");
        let record = CookieRecord {
            browser: id.browser.clone(),
            channel: id.channel.clone(),
            profile: id.profile.clone(),
            host: host.clone(),
            name: name.clone(),
            path: sql::text(sql::cell(row, 3)),
            value: plaintext,
            value_encrypted: if ciphertext.is_empty() {
                "False"
            } else {
                "True"
            }
            .to_string(),
            encryption_scheme: scheme.to_string(),
            value_length,
            created,
            last_accessed,
            last_updated,
            expires,
            session_cookie,
            secure: sql::bool_str(sql::cell(row, 8)),
            http_only: sql::bool_str(sql::cell(row, 9)),
            same_site: decode(SAME_SITE, sql::int(sql::cell(row, 11))),
            priority: decode(PRIORITY, sql::int(sql::cell(row, 12))),
            source_scheme: decode(SOURCE_SCHEME, sql::int(sql::cell(row, 13))),
            source_port: sql::int(sql::cell(row, 14)),
            top_frame_site: sql::text(sql::cell(row, 16)),
            origin_attributes: String::new(),
            notes: notes.into_string(),
            source_file: source.clone(),
        };
        out.write("cookies", &record)?;
        written += 1;

        for (timestamp, what) in [
            (created, kind::COOKIE_CREATED),
            (last_accessed, kind::COOKIE_LAST_ACCESSED),
            (last_updated, kind::COOKIE_LAST_UPDATED),
            (expires, kind::COOKIE_EXPIRES),
        ] {
            timeline.push(out, timestamp, what, artifact_name::COOKIES, &label)?;
        }
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encryption_schemes_are_identified_from_the_prefix() {
        assert_eq!(encryption_scheme(b"v10abcdef"), "v10");
        assert_eq!(encryption_scheme(b"v11abcdef"), "v11");
        assert_eq!(encryption_scheme(b"v20abcdef"), "v20");
        assert_eq!(
            encryption_scheme(&[0x01, 0x00, 0x00, 0x00, 0xD0, 0x8C, 0x9D, 0xDF, 0x11]),
            "DPAPI"
        );
        assert_eq!(encryption_scheme(b"\x00\x01\x02zzz"), "Unknown");
        assert_eq!(encryption_scheme(b""), "");
    }

    /// A short blob must not panic the slice.
    #[test]
    fn a_truncated_blob_is_unknown_rather_than_a_panic() {
        assert_eq!(encryption_scheme(b"v1"), "Unknown");
    }

    #[test]
    fn enum_columns_decode_and_report_the_unknown() {
        assert_eq!(decode(SAME_SITE, Some(-1)), "Unspecified");
        assert_eq!(decode(SAME_SITE, Some(2)), "Strict");
        assert_eq!(decode(SAME_SITE, Some(9)), "Unknown (9)");
        assert_eq!(decode(PRIORITY, None), "");
        assert_eq!(decode(SOURCE_SCHEME, Some(2)), "Secure");
    }
}
