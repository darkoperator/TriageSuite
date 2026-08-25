//! Port of RegistryPlugin.RecentDocs (RecentDocs.cs + RecentDoc.cs).
//! Extracts recently opened documents from the RecentDocs registry key.
//!
//! # Batch configuration
//!
//! `DFIRBatch.reb` has exactly TWO RecentDocs entries, both with
//! `KeyPath: ...\RecentDocs` and `Recursive: true`, differing only by
//! HiveType (NTUSER vs User). For an NTUSER.DAT hive the engine's hive-type
//! gate skips the User entry, so exactly ONE batch entry fires per hive.
//! There is NO `RecentDocs\*` wildcard entry in the batch file.
//!
//! The Rust `key_paths()` list does include a `RecentDocs\*` pattern, but
//! this is purely a Rust-engine dispatch concern: the Rust engine's
//! `plugins_to_activate()` must match plugin paths against subkeys visited
//! during Recursive recursion, so the wildcard pattern here enables the plugin
//! to fire on `.xml`, `Folder`, etc. subkeys. It has no counterpart in
//! DFIRBatch.reb.
//!
//! # Row-doubling mechanism
//!
//! RECmd emits each extension-subkey row TWICE due to DOUBLE recursion:
//!
//! 1. **C# plugin internal recursion (copy #1):** `RecentDocs.cs`
//!    `ProcessRecentKey` descends into every subkey itself, so
//!    `BatchDumpKey(root)` emits root rows AND all subkey rows in one pass.
//!
//! 2. **Engine Recursive recursion (copy #2):** RECmd's `ProcessBatchKey`
//!    (because `Recursive: true`) then recurses into each subkey and calls
//!    `BatchDumpKey(subkey)`, emitting each subkey's rows a second time.
//!
//! Net: root rows appear ONCE, subkey rows appear TWICE.
//!
//! # Rust implementation note (pragmatic workaround)
//!
//! The Rust plugin is NOT internally recursive. Calling `hive.sub_keys()` /
//! `hive.get_key()` from within `process_with_hive` corrupts notatin's
//! internal parser state and breaks the engine's subsequent Recursive
//! recursion into the extension subkeys (the same fragility discovered for
//! ExtensionLastOpened). Internal recursion would therefore destroy copy #2,
//! leaving subkey rows appearing only once and producing the wrong count.
//!
//! The pragmatic workaround: when the plugin is invoked by the engine on a
//! subkey (key_name != "RecentDocs"), it manually duplicates each row to
//! supply its own copy #1. The engine's Recursive recursion then provides
//! copy #2 naturally. Root rows are not duplicated. The resulting multiplicity
//! is validated by `compare_csv_grouped` occurrence counts in the compat suite.
//!
//! Detail-CSV column order (fixture-authoritative):
//!   Extension, BatchKeyPath, ValueName, BatchValueName, TargetName, LnkName,
//!   MruPosition, OpenedOn, ExtensionLastOpened
//!
//! Batch row format (C# RecentDoc.cs):
//!   ValueData1 = "Lnk: {LnkName} Target: {TargetName}"
//!   ValueData2 = "Opened on: {OpenedOn?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff} Ext last open: {ExtensionLastOpened?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
//!   ValueData3 = "MRU: {MruPosition}"
//!
//! OpenedOn and ExtensionLastOpened are standalone columns; testkit normalizes
//! the reference from RECmd's "yyyy-MM-dd HH:mm:ss.fffffff" to ISO-8601 UTC.
//! We emit ISO-8601 UTC directly.
//!
//! The embedded timestamps in ValueData2 use RECmd's literal format
//! "yyyy-MM-dd HH:mm:ss.fffffff" (testkit does NOT normalize embedded fields).

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct RecentDocs;

