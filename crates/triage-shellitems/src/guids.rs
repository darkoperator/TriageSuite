//! GUID -> friendly-name mapping, ported from EZ-tools `GuidMapping`.
//!
//! The full `guid|name` table is embedded so 0x1f / 0x2e root-folder items
//! render the same friendly names LECmd does (e.g. `This PC`, `Downloads`).
//! Unknown GUIDs fall back to the brace-wrapped GUID string, matching LECmd's
//! `GetDescriptionFromGuid` behaviour.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Raw embedded mapping file (`guid|name` per line, possibly BOM-prefixed).
const GUID_TABLE_RAW: &str = include_str!("../resources/guid_to_name.txt");

fn table() -> &'static HashMap<String, &'static str> {
    static MAP: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for line in GUID_TABLE_RAW.lines() {
            let line = line.trim_start_matches('\u{feff}');
            if let Some((guid, name)) = line.split_once('|') {
                let guid = guid.trim().to_ascii_lowercase();
                if !guid.is_empty() {
                    m.insert(guid, name.trim());
                }
            }
        }
        m
    })
}

/// Format 16 raw GUID bytes as a canonical lowercase GUID string. The first
/// three groups are little-endian and the last two big-endian, matching
/// `Utils.ExtractGuidFromShellItem`.
pub fn format_guid(b: &[u8]) -> Option<String> {
    if b.len() < 16 {
        return None;
    }
    Some(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    ))
}

/// Friendly name for a GUID given as a string (any casing, with or without
/// surrounding braces). Returns `Some(name)` only when the GUID is present in
/// the mapping table; `None` otherwise.
///
/// This mirrors JLECmd's `Utils.GetFolderNameFromGuid` lookup used when it
/// resolves a `DestList` `knownfolder:`/`::` path into a `==> <friendly>`
/// suffix — the same table that backs [`name_from_guid_bytes`].
pub fn name_from_guid_str(guid: &str) -> Option<&'static str> {
    let g = guid
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .to_ascii_lowercase();
    table().get(&g).copied()
}

/// Friendly name for 16 raw GUID bytes. Falls back to `{guid}` when the GUID
/// is not in the mapping table (LECmd renders unmapped GUIDs verbatim).
pub fn name_from_guid_bytes(b: &[u8]) -> String {
    match format_guid(b) {
        Some(g) => match table().get(&g) {
            Some(name) => (*name).to_string(),
            None => format!("{{{g}}}"),
        },
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_guid() {
        // 088e3905-0323-4b02-9826-5d99428e115f -> Downloads (raw mixed-endian)
        let raw = [
            0x05, 0x39, 0x8e, 0x08, 0x23, 0x03, 0x02, 0x4b, 0x98, 0x26, 0x5d, 0x99, 0x42, 0x8e,
            0x11, 0x5f,
        ];
        assert_eq!(
            format_guid(&raw).unwrap(),
            "088e3905-0323-4b02-9826-5d99428e115f"
        );
        assert_eq!(name_from_guid_bytes(&raw), "Downloads");
    }

    #[test]
    fn unknown_guid_falls_back_to_braced_string() {
        let raw = [0xffu8; 16];
        let v = name_from_guid_bytes(&raw);
        assert!(v.starts_with('{') && v.ends_with('}'));
    }
}
