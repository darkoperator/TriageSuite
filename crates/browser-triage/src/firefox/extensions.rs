//! Firefox `extensions.json` -> `addons[]`.
//!
//! `Signed State` is the column worth reading first here, as `Install Location`
//! is on the Chromium side. On a release build every add-on from AMO is signed;
//! `Missing` means the add-on was installed by policy, by a side-load into the
//! profile, or was tampered with after signing.

use crate::json;
use crate::profile::BrowserId;
use crate::records::ExtensionRecord;
use crate::sql::Notes;
use crate::timeline::{artifact_name, kind, Timeline};
use serde_json::Value;
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;

const SIGNED_STATES: &[(i64, &str)] = &[
    (-2, "Broken"),
    (-1, "Unknown"),
    (0, "Missing"),
    (1, "Preliminary"),
    (2, "Signed"),
    (3, "System"),
    (4, "Privileged"),
];

/// Firefox spreads "is this on?" across four independent flags. Summarize them
/// rather than picking one and losing the reason.
fn state_summary(entry: &Value) -> (String, String) {
    let active = json::bool_str(entry, "active") == "True";
    let mut reasons: Vec<&str> = Vec::new();
    if json::bool_str(entry, "userDisabled") == "True" {
        reasons.push("User Disabled");
    }
    if json::bool_str(entry, "appDisabled") == "True" {
        reasons.push("App Disabled");
    }
    if json::bool_str(entry, "softDisabled") == "True" {
        reasons.push("Soft Disabled");
    }
    let enabled = active && reasons.is_empty();
    let state = if enabled {
        "Enabled".to_string()
    } else if reasons.is_empty() {
        "Inactive".to_string()
    } else {
        reasons.join("|")
    };
    (if enabled { "True" } else { "False" }.to_string(), state)
}

/// The localized name and description live one level down.
fn default_locale(entry: &Value, key: &str) -> String {
    entry
        .get("defaultLocale")
        .map(|locale| json::text(locale, key))
        .unwrap_or_default()
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
        message: format!("extensions.json: {e}"),
    })?;

    let source = path.display().to_string();
    let mut written = 0u64;

    for entry in json::array(&document, "addons") {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }

        let extension_id = json::text(entry, "id");
        let name = default_locale(entry, "name");
        notes.note_if_lossy("Name", &name);

        let (enabled, state) = state_summary(entry);
        let install_time =
            WinTimestamp::from_unix_millis(json::int(entry, "installDate").unwrap_or_default());
        let update_time =
            WinTimestamp::from_unix_millis(json::int(entry, "updateDate").unwrap_or_default());

        let source_uri = json::text(entry, "sourceURI");
        let install_path = {
            let direct = json::text(entry, "path");
            if direct.is_empty() {
                json::text(entry, "rootURI")
            } else {
                direct
            }
        };

        let permissions = entry.get("userPermissions").cloned().unwrap_or(Value::Null);

        let label = if name.is_empty() {
            extension_id.clone()
        } else {
            format!("{name} ({extension_id})")
        };

        out.write(
            "extensions",
            &ExtensionRecord {
                browser: id.browser.clone(),
                channel: id.channel.clone(),
                profile: id.profile.clone(),
                extension_id,
                name,
                version: json::text(entry, "version"),
                description: default_locale(entry, "description"),
                enabled,
                state,
                // Firefox has no disable_reasons bitmask; the flags are
                // summarized into State instead.
                disable_reasons: String::new(),
                install_location: json::text(entry, "location"),
                from_store: if source_uri.contains("addons.mozilla.org") {
                    "True".to_string()
                } else if source_uri.is_empty() {
                    String::new()
                } else {
                    "False".to_string()
                },
                source_url: source_uri,
                install_time,
                update_time,
                install_path,
                permissions: json::joined_strings(&permissions, "permissions"),
                host_permissions: json::joined_strings(&permissions, "origins"),
                manifest_version: json::int(entry, "manifestVersion"),
                signed_state: super::decode(SIGNED_STATES, json::int(entry, "signedState")),
                addon_type: json::text(entry, "type"),
                notes: notes.into_string(),
                source_file: source.clone(),
            },
        )?;
        written += 1;

        timeline.push(
            out,
            install_time,
            kind::EXTENSION_INSTALLED,
            artifact_name::EXTENSIONS,
            &label,
        )?;
        timeline.push(
            out,
            update_time,
            kind::EXTENSION_UPDATED,
            artifact_name::EXTENSIONS,
            &label,
        )?;
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json as j;

    /// An unsigned add-on on a release build is the thing worth spotting.
    #[test]
    fn signed_states_decode_with_missing_named() {
        assert_eq!(super::super::decode(SIGNED_STATES, Some(2)), "Signed");
        assert_eq!(super::super::decode(SIGNED_STATES, Some(0)), "Missing");
        assert_eq!(super::super::decode(SIGNED_STATES, Some(-1)), "Unknown");
        assert_eq!(super::super::decode(SIGNED_STATES, None), "");
    }

    /// Four independent flags decide whether an add-on runs; collapsing them to
    /// one boolean would lose why it is off.
    #[test]
    fn the_disable_flags_are_summarized_rather_than_collapsed() {
        let (enabled, state) = state_summary(&j!({"active": true}));
        assert_eq!(enabled, "True");
        assert_eq!(state, "Enabled");

        let (enabled, state) = state_summary(&j!({"active": false, "userDisabled": true}));
        assert_eq!(enabled, "False");
        assert_eq!(state, "User Disabled");

        let (_, state) =
            state_summary(&j!({"active": false, "userDisabled": true, "appDisabled": true}));
        assert_eq!(state, "User Disabled|App Disabled");

        let (_, state) = state_summary(&j!({"active": false}));
        assert_eq!(
            state, "Inactive",
            "off with no reason given is still a state"
        );
    }
}
