//! `DestList` stream parser.
//!
//! The `DestList` stream inside an `*.automaticDestinations-ms` OLE compound
//! file is a 32-byte header followed by N variable-length entries. Each entry
//! carries MRU metadata (path, hostname, timestamps, pin status, interaction
//! count) plus four droid GUIDs from the same distributed-link-tracking scheme
//! used by LNK `TrackerDataBlock`s. The entry layout is **version-dependent**:
//! version 1 differs from versions 3/4 in field offsets and entry size.
//!
//! Ported field-for-field from Eric Zimmerman's `JumpList` (`DestList.cs`,
//! `DestListHeader.cs`, `DestListEntry.cs`). Every read is bounds-checked; the
//! parser never panics on malformed input.

use crate::ParseError;

/// 100-ns intervals between the Gregorian UUID epoch (1582-10-15) and the
/// Windows FILETIME epoch (1601-01-01). Subtracting this from a UUID-v1
/// timestamp yields a FILETIME. Identical to the constant in
/// `triage-lnk`'s extradata tracker decode.
const GREGORIAN_TO_FILETIME_100NS: u64 = 5_748_192_000_000_000;

/// Parsed `DestList` header (first 32 bytes of the stream).
#[derive(Debug, Clone)]
pub struct DestListHeader {
    pub version: u32,
    pub number_of_entries: u32,
    pub number_of_pinned_entries: u32,
    pub last_entry_number: u32,
    pub last_revision_number: u32,
}

/// One `DestList` entry: MRU metadata for a single recent item.
#[derive(Debug, Clone, Default)]
pub struct DestListEntry {
    pub entry_number: u32,
    pub mru_position: u32,
    pub path: String,
    pub hostname: String,
    /// Last-modified FILETIME (100ns since 1601-01-01).
    pub last_modified: u64,
    /// Creation FILETIME, derived from the file droid's UUID-v1 time field.
    /// `0` when the droid is not a version-1 UUID (no embedded time).
    pub creation_time: u64,
    pub pin_status: i32,
    pub interaction_count: i32,
    /// MAC of the originating machine (`xx:xx:..`, lowercase), from the file
    /// droid's UUID-v1 node field. `None` when the droid is not a v1 UUID.
    pub mac_address: Option<String>,
    pub file_birth_droid: String,
    pub file_droid: String,
    pub volume_birth_droid: String,
    pub volume_droid: String,
    /// Whether this entry carries a serialized property store (SPS). Only
    /// possible for versions > 1; always `false` for version 1.
    pub has_sps: bool,
}

/// A parsed `DestList` stream: header plus all entries.
#[derive(Debug, Clone)]
pub struct DestList {
    pub header: DestListHeader,
    pub entries: Vec<DestListEntry>,
}

// --- bounds-checked little-endian readers ---------------------------------

