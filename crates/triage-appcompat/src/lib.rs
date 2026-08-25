//! Windows ShimCache (AppCompatCache) **Windows 10/11 ("10ts")** binary parser.
//! Shared by re-triage's RECmd AppCompatCache plugin and AppCompatTriage.
//!
//! Entry layout (each entry starts with the "10ts" signature):
//!   +0x00 4B  sig "10ts"       +0x04 4B unknown
//!   +0x08 4B  data_size (u32)  — bytes after the 12-byte fixed prefix
//!   +0x0C 2B  path_size (u16)
//!   +0x0E N   path (UTF-16LE, path_size bytes)
//!   +..   8B  FILETIME (u64 LE)
//!   +..   4B  entry_data_size (u32 LE)   \  the Data block
//!   +..   M   entry_data (entry_data_size bytes) /  — its last 4 bytes == 1 → executed
//! Total entry size = 12 + data_size; next entry = current + 12 + data_size.

use chrono::{DateTime, Datelike, Utc};

/// One decoded Win10 ShimCache entry.
#[derive(Debug, Clone)]
pub struct ShimEntry {
    /// Sequential 0-based position in cache order.
    pub position: usize,
    /// Program path as stored (raw; `\??\` NOT stripped here — callers strip if needed).
    pub path: String,
    /// Last-modified UTC time from the FILETIME, or None if zero.
    pub modified_time: Option<DateTime<Utc>>,
    /// NEW: raw FILETIME (100ns since 1601) as read; 0 when the field was 0.
    pub filetime: u64,
    /// NEW: execution flag — last 4 bytes of the entry Data block == 1 (i32 LE).
    pub executed: bool,
}

/// Parse a Win10 ShimCache binary blob into entries in cache order.
pub fn parse_win10(raw: &[u8]) -> Vec<ShimEntry> {
    if raw.len() < 4 {
        return Vec::new();
    }
    let entry_offset = u32::from_le_bytes(raw[0..4].try_into().unwrap()) as usize;
    if entry_offset >= raw.len() {
        return Vec::new();
    }
    let mut pos = entry_offset;
    let mut entries = Vec::new();

    while pos < raw.len() {
        if pos + 14 > raw.len() {
            break;
        }
        if &raw[pos..pos + 4] != b"10ts" {
            break;
        }
        let data_size = u32::from_le_bytes(raw[pos + 8..pos + 12].try_into().unwrap()) as usize;
        let path_size = u16::from_le_bytes(raw[pos + 12..pos + 14].try_into().unwrap()) as usize;

        let entry_end = pos + 12 + data_size;
        if entry_end > raw.len() {
            break;
        }
        if path_size + 8 > data_size {
            pos = entry_end;
            continue;
        }
        let path_start = pos + 14;
        let path_end = path_start + path_size;
        if path_end > entry_end {
            pos = entry_end;
            continue;
        }
        let path = decode_utf16le(&raw[path_start..path_end]);

        let ft_start = path_end;
        let ft_end = ft_start + 8;
        if ft_end > entry_end {
            pos = entry_end;
            continue;
        }
        let filetime = u64::from_le_bytes(raw[ft_start..ft_end].try_into().unwrap());
        let modified_time = filetime_to_utc(filetime);

        // NEW: Data block after the FILETIME: data_size_field (u32) + data bytes.
        // executed = last 4 bytes of the data block == 1 (i32 LE). Defensive: the
        // simplified unit-test entries omit the block → executed stays false.
        let mut executed = false;
        if ft_end + 4 <= entry_end {
            let ds = u32::from_le_bytes(raw[ft_end..ft_end + 4].try_into().unwrap()) as usize;
            let data_start = ft_end + 4;
            let data_end = data_start + ds;
            if ds >= 4 && data_end <= entry_end && data_end <= raw.len() {
                let last4 = &raw[data_end - 4..data_end];
                executed = i32::from_le_bytes(last4.try_into().unwrap()) == 1;
            }
        }

        entries.push(ShimEntry {
            position: entries.len(),
            path,
            modified_time,
            filetime,
            executed,
        });
        pos = entry_end;
    }
    entries
}

/// Decode a UTF-16LE byte slice; unpaired surrogates → U+FFFD.
fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect()
}

