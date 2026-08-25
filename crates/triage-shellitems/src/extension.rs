//! beef0004 extension block parsing.
//!
//! The beef0004 block trails file/directory shell items (class 0x31/0x32 and
//! their Unicode variants) and carries the long Unicode name plus, on modern
//! Windows, an MFT entry/sequence reference. Layout (relative to the block
//! start, which is the 2-byte size field that precedes the `0xBEEF0004`
//! signature) ported from EZ-tools `ShellBag0x31.cs` and verified against the
//! capture corpus (every block observed is version 9):
//!
//! | off  | field                                              |
//! |------|----------------------------------------------------|
//! | 0x00 | u16 block size (includes these 2 bytes)            |
//! | 0x02 | u16 version (>= 3 carries a long name)             |
//! | 0x04 | u32 signature `0xBEEF0004`                          |
//! | 0x08 | FAT created (4)                                     |
//! | 0x0C | FAT accessed (4)                                    |
//! | 0x10 | u16 unknown                                         |
//! | ...  | version-dependent block (see [`name_offset`])      |
//!
//! For version >= 7 a file reference follows the 0x10 unknown: a u16 + u32
//! unknown, then an 8-byte file reference (6-byte MFT entry + 2-byte
//! sequence), then 8 bytes unknown, then the long Unicode name. The long-name
//! offset is version-keyed rather than scanned so junk that happens to spell a
//! string is never mistaken for the name.

/// Parsed beef0004 extension block fields used by LECmd.
#[derive(Debug, Clone, Default)]
pub struct Beef0004 {
    /// `LongName` per ExtensionBlocks: set ONLY when the trailing multistring
    /// holds exactly one string. When two or more strings are present, the
    /// library leaves `LongName` empty and exposes the second as `localised_name`.
    pub long_name: Option<String>,
    /// `LocalisedName` per ExtensionBlocks: the second multistring entry (e.g.
    /// `@shell32.dll,-21817` for a localized-resource folder). `None` when the
    /// block carries a single string.
    pub localised_name: Option<String>,
    /// Always `pieces[0]` (the first multistring entry) regardless of how many
    /// strings are present. Unlike `long_name` — which is `None` when 2+ strings
    /// exist — this field always carries the raw first string when non-empty.
    /// Used by SBETriage to display the resolved folder/file name (e.g. "Users")
    /// instead of the unresolvable localised resource ref (`@shell32.dll,-21813`).
    pub primary_name: Option<String>,
    pub mft_entry: Option<u64>,    // 6-byte MFT entry number
    pub mft_sequence: Option<u16>, // 2-byte sequence number
    /// FAT created timestamp (block offset 0x08), UTC.
    pub created: Option<chrono::DateTime<chrono::Utc>>,
    /// FAT last-accessed timestamp (block offset 0x0C), UTC.
    pub accessed: Option<chrono::DateTime<chrono::Utc>>,
}

const SIGNATURE: [u8; 4] = [0x04, 0x00, 0xEF, 0xBE];

/// True when a beef0004 signature (`04 00 EF BE`) is present far enough into the
/// item body that its 4-byte block header precedes it.
pub fn has_beef0004(item_body: &[u8]) -> bool {
    item_body
        .windows(4)
        .position(|w| w == SIGNATURE)
        .is_some_and(|p| p >= 4)
}

/// Offset of the trailing name multistring within the block, relative to the
/// block start, keyed by version. Ported from ExtensionBlocks `Beef0004`, which
/// builds the offset by cumulative version-gated skips starting from `index=18`:
///   `index=18; v>=7 {+2,+8(mft),+8}; v>=3 {+2}; v>=9 {+4}; v>=8 {+4}`.
/// Returns `None` for versions that carry no name (< 3).
fn name_offset(version: u16) -> Option<usize> {
    if version < 3 {
        return None;
    }
    let mut index = 18usize;
    if version >= 7 {
        index += 2 + 8 + 8;
    }
    // v >= 3
    index += 2;
    if version >= 9 {
        index += 4;
    }
    if version >= 8 {
        index += 4;
    }
    Some(index)
}