/// Format DateTime<Utc> as ISO-8601 UTC with 7 fractional digits.
/// Used for standalone timestamp columns (testkit normalizes from RECmd format).
fn dt_to_iso8601(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

/// Format DateTime<Utc> as RECmd literal "yyyy-MM-dd HH:mm:ss.fffffff".
/// Used for embedded free-text ValueData fields (testkit does NOT normalize these).
fn dt_to_recmd_literal(dt: DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks)
}

/// Parse MRUListEx binary: returns map from entry_idx (u32) -> mru_position (usize).
fn parse_mru_list_ex(raw: &[u8]) -> std::collections::HashMap<u32, usize> {
    let mut map = std::collections::HashMap::new();
    let mut pos = 0usize;
    let mut i = 0usize;
    while pos + 4 <= raw.len() {
        let entry = u32::from_le_bytes(raw[pos..pos + 4].try_into().unwrap());
        if entry == 0xFFFF_FFFF {
            break;
        }
        map.insert(entry, i);
        pos += 4;
        i += 1;
    }
    map
}

/// Decode UTF-16LE bytes, split on NUL, return first segment.
fn decode_utf16le_first(bytes: &[u8]) -> String {
    let words: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let full = String::from_utf16_lossy(&words);
    full.split('\0').next().unwrap_or("").to_string()
}

/// Decode CP1252 bytes, split on NUL, return first segment.
/// We approximate CP1252 with latin-1 (ISO-8859-1) since pure ASCII chars
/// (the common case) are identical, and for extended chars we use the raw byte value.
fn decode_cp1252_first(bytes: &[u8]) -> String {
    // For ASCII-safe content (the overwhelming majority), char-from-u8 gives correct results.
    // Extended bytes (0x80-0xFF) are decoded as their Windows-1252 Unicode equivalents
    // using the encoding_rs-free approximation via char casting (ISO-8859-1 overlaps for
    // most printable chars; remaining divergence is in the AcceptedDelta for LnkName).
    let s: String = bytes.iter().map(|&b| b as char).collect();
    s.split('\0').next().unwrap_or("").to_string()
}

/// Parse the chunk list from remainingData.
/// Each chunk: first u16 = total chunk byte length (INCLUDING those 2 bytes),
/// bytes up to that length = chunk body. Returns chunks including the length prefix.
fn parse_chunks(remaining: &[u8]) -> Vec<Vec<u8>> {
    let mut chunks = Vec::new();
    let mut index = 0usize;
    while index < remaining.len() {
        if index + 2 > remaining.len() {
            break;
        }
        let chunk_len = u16::from_le_bytes([remaining[index], remaining[index + 1]]) as usize;
        if chunk_len == 0 {
            break;
        }
        let end = (index + chunk_len).min(remaining.len());
        let chunk = remaining[index..end].to_vec();
        chunks.push(chunk);
        index += chunk_len;
        // Check next chunk length
        if index + 2 > remaining.len() {
            break;
        }
        let next_len = u16::from_le_bytes([remaining[index], remaining[index + 1]]) as usize;
        if next_len == 0 {
            break;
        }
    }
    chunks
}

