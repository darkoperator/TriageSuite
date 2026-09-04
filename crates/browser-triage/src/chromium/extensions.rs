//! Chromium extensions, read from `Preferences` / `Secure Preferences`.
//!
//! There is no extensions database; the installed set lives under
//! `extensions.settings` in the profile's preferences JSON, keyed by extension
//! ID. `Secure Preferences` holds the same shape for the protected store, and
//! both are parsed identically.
//!
//! The forensically interesting column is `Install Location`: `Unpacked (Load)`
//! means the extension was loaded from a directory rather than installed from a
//! store, which on a managed endpoint is a sideload.

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

/// `extensions.settings.<id>.location`.
const LOCATIONS: &[(i64, &str)] = &[
    (0, "Invalid"),
    (1, "Internal"),
    (2, "External Preference"),
    (3, "External Registry"),
    (4, "Unpacked (Load)"),
    (5, "Component"),
    (6, "External Preference Download"),
    (7, "External Policy Download"),
    (8, "Command Line"),
    (9, "External Policy"),
    (10, "External Component"),
];

/// `extensions.settings.<id>.state`.
const STATES: &[(i64, &str)] = &[(0, "Disabled"), (1, "Enabled"), (2, "Externally Killed")];

/// `disable_reasons`, a bitmask, transcribed from
/// `extensions/browser/disable_reason.h`.
///
/// The shift numbers are not contiguous: Chromium retired several reasons and
/// left their bits unused, so a table built by doubling from the previous entry
/// silently misnames everything above the first gap. The deprecated bits are
/// kept and labelled, because a profile written by an older Chrome can still
/// carry them.
const DISABLE_REASONS: &[(i64, &str)] = &[
    (1 << 0, "User Action"),
    (1 << 1, "Permissions Increase"),
    (1 << 2, "Reload"),
    (1 << 3, "Unsupported Requirement"),
    (1 << 4, "Sideload Wipeout"),
    (1 << 5, "Unknown From Sync (deprecated)"),
    (1 << 6, "Permissions Consent (deprecated)"),
    (1 << 7, "Known Disabled (deprecated)"),
    (1 << 8, "Not Verified"),
    (1 << 9, "Greylist"),
    (1 << 10, "Corrupted"),
    (1 << 11, "Remote Install"),
    (1 << 12, "Inactive Ephemeral App (deprecated)"),
    (1 << 13, "External Extension"),
    (1 << 14, "Update Required By Policy"),
    (1 << 15, "Custodian Approval Required"),
    (1 << 16, "Blocked By Policy"),
    (1 << 19, "Reinstall"),
    (1 << 20, "Not Allowlisted"),
    (1 << 22, "Published In Store Required By Policy"),
    (1 << 23, "Unsupported Manifest Version"),
    (1 << 24, "Unsupported Developer Extension"),
    (1 << 25, "Unknown"),
    (1 << 26, "Blocked By Cloud Policy Check"),
];

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