fn read_u16(data: &[u8], off: usize) -> Result<u16, ParseError> {
    let end = off
        .checked_add(2)
        .ok_or(ParseError::Corrupt("u16 overflow"))?;
    let b = data.get(off..end).ok_or(ParseError::Corrupt("u16 oob"))?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn read_u32(data: &[u8], off: usize) -> Result<u32, ParseError> {
    let end = off
        .checked_add(4)
        .ok_or(ParseError::Corrupt("u32 overflow"))?;
    let b = data.get(off..end).ok_or(ParseError::Corrupt("u32 oob"))?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u64(data: &[u8], off: usize) -> Result<u64, ParseError> {
    let end = off
        .checked_add(8)
        .ok_or(ParseError::Corrupt("u64 overflow"))?;
    let b = data.get(off..end).ok_or(ParseError::Corrupt("u64 oob"))?;
    Ok(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

/// Read a 16-byte GUID at `off`, returning it as a fixed array.
fn read_guid(data: &[u8], off: usize) -> Result<[u8; 16], ParseError> {
    let end = off
        .checked_add(16)
        .ok_or(ParseError::Corrupt("guid overflow"))?;
    let b = data.get(off..end).ok_or(ParseError::Corrupt("guid oob"))?;
    let mut g = [0u8; 16];
    g.copy_from_slice(b);
    Ok(g)
}

// --- droid / MAC decode (mirrors triage-lnk extradata) --------------------

/// Render a 16-byte GUID as lowercase `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`,
/// matching C# `Guid.ToString("D")` (first three fields little-endian, rest in
/// stored order).
fn guid_to_string(g: &[u8; 16]) -> String {
    let d1 = u32::from_le_bytes([g[0], g[1], g[2], g[3]]);
    let d2 = u16::from_le_bytes([g[4], g[5]]);
    let d3 = u16::from_le_bytes([g[6], g[7]]);
    format!(
        "{d1:08x}-{d2:04x}-{d3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]
    )
}

/// Is this GUID a version-1 (time-based) UUID? The version is the high nibble
/// of byte 7 (the stored time-hi-and-version byte).
fn is_uuid_v1(g: &[u8; 16]) -> bool {
    (g[7] >> 4) == 1
}

/// MAC from a v1 UUID node field: the last 6 bytes, lowercase colon-separated.
fn mac_from_droid(g: &[u8; 16]) -> String {
    let n = &g[10..16];
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        n[0], n[1], n[2], n[3], n[4], n[5]
    )
}

/// FILETIME from a v1 UUID 60-bit time field (bytes 0..8 LE, version nibble of
/// byte 7 masked off). `0` if the result predates the FILETIME epoch.
fn filetime_from_droid(g: &[u8; 16]) -> u64 {
    let mut ts = [0u8; 8];
    ts.copy_from_slice(&g[0..8]);
    ts[7] &= 0x0F;
    u64::from_le_bytes(ts).saturating_sub(GREGORIAN_TO_FILETIME_100NS)
}

/// Decode a length-prefixed (in UTF-16 code units, NOT bytes) UTF-16LE string.
/// `char_len` is the number of UTF-16 units; the byte length is `char_len*2`.
fn read_utf16(data: &[u8], off: usize, char_len: usize) -> Result<String, ParseError> {
    let byte_len = char_len
        .checked_mul(2)
        .ok_or(ParseError::Corrupt("utf16 overflow"))?;
    let end = off
        .checked_add(byte_len)
        .ok_or(ParseError::Corrupt("utf16 overflow"))?;
    let b = data.get(off..end).ok_or(ParseError::Corrupt("utf16 oob"))?;
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    Ok(String::from_utf16_lossy(&units))
}

/// Append JLECmd's `==> <friendly>` resolution to a raw `DestList` path,
/// mirroring `JumpList/Automatic/DestListEntry.cs` field-for-field.
///
/// Two shapes get a suffix (everything else passes through unchanged):
///  - `knownfolder:{GUID}` — the GUID after the last `{` (trailing `}` stripped)
///    is resolved to a friendly folder name, e.g.
///    `knownfolder:{374DE290-…} ==> Downloads`.
///  - `::{GUID}\N\::{GUID}` — each `\`-separated segment is scanned for a GUID
///    (with or without braces); a matched GUID is replaced by its friendly
///    name, non-GUID segments (e.g. `5`) pass through, and the rebuilt path is
///    appended, e.g. `::{26EE0668-…}\5\::{BB06C0E4-…} ==> ControlPanelHome\5\System`.
///
/// Unknown GUIDs resolve to the bare GUID string (JLECmd's `GetFolderNameFromGuid`
/// returns the id unchanged on a miss), so a divergence surfaces as a real
/// mismatch rather than being silently dropped.
fn friendly_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("knownfolder") {
        // `knownfolder:{GUID}` — take the substring after the last '{' and drop
        // the trailing '}', exactly as `Path.Split('{').Last()` then trim does.
        if let Some(brace) = rest.rfind('{') {
            let mut kf = &rest[brace + 1..];
            kf = kf.strip_suffix('}').unwrap_or(kf);
            let f_name = resolve_guid_segment(kf);
            return format!("{path} ==> {f_name}");
        }
        return path.to_string();
    }

    if path.starts_with("::") {
        let new_segs: Vec<String> = path
            .split('\\')
            .map(|seg| match extract_guid(seg) {
                Some(g) => resolve_guid_segment(&g),
                None => seg.to_string(),
            })
            .collect();
        return format!("{path} ==> {}", new_segs.join("\\"));
    }

    path.to_string()
}

/// Resolve a bare GUID (no braces) to its friendly name, falling back to the
/// GUID string itself when unmapped — matching JLECmd's resolver-on-miss.
fn resolve_guid_segment(guid: &str) -> String {
    triage_shellitems::guids::name_from_guid_str(guid)
        .map(|s| s.to_string())
        .unwrap_or_else(|| guid.to_string())
}

/// First GUID found in a path segment, returned WITHOUT surrounding braces.
/// Matches the canonical 8-4-4-4-12 hex shape; tolerates an optional `{}`
/// wrapper (mirrors JLECmd's regex + `Substring(1, len-2)` brace strip).
fn extract_guid(seg: &str) -> Option<String> {
    let bytes = seg.as_bytes();
    // Hyphen positions inside a 36-char GUID: 8,13,18,23.
    let is_hex = |c: u8| c.is_ascii_hexdigit();
    for start in 0..bytes.len() {
        if start + 36 > bytes.len() {
            break;
        }
        let cand = &bytes[start..start + 36];
        let shape_ok = cand.iter().enumerate().all(|(i, &c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => is_hex(c),
        });
        if shape_ok {
            return Some(seg[start..start + 36].to_string());
        }
    }
    None
}

impl DestList {
    /// Parse the raw `DestList` stream bytes. Never panics; every read is
    /// bounds-checked. Returns [`ParseError::Corrupt`] on truncation.
    pub fn parse(data: &[u8]) -> Result<DestList, ParseError> {
        if data.len() < 32 {
            return Err(ParseError::Corrupt("DestList shorter than header"));
        }

        let header = DestListHeader {
            version: read_u32(data, 0)?,
            number_of_entries: read_u32(data, 4)?,
            number_of_pinned_entries: read_u32(data, 8)?,
            // 12: UnknownCounter (float), skipped
            last_entry_number: read_u32(data, 16)?,
            // 20: Unknown1, skipped
            last_revision_number: read_u32(data, 24)?,
            // 28: Unknown2, skipped
        };

        let mut entries = Vec::new();
        let mut index = 32usize;
        let mut mru_pos = 0u32;

        if header.version == 1 {
            // Version 1: no SPS, path length at +112, path at +114.
            // Walk while there's room and we haven't exceeded the declared count.
            while index < data.len() && (entries.len() as u32) < header.number_of_entries {
                let path_len = read_u16(data, index + 112)? as usize;
                let entry_size = 114usize
                    .checked_add(
                        path_len
                            .checked_mul(2)
                            .ok_or(ParseError::Corrupt("v1 path overflow"))?,
                    )
                    .ok_or(ParseError::Corrupt("v1 entry overflow"))?;
                let end = index
                    .checked_add(entry_size)
                    .ok_or(ParseError::Corrupt("v1 entry overflow"))?;
                let raw = data
                    .get(index..end)
                    .ok_or(ParseError::Corrupt("v1 entry oob"))?;

                entries.push(parse_entry(raw, 1, mru_pos, path_len, false)?);
                index = end;
                mru_pos += 1;
            }
        } else {
            // Versions 3/4 (and any > 1): path length at +128, path at +130,
            // then a u32 SPS size at +entry_size. Total advance includes the
            // 4-byte SPS-size field plus the SPS payload.
            while index < data.len() {
                let path_len = read_u16(data, index + 128)? as usize;
                let entry_size = 130usize
                    .checked_add(
                        path_len
                            .checked_mul(2)
                            .ok_or(ParseError::Corrupt("path overflow"))?,
                    )
                    .ok_or(ParseError::Corrupt("entry overflow"))?;
                let sps_size = read_u32(
                    data,
                    index
                        .checked_add(entry_size)
                        .ok_or(ParseError::Corrupt("sps off overflow"))?,
                )? as usize;

                let end = index
                    .checked_add(entry_size)
                    .and_then(|v| v.checked_add(sps_size))
                    .and_then(|v| v.checked_add(4))
                    .ok_or(ParseError::Corrupt("entry+sps overflow"))?;
                if end > data.len() {
                    return Err(ParseError::Corrupt("entry+sps oob"));
                }
                let raw = data
                    .get(index..end)
                    .ok_or(ParseError::Corrupt("entry oob"))?;

                entries.push(parse_entry(
                    raw,
                    header.version,
                    mru_pos,
                    path_len,
                    sps_size > 0,
                )?);
                index = end;
                mru_pos += 1;
            }
        }

        Ok(DestList { header, entries })
    }
}

/// Parse a single entry slice. `path_len` is in UTF-16 code units; `has_sps`
/// reflects the trailing property store presence (version > 1 only).
fn parse_entry(
    raw: &[u8],
    version: u32,
    mru_position: u32,
    path_len: usize,
    has_sps: bool,
) -> Result<DestListEntry, ParseError> {
    // Fixed-offset GUIDs (same across versions).
    let volume_droid = read_guid(raw, 8)?;
    let file_droid = read_guid(raw, 24)?;
    let volume_birth_droid = read_guid(raw, 40)?;
    let file_birth_droid = read_guid(raw, 56)?;

    // Hostname: 16-byte field at +72. If byte[73] == 0 it's UTF-16, else ASCII.
    let hostname = {
        let field = raw.get(72..88).ok_or(ParseError::Corrupt("hostname oob"))?;
        if field[1] == 0 {
            let units: Vec<u16> = field
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let s = String::from_utf16_lossy(&units);
            s.split('\0').next().unwrap_or("").to_string()
        } else {
            let s: String = field.iter().map(|&b| b as char).collect();
            s.split('\0').next().unwrap_or("").to_string()
        }
    };

    let entry_number = read_u32(raw, 88)?;
    let last_modified = read_u64(raw, 100)?;
    let pin_status = read_u32(raw, 108)? as i32;

    let (interaction_count, raw_path) = if version > 1 {
        let ic = read_u32(raw, 116)? as i32;
        let p = read_utf16(raw, 130, path_len)?;
        (ic, p)
    } else {
        let p = read_utf16(raw, 114, path_len)?;
        (0, p)
    };
    // JLECmd resolves `knownfolder:`/`::` shell paths to a `==> <friendly>`
    // suffix in the DestList entry constructor; mirror it here.
    let path = friendly_path(&raw_path);

    // Droid-derived fields: MAC + creation time only when file droid is v1.
    let (mac_address, creation_time) = if is_uuid_v1(&file_droid) {
        (
            Some(mac_from_droid(&file_droid)),
            filetime_from_droid(&file_droid),
        )
    } else {
        (None, 0)
    };

    Ok(DestListEntry {
        entry_number,
        mru_position,
        path,
        hostname,
        last_modified,
        creation_time,
        pin_status,
        interaction_count,
        mac_address,
        file_birth_droid: guid_to_string(&file_birth_droid),
        file_droid: guid_to_string(&file_droid),
        volume_birth_droid: guid_to_string(&volume_birth_droid),
        volume_droid: guid_to_string(&volume_droid),
        has_sps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid version-3 DestList: 32-byte header + one entry
    /// with a known path, hostname, last-modified time, and a v1 file droid
    /// (so MAC + creation time decode). No SPS. Assert every parsed field.
    #[test]
    fn parses_synthetic_v3_single_entry() {
        let path = "C:\\Users\\test\\file.txt";
        let path_units: Vec<u16> = path.encode_utf16().collect();
        let path_len = path_units.len();

        // Version-1 file droid: predictable MAC + creation time.
        let mut file_droid = [0u8; 16];
        file_droid[0..4].copy_from_slice(&0x1122_3344u32.to_le_bytes());
        file_droid[4..6].copy_from_slice(&0x5566u16.to_le_bytes());
        file_droid[6..8].copy_from_slice(&0x1788u16.to_le_bytes()); // version nibble = 1
        file_droid[8] = 0x99;
        file_droid[9] = 0xAA;
        file_droid[10..16].copy_from_slice(&[0x00, 0x0C, 0x29, 0x3E, 0x1F, 0x2C]);

        // entry_size for v3 = 130 + path_len*2; full entry adds 4-byte SPS size.
        let entry_size = 130 + path_len * 2;
        let mut entry = vec![0u8; entry_size + 4];

        // GUIDs
        entry[24..40].copy_from_slice(&file_droid); // FileDroid @24
                                                    // hostname @72 (ASCII; byte[73] != 0 path). "WIN-HOST"
        let host = b"WIN-HOST";
        entry[72..72 + host.len()].copy_from_slice(host);
        // entry number @88
        entry[88..92].copy_from_slice(&7u32.to_le_bytes());
        // last modified @100
        let last_mod: u64 = 0x01D8_0000_0000_0000;
        entry[100..108].copy_from_slice(&last_mod.to_le_bytes());
        // pin status @108 = -1 (unpinned)
        entry[108..112].copy_from_slice(&(-1i32).to_le_bytes());
        // interaction count @116
        entry[116..120].copy_from_slice(&5u32.to_le_bytes());
        // path length @128 (in code units)
        entry[128..130].copy_from_slice(&(path_len as u16).to_le_bytes());
        // path @130
        for (i, u) in path_units.iter().enumerate() {
            entry[130 + i * 2..130 + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        // SPS size @entry_size = 0 (no SPS) — already zeroed.

        // Header (32 bytes): version 3, 1 entry.
        let mut stream = vec![0u8; 32];
        stream[0..4].copy_from_slice(&3u32.to_le_bytes()); // version
        stream[4..8].copy_from_slice(&1u32.to_le_bytes()); // number_of_entries
        stream[16..20].copy_from_slice(&7u32.to_le_bytes()); // last_entry_number
        stream.extend_from_slice(&entry);

        let dl = DestList::parse(&stream).expect("parses");
        assert_eq!(dl.header.version, 3);
        assert_eq!(dl.header.number_of_entries, 1);
        assert_eq!(dl.header.last_entry_number, 7);
        assert_eq!(dl.entries.len(), 1);

        let e = &dl.entries[0];
        assert_eq!(e.entry_number, 7);
        assert_eq!(e.mru_position, 0);
        assert_eq!(e.path, path);
        assert_eq!(e.hostname, "WIN-HOST");
        assert_eq!(e.last_modified, last_mod);
        assert_eq!(e.pin_status, -1);
        assert_eq!(e.interaction_count, 5);
        assert_eq!(e.mac_address.as_deref(), Some("00:0c:29:3e:1f:2c"));
        assert_eq!(e.file_droid, "11223344-5566-1788-99aa-000c293e1f2c");
        assert!(e.creation_time > 0);
        assert!(!e.has_sps);
    }

    /// Version-1 entry: path length at +112, path at +114, no SPS.
    #[test]
    fn parses_synthetic_v1_single_entry() {
        let path = "D:\\x";
        let path_units: Vec<u16> = path.encode_utf16().collect();
        let path_len = path_units.len();
        let entry_size = 114 + path_len * 2;
        let mut entry = vec![0u8; entry_size];
        entry[88..92].copy_from_slice(&3u32.to_le_bytes()); // entry number
        entry[108..112].copy_from_slice(&0u32.to_le_bytes()); // pin status
        entry[112..114].copy_from_slice(&(path_len as u16).to_le_bytes());
        for (i, u) in path_units.iter().enumerate() {
            entry[114 + i * 2..114 + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }

        let mut stream = vec![0u8; 32];
        stream[0..4].copy_from_slice(&1u32.to_le_bytes());
        stream[4..8].copy_from_slice(&1u32.to_le_bytes());
        stream.extend_from_slice(&entry);

        let dl = DestList::parse(&stream).expect("parses v1");
        assert_eq!(dl.entries.len(), 1);
        assert_eq!(dl.entries[0].entry_number, 3);
        assert_eq!(dl.entries[0].path, path);
        assert_eq!(dl.entries[0].interaction_count, 0);
        assert!(!dl.entries[0].has_sps);
    }

    /// `friendly_path` mirrors JLECmd's `==>` resolution for the three shapes
    /// the fixtures exercise, and leaves ordinary paths untouched.
    #[test]
    fn friendly_path_resolves_known_folder_and_clsid() {
        // knownfolder:{GUID} -> friendly folder name
        assert_eq!(
            friendly_path("knownfolder:{374DE290-123F-4565-9164-39C4925E467B}"),
            "knownfolder:{374DE290-123F-4565-9164-39C4925E467B} ==> Downloads"
        );
        // ::{GUID}\N\::{GUID} -> per-segment resolution, non-GUID segs kept
        assert_eq!(
            friendly_path(
                "::{26EE0668-A00A-44D7-9371-BEB064C98683}\\5\\::{BB06C0E4-D293-4F75-8A90-CB05B6477EEE}"
            ),
            "::{26EE0668-A00A-44D7-9371-BEB064C98683}\\5\\::{BB06C0E4-D293-4F75-8A90-CB05B6477EEE} \
             ==> ControlPanelHome\\5\\System"
        );
        // ordinary scheme paths and filesystem paths pass through unchanged
        assert_eq!(
            friendly_path("ms-gamingoverlay:///"),
            "ms-gamingoverlay:///"
        );
        assert_eq!(
            friendly_path("C:\\Users\\test\\file.txt"),
            "C:\\Users\\test\\file.txt"
        );
        // unknown GUID falls back to the bare GUID id (resolver-on-miss)
        assert_eq!(
            friendly_path("knownfolder:{00000000-0000-0000-0000-000000000000}"),
            "knownfolder:{00000000-0000-0000-0000-000000000000} ==> 00000000-0000-0000-0000-000000000000"
        );
    }

    /// Truncated stream (shorter than header) errors rather than panics.
    #[test]
    fn truncated_header_errors() {
        assert!(DestList::parse(&[0u8; 10]).is_err());
    }

    /// A v3 entry claiming a giant path length must not panic; it errors.
    #[test]
    fn giant_path_len_no_panic() {
        let mut stream = vec![0u8; 32 + 130 + 4];
        stream[0..4].copy_from_slice(&3u32.to_le_bytes());
        stream[4..8].copy_from_slice(&1u32.to_le_bytes());
        // path length @ 32+128 = 160 set to 0xFFFF
        stream[160..162].copy_from_slice(&0xFFFFu16.to_le_bytes());
        assert!(DestList::parse(&stream).is_err());
    }
}