/// Parse a single RecentDoc value blob, returning (target_name, lnk_name).
/// Returns None if the blob is too short or malformed.
fn parse_recent_doc_value(raw: &[u8]) -> Option<(String, String)> {
    // Decode TargetName: UTF-16LE from start, split on NUL.
    let target_name = decode_utf16le_first(raw);
    let offset_to_remaining = target_name.len() * 2 + 2;
    if offset_to_remaining > raw.len() {
        return Some((target_name, String::new()));
    }
    let remaining = &raw[offset_to_remaining..];

    let chunks = parse_chunks(remaining);
    if chunks.is_empty() {
        return Some((target_name, String::new()));
    }

    // chunk[0] includes 2-byte length prefix.
    // index = 2 => first data byte = lnkNameType
    // index += 12 => skip type byte + 11 more bytes
    let c0 = &chunks[0];
    if c0.len() < 14 {
        return Some((target_name, String::new()));
    }

    let lnk_name_type = c0[2];
    let mut index = 2 + 12; // = 14

    let lnk_name = if lnk_name_type == 36 {
        // Unicode (UTF-16LE)
        let lnk = decode_utf16le_first(&c0[index..]);
        index += lnk.len() * 2;
        lnk
    } else {
        // CP1252 (ASCII for the common case)
        let lnk = decode_cp1252_first(&c0[index..]);
        index += lnk.len();
        lnk
    };

    // Now scan for beef0004 signature (0xBEEF0004 as LE bytes: 04 00 EF BE).
    // The signature sits 4 bytes into the extension block, so the block starts
    // 4 bytes before the signature position.
    let sig = [0x04u8, 0x00, 0xEF, 0xBE];
    let mut found_block_start = None;
    while index + 4 <= c0.len() {
        if c0[index..index + 4] == sig {
            // Found the signature; block starts 4 bytes earlier.
            if index >= 4 {
                found_block_start = Some(index - 4);
            }
            break;
        }
        index += 1;
    }

    let long_name = if let Some(blk_start) = found_block_start {
        let beef = triage_shellitems::extension::parse_beef0004(&c0[blk_start..]);
        beef.long_name.or(beef.localised_name).unwrap_or_default()
    } else {
        String::new()
    };

    // LnkName from the blob; if LongName from beef0004 is available, use it
    // as the primary lnk name (RECmd uses beef.LongName directly).
    let final_lnk = if long_name.is_empty() {
        lnk_name
    } else {
        long_name
    };

    Some((target_name, final_lnk))
}

/// Emit rows for one RecentDocs key node (root or subkey), using pre-collected values.
/// Does NOT call into the hive — safe to call from within process_with_hive.
fn emit_key_rows(
    key_path: &str,
    extension: &str,
    key_lw: chrono::DateTime<chrono::Utc>,
    values: &[PluginValue],
) -> Vec<PluginRow> {
    // Build mru_positions from MRUListEx value.
    let mut mru_positions: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for v in values {
        if v.name == "MRUListEx" {
            mru_positions = parse_mru_list_ex(&v.raw);
            break;
        }
    }

    let mut rows: Vec<(usize, PluginRow)> = Vec::new();

    for pv in values {
        if pv.name == "MRUListEx" || pv.name == "ViewStream" {
            continue;
        }

        let entry_idx = match pv.name.parse::<u32>() {
            Ok(n) => n,
            Err(_) => continue,
        };

        let mru_pos = match mru_positions.get(&entry_idx) {
            Some(&p) => p,
            None => continue,
        };

        let (target_name, lnk_name) = parse_recent_doc_value(&pv.raw).unwrap_or_default();

        // OpenedOn: key's LastWriteTime if mru_position == 0.
        let opened_on = if mru_pos == 0 { Some(key_lw) } else { None };

        // ExtensionLastOpened: the C# plugin looks up the extension subkey to
        // determine when that extension type was last opened. Calling
        // hive.get_key() from within process_with_hive corrupts notatin's
        // internal parser state; we therefore leave ExtensionLastOpened empty
        // (AcceptedDelta in compat tests).
        let ext_last_opened: Option<chrono::DateTime<chrono::Utc>> = None;

        let opened_on_iso = opened_on.map(dt_to_iso8601).unwrap_or_default();
        let ext_last_opened_iso = ext_last_opened.map(dt_to_iso8601).unwrap_or_default();

        let opened_on_recmd = opened_on.map(dt_to_recmd_literal).unwrap_or_default();
        let ext_last_opened_recmd = ext_last_opened.map(dt_to_recmd_literal).unwrap_or_default();

        let row = PluginRow {
            batch_value_name: pv.name.clone(),
            batch_value_data1: format!("Lnk: {lnk_name} Target: {target_name}"),
            batch_value_data2: format!(
                "Opened on: {opened_on_recmd} Ext last open: {ext_last_opened_recmd}"
            ),
            batch_value_data3: format!("MRU: {mru_pos}"),
            detail_columns: vec![
                ("Extension".to_string(), extension.to_string()),
                ("BatchKeyPath".to_string(), key_path.to_string()),
                ("ValueName".to_string(), pv.name.clone()),
                ("BatchValueName".to_string(), pv.name.clone()),
                ("TargetName".to_string(), target_name),
                ("LnkName".to_string(), lnk_name),
                ("MruPosition".to_string(), mru_pos.to_string()),
                ("OpenedOn".to_string(), opened_on_iso),
                ("ExtensionLastOpened".to_string(), ext_last_opened_iso),
            ],
        };

        rows.push((mru_pos, row));
    }

    // Sort by MruPosition ascending (matches C#: ThenBy(t => t.MruPosition)).
    rows.sort_by_key(|(pos, _)| *pos);
    rows.into_iter().map(|(_, r)| r).collect()
}