/// Windows FILETIME (100ns since 1601-01-01 UTC) → UTC DateTime. None if 0.
pub fn filetime_to_utc(ft: u64) -> Option<DateTime<Utc>> {
    if ft == 0 {
        return None;
    }
    const FILETIME_TO_UNIX_SECS: i64 = 11_644_473_600;
    let secs = (ft / 10_000_000) as i64 - FILETIME_TO_UNIX_SECS;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    DateTime::from_timestamp(secs, nanos)
}

/// True when a decoded time is the 1601 epoch year (AppCompatCacheParser nulls these).
pub fn is_year_1601(dt: &DateTime<Utc>) -> bool {
    dt.year() == 1601
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(path_utf16: &[u16], filetime: u64, data_tail: Option<i32>) -> Vec<u8> {
        let path_bytes: Vec<u8> = path_utf16.iter().flat_map(|c| c.to_le_bytes()).collect();
        let path_size = path_bytes.len() as u16;
        let mut entry = Vec::new();
        entry.extend_from_slice(b"10ts");
        entry.extend_from_slice(&0u32.to_le_bytes()); // unknown
                                                      // Build the tail (path_size field + path + ft + optional data block) to size data_size.
        let mut tail = Vec::new();
        tail.extend_from_slice(&path_size.to_le_bytes());
        tail.extend_from_slice(&path_bytes);
        tail.extend_from_slice(&filetime.to_le_bytes());
        if let Some(flag) = data_tail {
            // data block: DataSize(=4) + 4-byte flag
            tail.extend_from_slice(&4u32.to_le_bytes());
            tail.extend_from_slice(&flag.to_le_bytes());
        }
        let data_size = tail.len() as u32;
        entry.extend_from_slice(&data_size.to_le_bytes());
        entry.extend_from_slice(&tail);
        entry
    }

    fn make_blob(entries: Vec<Vec<u8>>) -> Vec<u8> {
        let mut blob = vec![0u8; 52];
        blob[0..4].copy_from_slice(&0x34u32.to_le_bytes());
        for e in entries {
            blob.extend_from_slice(&e);
        }
        blob
    }

    fn u16s(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn parses_path_position_and_modified() {
        let ft0: u64 = 132_699_000_000_000_000;
        let blob = make_blob(vec![
            make_entry(&u16s(r"C:\foo.exe"), ft0, None),
            make_entry(&u16s(r"C:\bar.exe"), 0, None),
        ]);
        let e = parse_win10(&blob);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].position, 0);
        assert_eq!(e[0].path, r"C:\foo.exe");
        assert!(e[0].modified_time.is_some());
        assert_eq!(e[0].filetime, ft0);
        assert!(!e[0].executed); // no data block
        assert_eq!(e[1].position, 1);
        assert!(e[1].modified_time.is_none());
        assert_eq!(e[1].filetime, 0);
    }

    #[test]
    fn executed_flag_from_data_block() {
        let blob = make_blob(vec![
            make_entry(&u16s(r"C:\run.exe"), 132_699_000_000_000_000, Some(1)), // executed
            make_entry(&u16s(r"C:\norun.exe"), 132_699_000_000_000_000, Some(0)), // not
        ]);
        let e = parse_win10(&blob);
        assert_eq!(e.len(), 2);
        assert!(e[0].executed, "last-4-bytes==1 → executed");
        assert!(!e[1].executed);
    }

    #[test]
    fn known_filetime_converts() {
        let ft: u64 = 0x01dad8578959b0c1; // 2024-07-17 14:42:23.8961857 UTC
        let dt = filetime_to_utc(ft).unwrap();
        assert_eq!(
            dt.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2024-07-17 14:42:23"
        );
    }

    #[test]
    fn zero_ft_none_and_empty_and_truncated_no_panic() {
        assert!(filetime_to_utc(0).is_none());
        assert!(parse_win10(&[]).is_empty());
        assert!(parse_win10(&[0x34, 0, 0, 0]).is_empty());
    }

    #[test]
    fn wrong_sig_stops() {
        let mut blob = vec![0u8; 52];
        blob[0..4].copy_from_slice(&0x34u32.to_le_bytes());
        blob.extend_from_slice(b"XXXX");
        assert!(parse_win10(&blob).is_empty());
    }
}