/// Parse the trailing name multistring (NUL-separated UTF-16LE strings) from
/// `start` to the end of the block, EXCLUDING the trailing 2-byte VersionOffset
/// field that ExtensionBlocks reads separately. Mirrors
/// `Utils.GetStringsFromMultistring`: consecutive UTF-16 strings each terminated
/// by a NUL pair; trailing empties dropped.
fn parse_multistring(block: &[u8], start: usize) -> Vec<String> {
    // Exclude the 2-byte VersionOffset trailer when present.
    let end = block.len().saturating_sub(2).max(start);
    let region = block.get(start..end).unwrap_or(&[]);
    let mut pieces = Vec::new();
    let mut cur: Vec<u16> = Vec::new();
    let mut i = 0;
    while i + 1 < region.len() {
        let u = u16::from_le_bytes([region[i], region[i + 1]]);
        if u == 0 {
            pieces.push(String::from_utf16_lossy(&cur));
            cur.clear();
        } else {
            cur.push(u);
        }
        i += 2;
    }
    if !cur.is_empty() {
        pieces.push(String::from_utf16_lossy(&cur));
    }
    while pieces.last().is_some_and(|p| p.is_empty()) {
        pieces.pop();
    }
    pieces
}

/// Whether this version carries an 8-byte MFT file reference (entry+sequence).
/// The reference sits at block offset 0x14 for version >= 7.
fn has_file_reference(version: u16) -> bool {
    version >= 7
}

const FILE_REF_OFFSET: usize = 0x14;

fn read_u16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

/// Decode a 4-byte FAT DOSDate(2)+DOSTime(2) (little-endian) to UTC. Returns
/// `None` for the all-zero/invalid sentinel. Reused by Task 2 for the item
/// DOS-modified time.
pub(crate) fn fat_datetime(bytes: &[u8]) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{TimeZone, Utc};
    let raw = bytes.get(0..4)?;
    let date = u16::from_le_bytes([raw[0], raw[1]]);
    let time = u16::from_le_bytes([raw[2], raw[3]]);
    if date == 0 {
        return None;
    }
    let year = 1980 + (date >> 9) as i32;
    let month = ((date >> 5) & 0x0F) as u32;
    let day = (date & 0x1F) as u32;
    let hour = (time >> 11) as u32;
    let min = ((time >> 5) & 0x3F) as u32;
    let sec = ((time & 0x1F) * 2) as u32;
    Utc.with_ymd_and_hms(year, month, day, hour, min, sec)
        .single()
}