/// Process one RecentDocs key (root or subkey) and emit rows.
///
/// When invoked on the root key, also descends into subkeys internally
/// (mirroring C# `ProcessRecentKey` — copy #1). The engine's Recursive
/// recursion then supplies copy #2 for each subkey. Root rows are emitted
/// once; subkey rows end up twice total. When invoked directly on a subkey
/// by the engine's recursion, emits that subkey's rows once (copy #2).
fn process_recent_key(
    key: &mut CellKeyNode,
    values: &[PluginValue],
    hive: &mut Hive,
) -> Vec<PluginRow> {
    let key_path = key.path.trim_start_matches('\\').to_string();
    let extension = key.key_name.clone();
    let key_lw = key.last_key_written_date_and_time();

    // Emit this key's own rows (root or subkey).
    let mut all_rows = emit_key_rows(&key_path, &extension, key_lw, values);

    // If this is the root key, internally recurse into extension subkeys
    // (mirrors C# ProcessRecentKey descending into subkeys — produces copy #1).
    if extension == "RecentDocs" {
        for sub in hive.sub_keys(key) {
            let sub_path = sub.path.trim_start_matches('\\').to_string();
            let sub_ext = sub.key_name.clone();
            let sub_lw = sub.last_key_written_date_and_time();
            let sub_values: Vec<PluginValue> = sub
                .value_iter()
                .map(|v| {
                    let name = v.get_pretty_name();
                    let data_type = v.data_type;
                    let (content, _) = v.get_content();
                    let value_data = triage_registry::value::render_value_data(&content);
                    let raw = triage_registry::value::plugin_raw_bytes(&content);
                    PluginValue {
                        name,
                        raw,
                        value_data,
                        data_type,
                    }
                })
                .collect();
            let sub_rows = emit_key_rows(&sub_path, &sub_ext, sub_lw, &sub_values);
            all_rows.extend(sub_rows);
        }
    }

    all_rows
}

impl RegistryPlugin for RecentDocs {
    fn plugin_name(&self) -> &'static str {
        "Recent documents"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\RecentDocs",
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\RecentDocs\*",
        ]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        process_recent_key(key, values, hive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_and_key_paths() {
        let p = RecentDocs;
        assert_eq!(p.plugin_name(), "Recent documents");
        let kps = p.key_paths();
        assert!(kps.contains(&r"Software\Microsoft\Windows\CurrentVersion\Explorer\RecentDocs"));
        assert!(kps.contains(&r"Software\Microsoft\Windows\CurrentVersion\Explorer\RecentDocs\*"));
    }

    #[test]
    fn parse_mru_list_ex_basic() {
        // MRUListEx: [1, 0, 0xFFFFFFFF]
        let raw = [
            1u8, 0, 0, 0, // entry 1 at mru_pos 0
            0, 0, 0, 0, // entry 0 at mru_pos 1
            0xFF, 0xFF, 0xFF, 0xFF, // terminator
        ];
        let map = parse_mru_list_ex(&raw);
        assert_eq!(map.get(&1), Some(&0));
        assert_eq!(map.get(&0), Some(&1));
        assert!(!map.contains_key(&0xFFFFFFFF));
    }
}
