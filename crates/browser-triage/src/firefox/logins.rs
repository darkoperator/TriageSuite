//! Firefox `logins.json` — metadata only.
//!
//! Firefox encrypts **both** the username and the password with NSS, using a
//! key from `key4.db`. Chromium leaves the username in the clear, so this is
//! the one place where the shared schema's `Username Encrypted` column carries
//! its weight: a blank username here means "there is one, we chose not to
//! decrypt it", which an examiner must not read as "no username was stored".
//!
//! As on the Chromium side, no ciphertext is emitted in any encoding.

use crate::json;
use crate::profile::BrowserId;
use crate::records::LoginRecord;
use crate::sql::Notes;
use crate::timeline::{artifact_name, kind, Timeline};
use serde_json::Value;
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;

/// Decode base64 to a length only. The bytes are deliberately discarded: we
/// report how much ciphertext there is, never what it is.
fn base64_len(encoded: &str) -> Option<i64> {
    let trimmed = encoded.trim_end_matches('=');
    if trimmed.is_empty() {
        return None;
    }
    // 4 base64 characters encode 3 bytes.
    Some((trimmed.len() as i64 * 3) / 4)
}

pub fn parse(
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    let text = std::fs::read_to_string(path).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    let document: Value = serde_json::from_str(&text).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: format!("logins.json: {e}"),
    })?;

    let source = path.display().to_string();
    let mut written = 0u64;

    for entry in json::array(&document, "logins") {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }

        // Firefox renamed these keys; older profiles use the first spelling.
        let origin_url = {
            let modern = json::text(entry, "origin");
            if modern.is_empty() {
                json::text(entry, "hostname")
            } else {
                modern
            }
        };
        let action_url = {
            let modern = json::text(entry, "formActionOrigin");
            if modern.is_empty() {
                json::text(entry, "formSubmitURL")
            } else {
                modern
            }
        };

        let encrypted_username = json::text(entry, "encryptedUsername");
        let encrypted_password = json::text(entry, "encryptedPassword");

        let date_created =
            WinTimestamp::from_unix_millis(json::int(entry, "timeCreated").unwrap_or_default());
        let date_last_used =
            WinTimestamp::from_unix_millis(json::int(entry, "timeLastUsed").unwrap_or_default());
        let date_password_modified = WinTimestamp::from_unix_millis(
            json::int(entry, "timePasswordChanged").unwrap_or_default(),
        );

        out.write(
            "logins",
            &LoginRecord {
                browser: id.browser.clone(),
                channel: id.channel.clone(),
                profile: id.profile.clone(),
                origin_url: origin_url.clone(),
                action_url,
                signon_realm: json::text(entry, "httpRealm"),
                // Never decrypted, so never populated for Firefox.
                username: String::new(),
                username_encrypted: if encrypted_username.is_empty() {
                    "False"
                } else {
                    "True"
                }
                .to_string(),
                password_present: if encrypted_password.is_empty() {
                    "False"
                } else {
                    "True"
                }
                .to_string(),
                password_encryption: if encrypted_password.is_empty() {
                    String::new()
                } else {
                    "NSS".to_string()
                },
                password_length: base64_len(&encrypted_password),
                username_element: json::text(entry, "usernameField"),
                password_element: json::text(entry, "passwordField"),
                times_used: json::int(entry, "timesUsed"),
                date_created,
                date_last_used,
                date_password_modified,
                date_received: WinTimestamp::none(),
                scheme: String::new(),
                blocklisted: String::new(),
                federation_url: String::new(),
                display_name: String::new(),
                login_id: json::int(entry, "id"),
                guid: json::text(entry, "guid"),
                notes: notes.into_string(),
                source_file: source.clone(),
            },
        )?;
        written += 1;

        for (timestamp, what) in [
            (date_created, kind::LOGIN_CREATED),
            (date_last_used, kind::LOGIN_LAST_USED),
            (date_password_modified, kind::PASSWORD_CHANGED),
        ] {
            timeline.push(out, timestamp, what, artifact_name::LOGINS, &origin_url)?;
        }
    }

    // `disabledHosts` records sites the user told Firefox never to save — the
    // counterpart to Chromium's blacklisted_by_user, and evidence of intent.
    for host in json::array(&document, "disabledHosts")
        .iter()
        .filter_map(Value::as_str)
    {
        out.write(
            "logins",
            &LoginRecord {
                browser: id.browser.clone(),
                channel: id.channel.clone(),
                profile: id.profile.clone(),
                origin_url: host.to_string(),
                username_encrypted: "False".to_string(),
                password_present: "False".to_string(),
                blocklisted: "True".to_string(),
                date_created: WinTimestamp::none(),
                date_last_used: WinTimestamp::none(),
                date_password_modified: WinTimestamp::none(),
                date_received: WinTimestamp::none(),
                notes: "disabledHosts entry: saving was declined for this site".to_string(),
                source_file: source.clone(),
                ..Default::default()
            },
        )?;
        written += 1;
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A length, never the bytes.
    #[test]
    fn base64_length_is_reported_without_decoding_the_content() {
        assert_eq!(base64_len("YWJjZA=="), Some(4));
        assert_eq!(base64_len(""), None);
        assert_eq!(base64_len("===="), None);
    }
}
