//! Chromium `Login Data` / `Login Data For Account` -> `logins`.
//!
//! # Metadata only, by design
//!
//! The password blob is never emitted in any form. What is reported is that a
//! credential exists, which scheme protects it, and how long it is. The same
//! reasoning as cookies applies, with more force: this is credential material,
//! and putting it in a triage CSV would make the output more sensitive than the
//! evidence. An end-to-end test greps every produced file for the password
//! bytes and their hex and base64 encodings.
//!
//! The username, by contrast, *is* plaintext in Chromium and is emitted: it is
//! ordinary account-attribution evidence, not a secret.

use crate::profile::BrowserId;
use crate::records::LoginRecord;
use crate::sql::{self, Notes};
use crate::timeline::{artifact_name, kind, Timeline};
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_sqlite::Database;

const SCHEMES: &[(i64, &str)] = &[
    (0, "HTML"),
    (1, "Basic"),
    (2, "Digest"),
    (3, "Other"),
    (4, "Username Only"),
];

/// Chrome wrote `logins.date_created` as a `time_t` before roughly M55 and as
/// WebKit microseconds after.
///
/// The two ranges do not overlap: a value below 1e11 cannot be WebKit
/// microseconds for any date after 1601-01-02, and *is* a plausible `time_t`
/// through the year 5138. Returns the timestamp and whether the legacy branch
/// fired, so the row can say so rather than silently presenting one epoch as
/// the other.
pub fn webkit_or_time_t(value: i64) -> (WinTimestamp, bool) {
    const MAX_PLAUSIBLE_TIME_T: u64 = 100_000_000_000;
    if value == 0 {
        (WinTimestamp::none(), false)
    } else if value.unsigned_abs() < MAX_PLAUSIBLE_TIME_T {
        // `unsigned_abs`, not `abs`: the value comes straight from an evidence
        // cell, and `i64::MIN.abs()` panics wherever overflow checks are on.
        (WinTimestamp::from_unix(value), true)
    } else {
        (WinTimestamp::from_webkit_micros(value), false)
    }
}

pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    if !db.table_exists("logins").unwrap_or(false) {
        return Ok(0);
    }
    let cols = sql::columns(db, "logins");
    let source = path.display().to_string();
    let mut written = 0u64;

    let sql_text = format!(
        "SELECT {} FROM logins ORDER BY date_created",
        sql::projection(
            &cols,
            &[
                "origin_url",
                "action_url",
                "signon_realm",
                "username_value",
                "password_value",
                "username_element",
                "password_element",
                "times_used",
                "date_created",
                "date_last_used",
                "date_password_modified",
                "date_received",
                "scheme",
                "blacklisted_by_user",
                "federation_url",
                "display_name",
                "id",
            ]
        )
    );
    let rows = db.query(&sql_text).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: format!("logins: {e}"),
    })?;

    for row in &rows {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }

        let password = sql::bytes(sql::cell(row, 4));
        let scheme_label = super::cookies::encryption_scheme(password);
        if scheme_label == "Unknown" && !password.is_empty() {
            notes.push("password_value has an unrecognized prefix".to_string());
        }

        let (date_created, legacy_epoch) =
            webkit_or_time_t(sql::int(sql::cell(row, 8)).unwrap_or_default());
        if legacy_epoch {
            notes.push("date_created read as a legacy time_t, not WebKit microseconds".to_string());
        }
        let date_last_used =
            WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 9)).unwrap_or_default());
        let date_password_modified =
            WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 10)).unwrap_or_default());
        let date_received =
            WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 11)).unwrap_or_default());

        let origin_url = sql::text(sql::cell(row, 0));
        let username = sql::text(sql::cell(row, 3));
        notes.note_if_lossy("Username", &username);

        let label = if username.is_empty() {
            origin_url.clone()
        } else {
            format!("{origin_url} ({username})")
        };

        out.write(
            "logins",
            &LoginRecord {
                browser: id.browser.clone(),
                channel: id.channel.clone(),
                profile: id.profile.clone(),
                origin_url: origin_url.clone(),
                action_url: sql::text(sql::cell(row, 1)),
                signon_realm: sql::text(sql::cell(row, 2)),
                username: username.clone(),
                // Chromium keeps the username in the clear.
                username_encrypted: "False".to_string(),
                password_present: if password.is_empty() { "False" } else { "True" }.to_string(),
                password_encryption: scheme_label.to_string(),
                password_length: Some(password.len() as i64),
                username_element: sql::text(sql::cell(row, 5)),
                password_element: sql::text(sql::cell(row, 6)),
                times_used: sql::int(sql::cell(row, 7)),
                date_created,
                date_last_used,
                date_password_modified,
                date_received,
                scheme: match sql::int(sql::cell(row, 12)) {
                    None => String::new(),
                    Some(v) => SCHEMES
                        .iter()
                        .find(|(candidate, _)| *candidate == v)
                        .map(|(_, name)| (*name).to_string())
                        .unwrap_or_else(|| format!("Unknown ({v})")),
                },
                blocklisted: sql::bool_str(sql::cell(row, 13)),
                federation_url: sql::text(sql::cell(row, 14)),
                display_name: sql::text(sql::cell(row, 15)),
                login_id: sql::int(sql::cell(row, 16)),
                guid: String::new(),
                notes: notes.into_string(),
                source_file: source.clone(),
            },
        )?;
        written += 1;

        for (timestamp, what) in [
            (date_created, kind::LOGIN_CREATED),
            (date_last_used, kind::LOGIN_LAST_USED),
            (date_password_modified, kind::PASSWORD_CHANGED),
            (date_received, kind::LOGIN_RECEIVED),
        ] {
            timeline.push(out, timestamp, what, artifact_name::LOGINS, &label)?;
        }
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two ranges do not overlap, which is what makes the split safe.
    #[test]
    fn the_legacy_and_modern_epochs_are_distinguished() {
        let (modern, legacy) = webkit_or_time_t(13_344_473_600_000_000);
        assert!(!legacy);
        assert_eq!(modern.to_string(), "2023-11-14T22:13:20.0000000Z");

        let (old, legacy) = webkit_or_time_t(1_700_000_000);
        assert!(legacy, "a time_t-sized value must take the legacy branch");
        assert_eq!(old.to_string(), "2023-11-14T22:13:20.0000000Z");
    }

    #[test]
    fn zero_is_unset_on_either_branch() {
        let (timestamp, legacy) = webkit_or_time_t(0);
        assert!(timestamp.is_none());
        assert!(!legacy);
    }

    /// The value is an unconstrained integer read from an evidence cell, so
    /// the extremes must not panic. `i64::MIN.abs()` overflows, which made a
    /// crafted `date_created` abort the parse wherever overflow checks are on.
    #[test]
    fn the_integer_extremes_do_not_panic() {
        for value in [i64::MIN, i64::MIN + 1, i64::MAX, -1] {
            let (timestamp, _) = webkit_or_time_t(value);
            // Out of representable range either way; the point is reaching here.
            assert!(timestamp.is_none() || !timestamp.to_string().is_empty());
        }
    }
}