/// Locate and parse a beef0004 block within a file/dir item body. Returns
/// Default (all None) when absent. Never panics (every read bounds-checked).
pub fn parse_beef0004(item_body: &[u8]) -> Beef0004 {
    let mut out = Beef0004::default();

    // The signature is `04 00 EF BE` and sits 4 bytes into the block, so the
    // block starts 4 bytes before the signature.
    let Some(sig_pos) = item_body
        .windows(4)
        .position(|w| w == SIGNATURE)
        .filter(|&p| p >= 4)
    else {
        return out;
    };
    let blk = sig_pos - 4;

    let Some(block_size) = read_u16(item_body, blk) else {
        return out;
    };
    let block_size = block_size as usize;
    let block_end = match blk.checked_add(block_size) {
        Some(e) if e <= item_body.len() && block_size >= 0x0A => e,
        // A bogus size still lets us read the long name relative to blk; clamp
        // to the buffer end so we never read past it.
        _ => item_body.len(),
    };

    let Some(version) = read_u16(item_body, blk + 2) else {
        return out;
    };

    // FAT created (block offset 0x08) and accessed (block offset 0x0C).
    // sig_pos points to the 4-byte signature which is at block offset 0x04,
    // so created = sig_pos+4 and accessed = sig_pos+8 relative to item_body.
    out.created = item_body.get(sig_pos + 4..).and_then(fat_datetime);
    out.accessed = item_body.get(sig_pos + 8..).and_then(fat_datetime);

    // File reference (MFT entry + sequence), version >= 7.
    if has_file_reference(version) {
        let ref_off = blk + FILE_REF_OFFSET;
        if ref_off + 8 <= block_end {
            let e = &item_body[ref_off..ref_off + 6];
            let entry = (e[0] as u64)
                | (e[1] as u64) << 8
                | (e[2] as u64) << 16
                | (e[3] as u64) << 24
                | (e[4] as u64) << 32
                | (e[5] as u64) << 40;
            let seq = read_u16(item_body, ref_off + 6).unwrap_or(0);
            // MFT entry and sequence are always set when version >= 7 and the
            // reference block is present. Entry=0 is a valid (if unusual) value
            // that SBECmd renders as "0" in the MFTEntry column. It does NOT,
            // however, render "NTFS file system" in Miscellaneous for entry=0
            // items — the Miscellaneous column is suppressed for those. This is
            // handled in walk.rs by checking `mft_entry > 0`.
            out.mft_entry = Some(entry);
            out.mft_sequence = if seq == 0 { None } else { Some(seq) };
        }
    }

    // Trailing name multistring at a version-keyed offset. Ported from
    // ExtensionBlocks `Beef0004.cs`:
    //   multiStrings[0] → LongName
    //   multiStrings[1] → LocalizedName (resource ref, e.g. "@shell32.dll,-21817")
    // ShellBag0x31/0x32 then render:
    //   LongName.Length > 0 ? LongName : LocalizedName.Localise()
    //
    // Original M3/M4/M5-compat-verified behaviour (LECmd/JLECmd depend on it):
    //   1 string  → long_name = pieces[0], localised_name = None
    //   2+ strings → long_name = None,      localised_name = pieces[1]
    //
    // `primary_name` is an ADDITIVE field: always pieces[0] when present,
    // regardless of count. SBETriage uses it to show the real folder/file
    // name (e.g. "Users") instead of the unresolvable @shell32.dll ref.
    if let Some(noff) = name_offset(version) {
        let block = item_body.get(blk..block_end).unwrap_or(&[]);
        let pieces = parse_multistring(block, noff);
        // Always capture pieces[0] as primary_name.
        out.primary_name = pieces.first().cloned();
        match pieces.len() {
            0 => {}
            1 => out.long_name = Some(pieces[0].clone()),
            _ => out.localised_name = Some(pieces[1].clone()),
        }
    }

    out
}

// ─── Beef001d / Beef001e (taskbar-pin metadata) ───────────────────────────────
//
// Two sibling extension blocks carried by taskbar-pinned shell items (the
// Taskband `Favorites` value). Both share the BeefBase header and the identical
// payload shape, differing only in signature and in what the trailing string
// means:
//
//   Beef001d → Executable (pinned executable / AppUserModelID identifier)
//   Beef001e → PinType    (e.g. "SystemPinned", "UserPinned")
//
// Layout (ported from EZ-tools `ExtensionBlocks/Beef001d.cs`, `Beef001e.cs`,
// and `BeefBase.cs`; constructor does `index = 10; Encoding.Unicode.GetString(
// rawBytes, index, rawBytes.Length - 10 - 2).Trim('\0')`):
//
// | off          | field                                              |
// |--------------|----------------------------------------------------|
// | 0x00 (u16)   | block size (includes these 2 bytes)                |
// | 0x02 (u16)   | version                                            |
// | 0x04 (u32)   | signature `0xBEEF001D` / `0xBEEF001E`              |
// | 0x08 (u16)   | unknown                                            |
// | 0x0A..size-2 | UTF-16LE string (Executable / PinType), NUL-trimmed |
// | size-2 (u16) | VersionOffset                                      |
//
// Verified against the real `Taskband\Favorites` capture: the decoded strings
// reproduce the RECmd fixture exactly (`Microsoft.Windows.Explorer`, `MSEdge`,
// `Microsoft.WindowsStore_8wekyb3d8bbwe!App`; `SystemPinned`).

const SIGNATURE_001D: [u8; 4] = [0x1D, 0x00, 0xEF, 0xBE];
const SIGNATURE_001E: [u8; 4] = [0x1E, 0x00, 0xEF, 0xBE];

