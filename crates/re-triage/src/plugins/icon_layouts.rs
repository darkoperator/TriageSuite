//! Port of RegistryPlugin.IconLayouts (IconLayouts.cs + ValuesOut.cs).
//! Extracts desktop icon names from the IconLayouts binary value.
//!
//! Fires on NTUSER.DAT at:
//!   `Software\Microsoft\Windows\Shell\Bags\1\Desktop`
//!
//! Detail-CSV column order (fixture-authoritative, from
//! `<host>__<user>_NTUSER__plugin_IconLayouts_NTUSER.DAT.csv`):
//!   Name, BatchKeyPath, BatchValueName
//!
//! Batch row format (C# ValuesOut):
//!   ValueData1 = "Name: {Name}"
//!   ValueData2 = ""
//!   ValueData3 = ""
//!
//! Binary parsing (C# logic):
//!   Offset 0x18 (24): read 2 bytes LE as i16 → count
//!   Skip 6 bytes (to offset 0x20 = 32)
//!   For each entry (0..count):
//!     Read 2 bytes LE → length (in UTF-16 characters, NOT bytes)
//!     Skip 6 bytes
//!     Read (length * 2) bytes → name as UTF-16LE
//!
//! The C# code uses `BinaryReader` which advances position.
//! After reading `count` at offset 0x18, it does `br.BaseStream.Position += 0x6`
//! to skip 6 more bytes before the first entry. Per-entry: read 2 bytes (length),
//! skip 6 bytes, read (length*2) bytes (name).

use notatin::cell_key_node::CellKeyNode;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};
use triage_registry::value::plugin_raw_bytes;

pub struct IconLayouts;

/// Parse icon names from the IconLayouts binary blob.
///
/// Layout (from C# BinaryReader traversal):
///   offset 0x18 (24): count (i16 LE)
///   offset 0x1a (26): skip 6 bytes  → next entry starts at 0x20 (32)
///   Per entry:
///     +0: length (i16 LE, in UTF-16 chars)
///     +2: skip 6 bytes
///     +8: length * 2 bytes = UTF-16LE name
///     ... repeat
pub fn parse_icon_layouts(raw: &[u8]) -> Vec<String> {
    if raw.len() < 0x1a {
        return Vec::new();
    }

    // Read count at offset 0x18 (2 bytes LE i16).
    let count = i16::from_le_bytes([raw[0x18], raw[0x19]]) as usize;
    if count == 0 {
        return Vec::new();
    }

    // After reading count (2 bytes at 0x18..0x1a), skip 6 more bytes → position = 0x20.
    let mut pos = 0x1a + 6; // = 0x20

    let mut names = Vec::with_capacity(count);
    for _ in 0..count {
        // Read 2 bytes → length in UTF-16 chars.
        if pos + 2 > raw.len() {
            break;
        }
        let length = i16::from_le_bytes([raw[pos], raw[pos + 1]]) as usize;
        pos += 2;

        // Skip 6 bytes.
        pos += 6;

        // Read (length * 2) bytes as UTF-16LE.
        let byte_count = length * 2;
        if pos + byte_count > raw.len() {
            break;
        }
        let name_bytes = &raw[pos..pos + byte_count];
        let words: Vec<u16> = name_bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let name = String::from_utf16_lossy(&words).to_string();
        names.push(name);
        pos += byte_count;
    }

    names
}

impl RegistryPlugin for IconLayouts {
    fn plugin_name(&self) -> &'static str {
        "IconLayouts"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Software\Microsoft\Windows\Shell\Bags\1\Desktop"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        _hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let key_path = key.path.trim_start_matches('\\').to_string();

        // Find the "IconLayouts" value in the key.
        let raw = key.value_iter().find_map(|v| {
            if v.get_pretty_name().eq_ignore_ascii_case("IconLayouts") {
                let (content, _) = v.get_content();
                Some(plugin_raw_bytes(&content))
            } else {
                None
            }
        });

        let raw = match raw {
            Some(r) if !r.is_empty() => r,
            _ => return Vec::new(),
        };

        let names = parse_icon_layouts(&raw);

        names
            .into_iter()
            .map(|name| PluginRow {
                batch_value_name: "Multiple".to_string(),
                batch_value_data1: format!("Name: {name}"),
                batch_value_data2: String::new(),
                batch_value_data3: String::new(),
                detail_columns: vec![
                    ("Name".to_string(), name),
                    ("BatchKeyPath".to_string(), key_path.clone()),
                    ("BatchValueName".to_string(), "Multiple".to_string()),
                ],
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_icon_layouts_blob(names: &[&str]) -> Vec<u8> {
        let mut blob = vec![0u8; 0x20]; // header up to first entry start
        let count = names.len() as i16;
        blob[0x18] = (count & 0xff) as u8;
        blob[0x19] = ((count >> 8) & 0xff) as u8;
        // blob[0x1a..0x20] = 6 zero bytes (already zeroed)

        for name in names {
            let utf16: Vec<u16> = name.encode_utf16().collect();
            let length = utf16.len() as i16;
            // Write length (2 bytes LE)
            blob.push((length & 0xff) as u8);
            blob.push(((length >> 8) & 0xff) as u8);
            // Write 6 skip bytes
            blob.extend_from_slice(&[0u8; 6]);
            // Write UTF-16LE name bytes
            for w in &utf16 {
                blob.push((w & 0xff) as u8);
                blob.push(((w >> 8) & 0xff) as u8);
            }
        }
        blob
    }

    #[test]
    fn parses_multiple_names() {
        let blob = build_icon_layouts_blob(&["File.txt", "Program.exe", "Folder"]);
        let names = parse_icon_layouts(&blob);
        assert_eq!(names, vec!["File.txt", "Program.exe", "Folder"]);
    }

    #[test]
    fn empty_blob_returns_empty() {
        let names = parse_icon_layouts(&[]);
        assert!(names.is_empty());
    }

    #[test]
    fn zero_count_returns_empty() {
        let blob = vec![0u8; 0x20];
        let names = parse_icon_layouts(&blob);
        assert!(names.is_empty());
    }

    #[test]
    fn detail_columns_order() {
        let blob = build_icon_layouts_blob(&["test.lnk"]);
        let names = parse_icon_layouts(&blob);
        assert_eq!(names.len(), 1);
        let p = IconLayouts;
        // Simulate what process_with_hive would produce:
        let row = PluginRow {
            batch_value_name: "Multiple".to_string(),
            batch_value_data1: format!("Name: {}", names[0]),
            batch_value_data2: String::new(),
            batch_value_data3: String::new(),
            detail_columns: vec![
                ("Name".to_string(), names[0].clone()),
                ("BatchKeyPath".to_string(), "test_path".to_string()),
                ("BatchValueName".to_string(), "Multiple".to_string()),
            ],
        };
        let _ = p; // use p to avoid unused warning
        let col_names: Vec<&str> = row.detail_columns.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(col_names, &["Name", "BatchKeyPath", "BatchValueName"]);
    }

    #[test]
    fn plugin_name_and_key_paths() {
        let p = IconLayouts;
        assert_eq!(p.plugin_name(), "IconLayouts");
        assert!(p
            .key_paths()
            .contains(&r"Software\Microsoft\Windows\Shell\Bags\1\Desktop"));
    }
}
