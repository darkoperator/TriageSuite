//! Windows shell item (IDList) parsing engine.
//!
//! Consumers (LNK target IDs, Jump Lists, shellbags) walk an IDList into a
//! `Vec<ShellItem>` and render each item's `value`. Unknown class types
//! degrade to a non-empty placeholder rather than failing — real IDLists
//! always contain surprises.

// `chunks_exact(2)` decodes UTF-16LE byte pairs throughout this crate.
// Newer clippy (not yet universally in use here) suggests `as_chunks::<2>()`
// instead; deferred until that API's stable-since version is confirmed
// compatible with this project's MSRV. `unknown_lints` keeps this harmless
// on older clippy that doesn't recognize the lint name yet.
#![allow(unknown_lints, clippy::chunks_exact_to_as_chunks)]

pub mod extension;
pub mod guids;
pub mod items;

/// One parsed shell item.
#[derive(Debug, Clone)]
pub struct ShellItem {
    pub class_type: u8,
    /// Display string (the unit of TargetIDAbsolutePath). May be empty for
    /// items that legitimately contribute no path component.
    pub value: String,
    /// beef0004 extension block, when present (file/dir entries).
    pub extension: Option<extension::Beef0004>,
    /// Raw item body bytes (without the 2-byte size prefix).
    pub raw: Vec<u8>,
    /// FAT modified time embedded in file/dir entries (DOSDate+DOSTime @ item
    /// body offset 0x08), UTC. None for non-file-entry items.
    pub modified: Option<chrono::DateTime<chrono::Utc>>,
}

/// Parse a LinkTargetIDList, decoding legacy (non-Unicode) short names with
/// `codepage`. Long (UTF-16) names are codepage-independent. Each item is
/// u16-size-prefixed; a 0x0000 size terminates. Never panics; truncated or
/// unknown items degrade gracefully.
pub fn parse_id_list_with_codepage(data: &[u8], codepage: u16) -> Vec<ShellItem> {
    let mut items = Vec::new();
    let mut off = 0usize;
    loop {
        if off + 2 > data.len() {
            break;
        }
        let size = u16::from_le_bytes([data[off], data[off + 1]]) as usize;
        if size == 0 {
            break;
        } // terminator
        if size < 2 || off + size > data.len() {
            break;
        }
        let body = &data[off + 2..off + size];
        items.push(items::parse_item_cp(body, codepage));
        off += size;
    }
    items
}

/// Parse a LinkTargetIDList using the default codepage (Windows-1252). Thin,
/// non-breaking wrapper over [`parse_id_list_with_codepage`].
pub fn parse_id_list(data: &[u8]) -> Vec<ShellItem> {
    parse_id_list_with_codepage(data, 1252)
}

/// LECmd TargetIDAbsolutePath: each item's value trimmed of '\\', joined by '\\'.
pub fn absolute_path(items: &[ShellItem]) -> String {
    if items.is_empty() {
        return String::new();
    }
    items
        .iter()
        .map(|i| i.value.trim_matches('\\').to_string())
        .collect::<Vec<_>>()
        .join("\\")
}