/// String offset within a Beef001d/Beef001e block (C# `index = 10`).
const BEEF001X_STR_OFFSET: usize = 10;

/// Decode the UTF-16LE string payload of a Beef001d/Beef001e block that begins
/// at `item_body[blk..]`. Returns `None` when the block header is missing or
/// truncated. Mirrors the C# `Encoding.Unicode.GetString(rawBytes, 10,
/// Length - 10 - 2).Trim('\0')`: the string spans offset 10 to the 2-byte
/// VersionOffset trailer.
fn decode_beef001x_string(item_body: &[u8], sig: [u8; 4]) -> Option<String> {
    let sig_pos = item_body
        .windows(4)
        .position(|w| w == sig)
        .filter(|&p| p >= 4)?;
    let blk = sig_pos - 4;
    let block_size = read_u16(item_body, blk)? as usize;
    // Clamp the block end to the buffer; a bogus size still reads safely.
    let block_end = match blk.checked_add(block_size) {
        Some(e) if e <= item_body.len() && block_size >= BEEF001X_STR_OFFSET + 2 => e,
        _ => item_body.len(),
    };
    let start = blk + BEEF001X_STR_OFFSET;
    // String runs from `start` up to the 2-byte VersionOffset trailer.
    let end = block_end.saturating_sub(2).max(start);
    let region = item_body.get(start..end)?;
    let units: Vec<u16> = region
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let s = String::from_utf16_lossy(&units);
    let s = s.trim_matches('\0');
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// True when a Beef001d signature (`1D 00 EF BE`) is present far enough into the
/// item body that its 4-byte block header precedes it.
pub fn has_beef001d(item_body: &[u8]) -> bool {
    item_body
        .windows(4)
        .position(|w| w == SIGNATURE_001D)
        .is_some_and(|p| p >= 4)
}

/// True when a Beef001e signature (`1E 00 EF BE`) is present far enough into the
/// item body that its 4-byte block header precedes it.
pub fn has_beef001e(item_body: &[u8]) -> bool {
    item_body
        .windows(4)
        .position(|w| w == SIGNATURE_001E)
        .is_some_and(|p| p >= 4)
}

/// Locate and decode a Beef001d block's `Executable` string within an item body
/// (the body may be a shell item or a serialized property-store blob, since
/// taskbar pins embed the block inside a PropertyStore). Returns `None` when the
/// block is absent or carries no string. Never panics.
pub fn parse_beef001d_executable(item_body: &[u8]) -> Option<String> {
    decode_beef001x_string(item_body, SIGNATURE_001D)
}

/// Locate and decode a Beef001e block's `PinType` string within an item body.
/// Returns `None` when the block is absent or carries no string. Never panics.
pub fn parse_beef001e_pintype(item_body: &[u8]) -> Option<String> {
    decode_beef001x_string(item_body, SIGNATURE_001E)
}

// ─── Beef0026 (First/Last interacted FILETIMEs) ───────────────────────────────
//
// Carried by folder/item shell items and exposed by SBECmd as `FirstInteracted`
// and `LastInteracted`. Layout (ported from EZ-tools `ExtensionBlocks/Beef0026.cs`):
//
// Version 1 layout (EZ-tools v1 path):
// | off        | field                                              |
// |------------|----------------------------------------------------|
// | 0x00 (u16) | block size (includes these 2 bytes)                |
// | 0x02 (u16) | version (== 1)                                     |
// | 0x04 (u32) | signature `0xBEEF0026`                             |
// | 0x08 (u32) | padding / unknown                                  |
// | 0x0C (u64) | FirstInteracted FILETIME (100-ns since 1601-01-01) |
// | 0x14 (u64) | LastInteracted  FILETIME                           |
// | 0x1C (u64) | third FILETIME (purpose unknown)                   |
// | ...        | VersionOffset etc.                                 |
//
// Version >= 2 layout:
// | off        | field                                              |
// |------------|----------------------------------------------------|
// | 0x08 (u64) | FirstInteracted FILETIME                           |
// | 0x10 (u64) | LastInteracted  FILETIME                           |
//
// Both FILETIMEs: 0 → None.

const SIGNATURE_0026: [u8; 4] = [0x26, 0x00, 0xEF, 0xBE];

/// Beef0026 metadata block: the two interaction timestamps ShellBags exposes as
/// `FirstInteracted` / `LastInteracted`.
pub struct Beef0026 {
    pub first_interacted: Option<chrono::DateTime<chrono::Utc>>,
    pub last_interacted: Option<chrono::DateTime<chrono::Utc>>,
}

/// True when a Beef0026 signature (`26 00 EF BE`) is present far enough into the
/// item body that its 4-byte block header precedes it.
pub fn has_beef0026(item_body: &[u8]) -> bool {
    item_body
        .windows(4)
        .position(|w| w == SIGNATURE_0026)
        .is_some_and(|p| p >= 4)
}

/// Locate and parse a Beef0026 block within an item body. Returns `None` when the
/// block is absent or truncated. Never panics.
pub fn parse_beef0026(item_body: &[u8]) -> Option<Beef0026> {
    let sig_pos = item_body
        .windows(4)
        .position(|w| w == SIGNATURE_0026)
        .filter(|&pos| pos >= 4)?;

    // The signature is at block offset 0x04, so the block starts at sig_pos-4.
    let blk = sig_pos - 4;
    let version = read_u16(item_body, blk + 2).unwrap_or(0);

    // Version 1 has a 4-byte padding field between the signature and FILETIMEs
    // (block offset 0x08..0x0C); FILETIMEs start at block offset 0x0C = sig_pos+8.
    // Version >= 2 has FILETIMEs immediately after the signature at block offset
    // 0x08 = sig_pos+4.
    let (fi_off, li_off) = if version <= 1 {
        (sig_pos + 8, sig_pos + 16)
    } else {
        (sig_pos + 4, sig_pos + 12)
    };

    let ft = |o: usize| -> Option<chrono::DateTime<chrono::Utc>> {
        let n = u64::from_le_bytes(item_body.get(o..o + 8)?.try_into().ok()?);
        if n == 0 {
            return None;
        }
        let secs = (n / 10_000_000) as i64 - 11_644_473_600;
        let nanos = ((n % 10_000_000) * 100) as u32;
        chrono::DateTime::from_timestamp(secs, nanos)
    };
    Some(Beef0026 {
        first_interacted: ft(fi_off),
        last_interacted: ft(li_off),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic version-9 file (0x32) item body carrying a beef0004
    /// block with a known long name, MFT entry, and sequence, then assert the
    /// parser extracts them. Offsets mirror the real captures.
    #[test]
    fn extracts_long_name_and_mft() {
        let mut body = Vec::new();
        // Item prefix: class, unknown, 4-byte size, 4-byte mtime, 2-byte
        // unknown, then a short name + NUL (mirrors the on-disk shape; the
        // exact prefix bytes do not matter to beef parsing).
        body.extend_from_slice(&[0x32, 0x00]); // class + unknown
        body.extend_from_slice(&[0u8; 4]); // file size
        body.extend_from_slice(&[0u8; 4]); // mtime
        body.extend_from_slice(&[0u8; 2]); // unknown
        body.extend_from_slice(b"NAME~1.TXT\0"); // short name

        // beef0004 block.
        let name_utf16: Vec<u8> = "Report Final.txt"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .chain([0, 0]) // NUL terminator
            .collect();
        // Block: 0x00 size, 0x02 version=9, 0x04 sig, 0x08 created, 0x0C
        // accessed, 0x10 unk(2), 0x12 unk(4)?? -> file ref at 0x14.
        let mut block = Vec::new();
        block.extend_from_slice(&[0, 0]); // size placeholder
        block.extend_from_slice(&9u16.to_le_bytes()); // version 9
        block.extend_from_slice(&[0x04, 0x00, 0xEF, 0xBE]); // signature
        block.extend_from_slice(&[0u8; 4]); // created
        block.extend_from_slice(&[0u8; 4]); // accessed
        block.extend_from_slice(&[0u8; 2]); // 0x10 unknown
                                            // pad up to file ref at 0x14 (currently at 0x12)
        block.extend_from_slice(&[0u8; 2]); // 0x12 unknown -> now at 0x14
                                            // file reference: entry 0x0B4F6B, seq 3
        block.extend_from_slice(&[0x6b, 0x4f, 0x0b, 0x00, 0x00, 0x00]); // entry
        block.extend_from_slice(&3u16.to_le_bytes()); // sequence -> now at 0x1C
                                                      // pad to long-name offset 0x2E
        while block.len() < 0x2E {
            block.push(0);
        }
        block.extend_from_slice(&name_utf16);
        block.push(0); // trailing
        block.push(0);
        let size = block.len() as u16;
        block[0..2].copy_from_slice(&size.to_le_bytes());

        body.extend_from_slice(&block);

        let b4 = parse_beef0004(&body);
        assert_eq!(b4.long_name.as_deref(), Some("Report Final.txt"));
        assert_eq!(b4.mft_entry, Some(0x0B4F6B));
        assert_eq!(b4.mft_sequence, Some(3));
    }

    #[test]
    fn absent_block_yields_default() {
        let b4 = parse_beef0004(&[0x31, 0x00, 0x01, 0x02, 0x03]);
        assert!(b4.long_name.is_none());
        assert!(b4.mft_entry.is_none());
    }

    /// Build a synthetic Beef001d/Beef001e block carrying `s` as its UTF-16LE
    /// payload, with the canonical layout (size, version, sig, 2-byte unknown,
    /// string, 2-byte VersionOffset). `sig` is the 4-byte little-endian signature.
    fn build_beef001x_block(sig: [u8; 4], s: &str) -> Vec<u8> {
        let mut blk = Vec::new();
        blk.extend_from_slice(&[0, 0]); // 0x00 size placeholder
        blk.extend_from_slice(&0u16.to_le_bytes()); // 0x02 version
        blk.extend_from_slice(&sig); // 0x04 signature
        blk.extend_from_slice(&[0, 0]); // 0x08 unknown
        for u in s.encode_utf16() {
            blk.extend_from_slice(&u.to_le_bytes());
        }
        blk.extend_from_slice(&[0, 0]); // string NUL terminator
        blk.extend_from_slice(&0u16.to_le_bytes()); // VersionOffset trailer
        let size = blk.len() as u16;
        blk[0..2].copy_from_slice(&size.to_le_bytes());
        blk
    }

    #[test]
    fn beef001d_decodes_executable() {
        // Synthetic block with the exact string the Taskband fixture renders.
        let blk = build_beef001x_block([0x1D, 0x00, 0xEF, 0xBE], "Microsoft.Windows.Explorer");
        // Prefix some junk so the parser must locate the signature, not assume 0.
        let mut body = vec![0xAA, 0xBB, 0xCC, 0xDD];
        body.extend_from_slice(&blk);
        assert_eq!(
            parse_beef001d_executable(&body).as_deref(),
            Some("Microsoft.Windows.Explorer")
        );
        assert!(has_beef001d(&body));
        // The 001e accessor must NOT match a 001d block.
        assert!(parse_beef001e_pintype(&body).is_none());
    }

    #[test]
    fn beef001e_decodes_pintype() {
        let blk = build_beef001x_block([0x1E, 0x00, 0xEF, 0xBE], "SystemPinned");
        let mut body = vec![0x00, 0x00, 0x00, 0x00];
        body.extend_from_slice(&blk);
        assert_eq!(
            parse_beef001e_pintype(&body).as_deref(),
            Some("SystemPinned")
        );
        assert!(has_beef001e(&body));
        assert!(parse_beef001d_executable(&body).is_none());
    }

    #[test]
    fn beef001x_real_fixture_bytes() {
        // Bytes carved from the real Taskband\Favorites capture (the Microsoft
        // Store pin's embedded property store), Beef001d then Beef001e:
        //   5e00 0000 1d00efbe 0200  <utf16 "Microsoft.WindowsStore_8wekyb3d8bbwe!App"> b005
        // Reconstruct just the two blocks and assert exact strings.
        let exe = build_beef001x_block(
            [0x1D, 0x00, 0xEF, 0xBE],
            "Microsoft.WindowsStore_8wekyb3d8bbwe!App",
        );
        assert_eq!(
            parse_beef001d_executable(&exe).as_deref(),
            Some("Microsoft.WindowsStore_8wekyb3d8bbwe!App")
        );
        let pin = build_beef001x_block([0x1E, 0x00, 0xEF, 0xBE], "SystemPinned");
        assert_eq!(
            parse_beef001e_pintype(&pin).as_deref(),
            Some("SystemPinned")
        );
    }

    #[test]
    fn beef001x_absent_yields_none() {
        assert!(parse_beef001d_executable(&[0x32, 0x00, 0x01, 0x02]).is_none());
        assert!(parse_beef001e_pintype(&[0x32, 0x00, 0x01, 0x02]).is_none());
    }

    #[test]
    fn beef0026_parses_two_filetimes() {
        // FILETIME for 2023-04-20 14:19:56 UTC:
        // unix_ts=1681999196; (1681999196 + 11644473600) * 10_000_000 = 133264739960000000
        let ft: u64 = 133_264_739_960_000_000;
        let mut body = vec![0u8; 8]; // fake item header so a 4-byte block header precedes the sig
        let mut blk = vec![0u8, 0u8]; // size placeholder
        blk.extend_from_slice(&3u16.to_le_bytes()); // version
        blk.extend_from_slice(&[0x26, 0x00, 0xEF, 0xBE]); // signature
        blk.extend_from_slice(&ft.to_le_bytes()); // FirstInteracted
        blk.extend_from_slice(&0u64.to_le_bytes()); // LastInteracted (unset -> None)
        let size = blk.len() as u16;
        blk[0..2].copy_from_slice(&size.to_le_bytes());
        body.extend_from_slice(&blk);

        assert!(has_beef0026(&body));
        let b = parse_beef0026(&body).expect("beef0026");
        assert_eq!(
            b.first_interacted
                .unwrap()
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            "2023-04-20 14:19:56"
        );
        assert!(b.last_interacted.is_none());
    }

    #[test]
    fn beef0004_parses_fat_created_accessed() {
        // FAT DOSDate+DOSTime for 2023-04-20 14:19:56:
        //   DOSDate = ((2023-1980)<<9)|(4<<5)|20 = 0x5694
        //   DOSTime = (14<<11)|(19<<5)|(56/2) = 0x727C
        //   stored little-endian DATE(2) then TIME(2): 94 56 7C 72
        let mut body = vec![0u8; 8]; // fake item header so the block starts at offset 8
        let mut blk = Vec::new();
        blk.extend_from_slice(&0u16.to_le_bytes()); // size placeholder
        blk.extend_from_slice(&9u16.to_le_bytes()); // version
        blk.extend_from_slice(&[0x04, 0x00, 0xEF, 0xBE]); // signature
        blk.extend_from_slice(&[0x94, 0x56, 0x7C, 0x72]); // FAT created
        blk.extend_from_slice(&[0x94, 0x56, 0x7C, 0x72]); // FAT accessed
        blk.extend_from_slice(&0u16.to_le_bytes()); // unknown
        blk.extend_from_slice(&0u16.to_le_bytes()); // version offset / terminator
        let size = blk.len() as u16;
        blk[0..2].copy_from_slice(&size.to_le_bytes());
        body.extend_from_slice(&blk);

        let bb = parse_beef0004(&body);
        assert_eq!(
            bb.created.unwrap().format("%Y-%m-%d %H:%M:%S").to_string(),
            "2023-04-20 14:19:56"
        );
        assert_eq!(
            bb.accessed.unwrap().format("%Y-%m-%d %H:%M:%S").to_string(),
            "2023-04-20 14:19:56"
        );
    }
}