fn decode_bitmask(mask: Option<i64>) -> String {
    let Some(mask) = mask.filter(|m| *m != 0) else {
        return String::new();
    };
    let named: Vec<&str> = DISABLE_REASONS
        .iter()
        .filter(|(bit, _)| mask & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    if named.is_empty() {
        format!("Unknown ({mask})")
    } else {
        named.join("|")
    }
}

/// Merge the manifest's declared permissions with the ones actually granted,
/// deduplicated. Either alone understates what the extension can do.
fn merge_permissions(
    manifest: &Value,
    active: &Value,
    manifest_key: &str,
    active_key: &str,
) -> String {
    let mut all: Vec<String> = json::array(manifest, manifest_key)
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    for granted in json::array(active, active_key)
        .iter()
        .filter_map(Value::as_str)
    {
        if !all.iter().any(|p| p == granted) {
            all.push(granted.to_string());
        }
    }
    all.join("|")
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
        message: format!("Preferences: {e}"),
    })?;

    let Some(settings) = document
        .get("extensions")
        .and_then(|e| e.get("settings"))
        .and_then(Value::as_object)
    else {
        // A genuine Preferences file with no extensions installed, or some
        // other application's file that got this far. Neither is an error.
        return Ok(0);
    };

    let source = path.display().to_string();
    let mut written = 0u64;

    for (extension_id, entry) in settings {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }

        let manifest = entry.get("manifest").cloned().unwrap_or(Value::Null);
        let active = entry
            .get("active_permissions")
            .cloned()
            .unwrap_or(Value::Null);

        let state_value = json::int(entry, "state");
        let install_time = WinTimestamp::from_webkit_micros(
            json::int(entry, "install_time")
                .or_else(|| json::int(entry, "first_install_time"))
                .unwrap_or_default(),
        );
        let update_time = WinTimestamp::from_webkit_micros(
            json::int(entry, "last_update_time").unwrap_or_default(),
        );

        let name = json::text(&manifest, "name");
        notes.note_if_lossy("Name", &name);

        // Chrome keeps sparse entries for extensions it knows about but has not
        // fully installed — typically pre-seeded by sync or policy — carrying
        // only state, disable_reasons and active_permissions. They are real
        // records and are emitted, but an analyst reading a row of blanks
        // deserves to know it is a stub rather than a parsing failure.
        if manifest.is_null() && json::int(entry, "location").is_none() {
            notes.push(
                "settings stub: no manifest or location recorded, so this extension was known \
                 to the profile but not fully installed"
                    .to_string(),
            );
        }

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
                extension_id: extension_id.clone(),
                name: name.clone(),
                version: json::text(&manifest, "version"),
                description: json::text(&manifest, "description"),
                enabled: match state_value {
                    Some(1) => "True".to_string(),
                    Some(_) => "False".to_string(),
                    None => String::new(),
                },
                state: decode(STATES, state_value),
                disable_reasons: decode_bitmask(json::int(entry, "disable_reasons")),
                install_location: decode(LOCATIONS, json::int(entry, "location")),
                from_store: json::bool_str(entry, "from_webstore"),
                source_url: json::text(&manifest, "update_url"),
                install_time,
                update_time,
                install_path: json::text(entry, "path"),
                permissions: merge_permissions(&manifest, &active, "permissions", "api"),
                host_permissions: merge_permissions(
                    &manifest,
                    &active,
                    "host_permissions",
                    "explicit_host",
                ),
                manifest_version: json::int(&manifest, "manifest_version"),
                signed_state: String::new(),
                addon_type: if manifest.get("theme").is_some() {
                    "theme".to_string()
                } else {
                    "extension".to_string()
                },
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

    /// The sideload indicator has to be named, not left as a bare number.
    #[test]
    fn install_locations_decode_with_unpacked_named() {
        assert_eq!(decode(LOCATIONS, Some(1)), "Internal");
        assert_eq!(decode(LOCATIONS, Some(4)), "Unpacked (Load)");
        assert_eq!(decode(LOCATIONS, Some(99)), "Unknown (99)");
        assert_eq!(decode(LOCATIONS, None), "");
    }

    /// Pinned to `extensions/browser/disable_reason.h`. The shift numbers skip
    /// retired reasons, so these are asserted individually rather than against
    /// the table, which is how a doubled-from-the-previous-entry table shipped
    /// with six reasons misnamed.
    #[test]
    fn disable_reason_bits_match_the_upstream_header() {
        for (shift, name) in [
            (0, "User Action"),
            (1, "Permissions Increase"),
            (2, "Reload"),
            (3, "Unsupported Requirement"),
            (4, "Sideload Wipeout"),
            (8, "Not Verified"),
            (9, "Greylist"),
            (10, "Corrupted"),
            (11, "Remote Install"),
            (13, "External Extension"),
            (14, "Update Required By Policy"),
            (15, "Custodian Approval Required"),
            (16, "Blocked By Policy"),
        ] {
            assert_eq!(decode_bitmask(Some(1 << shift)), name, "1 << {shift}");
        }
    }

    #[test]
    fn disable_reason_bits_decode_and_join() {
        assert_eq!(decode_bitmask(Some(0)), "");
        assert_eq!(decode_bitmask(None), "");
        assert_eq!(decode_bitmask(Some(1)), "User Action");
        assert_eq!(
            decode_bitmask(Some((1 << 0) | (1 << 10))),
            "User Action|Corrupted"
        );
        assert_eq!(decode_bitmask(Some(1 << 30)), "Unknown (1073741824)");
    }

    /// Declared and granted permissions each understate the extension alone.
    #[test]
    fn permissions_merge_the_declared_and_the_granted() {
        let manifest = j!({"permissions": ["tabs", "storage"]});
        let active = j!({"api": ["storage", "cookies"]});
        assert_eq!(
            merge_permissions(&manifest, &active, "permissions", "api"),
            "tabs|storage|cookies"
        );
    }
}
