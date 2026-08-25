//! Shell item body parsing and value rendering.
//!
//! `parse_item_cp` dispatches on the class type (the first body byte) and renders
//! the display `value` LECmd joins into `TargetIDAbsolutePath`. The common
//! types are implemented faithfully; exotic types degrade to a non-empty
//! `Unknown-0x..` placeholder (refined in the Task 9 compat burn-down).
//!
//! Note on offsets: the C# reference parsers operate on `rawBytes` that still
//! include the 2-byte size prefix, so their `index = 2` is our body offset 0.
//! Every offset ported here is shifted down by 2 accordingly.

use crate::{extension, guids, ShellItem};

/// Parse one shell item body (without the 2-byte size prefix), decoding legacy
/// (non-Unicode) strings such as short names with `codepage`. UTF-16 long names
/// are codepage-independent.
pub fn parse_item_cp(body: &[u8], codepage: u16) -> ShellItem {
    let class_type = body.first().copied().unwrap_or(0);
    // A beef0004 block trails file/dir entries (0x31/0x32 and Unicode variants)
    // and the 0x74 delegate "Users Files Folder" item. LECmd reads the LAST
    // target ID's last Beef0004 for the MFT reference. We restrict beef parsing
    // to these structural carriers: property-view items (0x00/0x71) embed
    // PropertyStores whose bytes can contain spurious `04 00 EF BE` patterns, so
    // a raw signature scan there would surface a bogus reference.
    let extension = if matches!(class_type, 0x31 | 0x32 | 0x35 | 0x36 | 0xb1 | 0x74)
        && extension::has_beef0004(body)
    {
        Some(extension::parse_beef0004(body))
    } else {
        None
    };
    // File/dir entries (class 0x31/0x32/0x35/0x36/0xb1) carry the FAT
    // last-modified timestamp. The C# reference (`ShellBag0x31.cs`) operates on
    // `rawBytes` which includes the 2-byte size prefix; our `body` starts 2 bytes
    // later, so all offsets shift down by 2:
    //   rawBytes[8..10]  = DOSDate  →  body[6..8]
    //   rawBytes[10..12] = DOSTime  →  body[8..10]
    // `fat_datetime` expects 4 bytes ordered as [date_lo, date_hi, time_lo, time_hi],
    // so we read body[6..10].  The previous code read body[8..12] which landed on
    // the Attributes field + first byte of ShortName and produced bogus far-future
    // dates (year 2036–2064).
    let modified = if matches!(class_type, 0x31 | 0x32 | 0x35 | 0x36 | 0xb1) {
        body.get(0x06..0x0A).and_then(extension::fat_datetime)
    } else {
        None
    };
    let value = render_value(class_type, body, extension.as_ref(), codepage);
    ShellItem {
        class_type,
        value,
        extension,
        raw: body.to_vec(),
        modified,
    }
}

/// Render the display value for a shell item (LECmd ShellBag.Value semantics).
///
/// Offset note: the C# `rawBytes` includes the 2-byte size prefix that our
/// `body` omits, so every C# `rawBytes[N]` maps to our `body[N-2]`.
fn render_value(
    class_type: u8,
    body: &[u8],
    ext: Option<&extension::Beef0004>,
    codepage: u16,
) -> String {
    match class_type {
        0x00 => variable_value(body), // variable (FTP/HTTP URI, network host, etc.)
        0x01 => control_panel_category_value(body), // control panel category
        0x1f => root_folder_value(body), // root/GUID folder (e.g. "This PC")
        0x23 => drive_letter_value(body, codepage), // drive letter
        0x2e => control_panel_value(body), // control panel / property view
        0x2f => volume_value(body, codepage), // volume/drive (e.g. "C:\")
        0x31 | 0x32 | 0x35 | 0x36 | 0xb1 => file_entry_value(body, ext, codepage), // dir/file
        0x4c => sharepoint_value(body), // sharepoint directory
        0x61 => uri_value(body),      // URI (e.g. ms-gamingoverlay://)
        0x71 => control_panel_guid_value(body), // control-panel GUID folder
        0x74 => users_files_folder_value(body), // Users Files Folder (delegate)
        // 0xC0..=0xFF: high-bit network variants (UNC share/server entries, class 0x43|0x80 etc.)
        0xc0..=0xff => network_share_value(body), // network share / UNC path
        _ => format!("Unknown-0x{class_type:02x}"),
    }
}

/// 0x61 URI item (`ShellBag0X61`). When the inline data size (C# `rawBytes[4]`
/// = body offset 2) is zero, the value is a UTF-16LE string at C# offset 8
/// (body offset 6), taken up to the first NUL. The populated-data-size variant
/// (FTP with timestamps) is rarer; its value also sits in a decoded string
/// region — handled by the same fallback when the structured read is absent.
fn uri_value(body: &[u8]) -> String {
    // dataSize at body offset 2 (C# rawBytes[4]).
    let data_size = body
        .get(2..4)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .unwrap_or(0);
    if data_size == 0 {
        // Value = Encoding.Unicode.GetString(rawBytes, 8, ...).Split('\0').First()
        if body.len() > 6 {
            let region = &body[6..];
            let end = utf16_nul(region);
            let units: Vec<u16> = region[..end]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let s = String::from_utf16_lossy(&units);
            let s = s.trim_matches('\0');
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    "Unknown-0x61".to_string()
}

/// 0x01 control-panel category (`ShellBag0X01`). When the special data sig (C#
/// `ToUInt32(rawBytes, 4)` = body offset 2) is `0x39de2184`, the category is a
/// fixed table keyed by C# `rawBytes[8]` (body offset 6). Otherwise the value
/// is a UTF-16LE string at C# offset 14 (body offset 12).
fn control_panel_category_value(body: &[u8]) -> String {
    let special = body
        .get(2..6)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if special == Some(0x39de2184) {
        let key = body.get(6).copied().unwrap_or(0xff);
        let name = match key {
            0x00 => "All Control Panel Items",
            0x01 => "Appearance and Personalization",
            0x02 => "Hardware and Sound",
            0x03 => "Network and Internet",
            0x04 => "Sound, Speech and Audio Devices",
            0x05 => "System and Security",
            0x06 => "Clock, Language, and Region",
            0x07 => "Ease of Access",
            0x08 => "Programs",
            0x09 => "User Accounts",
            0x10 => "Security Center",
            0x11 => "Mobile PC",
            _ => return format!("Unknown category! Category ID: {key}"),
        };
        return name.to_string();
    }
    // Non-category 0x01: UTF-16LE string at body offset 12 (C# offset 14).
    if body.len() > 12 {
        let region = &body[12..];
        let units: Vec<u16> = region
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        let s = String::from_utf16_lossy(&units);
        let s = s.replace('\0', "");
        if !s.is_empty() {
            return s;
        }
    }
    "Unknown-0x01".to_string()
}

/// 0x71 control-panel GUID folder (`ShellBag0X71`, general case). The 16-byte
/// GUID sits at C# `index` after `2 + 2 + 10` skips (body offset 12), resolving
/// to a friendly folder name (e.g. `System`). The `beebee00` property-view
/// variant is not represented in the corpus; it degrades to a placeholder.
fn control_panel_guid_value(body: &[u8]) -> String {
    // beebee00 property-view sig at C# ToUInt32(rawBytes,6) = body offset 4.
    let data_sig = body
        .get(4..8)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if data_sig != Some(0xbeebee00) {
        // GUID at body offset 12 (C# index 2+2+10 = 14).
        if body.len() >= 28 {
            let name = guids::name_from_guid_bytes(&body[12..28]);
            if !name.is_empty() {
                return name;
            }
        }
    }
    "Unknown-0x71".to_string()
}

/// 0x74 Users Files Folder (`ShellBag0X74`). Its rendered value is the
/// beef0004 LongName of the embedded delegate item (e.g. `AppData`). We locate
/// and parse the trailing beef0004 block directly and use its long name.
fn users_files_folder_value(body: &[u8]) -> String {
    let b4 = extension::parse_beef0004(body);
    if let Some(name) = b4.long_name.as_deref() {
        let name = name.trim_matches('\0');
        if !name.is_empty() {
            return name.to_string();
        }
    }
    "Unknown-0x74".to_string()
}

/// 0x23 drive letter (`ShellBag0X23`): a 2-char codepage string at C# offset 3
/// (body offset 1), e.g. `C:`.
fn drive_letter_value(body: &[u8], codepage: u16) -> String {
    if body.len() >= 3 {
        let s = decode_codepage(&body[1..3], codepage);
        let s = s.trim_matches('\0');
        if !s.is_empty() {
            return s.to_string();
        }
    }
    "Unknown-0x23".to_string()
}

/// 0x4c Sharepoint directory (`ShellBag0X4C`): `"{name} ({name2})"` from two
/// length-prefixed UTF-16LE strings starting at C# offset 0x1c (body 0x1a).
fn sharepoint_value(body: &[u8]) -> String {
    let read_lp = |off: usize| -> Option<(String, usize)> {
        let len = body
            .get(off..off + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)?;
        let start = off + 2;
        let nbytes = len * 2;
        let region = body.get(start..start + nbytes)?;
        let units: Vec<u16> = region
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let s = String::from_utf16_lossy(&units);
        Some((s.trim_matches('\0').to_string(), start + nbytes))
    };
    if let Some((name, next)) = read_lp(0x1a) {
        // +2 terminator, then second length-prefixed string.
        if let Some((name2, _)) = read_lp(next + 2) {
            let name2 = if name2.is_empty() {
                "URL not specified".to_string()
            } else {
                name2
            };
            return format!("{name} ({name2})");
        }
    }
    "Unknown-0x4c".to_string()
}

/// 0x00 variable item (`ShellBag0X00`). Handles four variants:
///
/// - `0xC001B000` (body[2..6]) — HTTP-URI container: URL-unescaped UTF-16LE
///   string at body[0x16] preceded by a size (u32 at body[0x12]).
/// - `0xAFBB00B5` (body[2..6]) — Network computer/workgroup container: the
///   display name (hostname) is embedded in a serialized PropertyStore blob
///   starting at body[16]. We scan the blob for the `{B725F130...}` GUID
///   (System.ItemNameDisplay) and return the following UTF-16LE string value.
///   Fallback: scan all UTF-16LE strings ≥ 3 chars and return the first.
/// - `0xC001B001` or similar (FTP/MTP) — scan body for a UTF-16LE name.
/// - Unknown sub-type — returns "Unknown-0x00".
fn variable_value(body: &[u8]) -> String {
    let special = body
        .get(2..6)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    match special {
        Some(0xc001b000) => {
            // HTTP-URI container.
            if let Some(sz) = body
                .get(0x12..0x16)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            {
                let start = 0x16;
                if let Some(region) = body.get(start..start + sz) {
                    let units: Vec<u16> = region
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    let s = String::from_utf16_lossy(&units).replace('\0', "");
                    let s = url_unescape(&s);
                    if !s.is_empty() {
                        return s;
                    }
                }
            }
        }
        Some(0xafbb00b5) => {
            // Network computer / workgroup container (ShellBag0X00 "cid.x0xAFBB00B5").
            // The display name lives in a PropertyStore blob at body[16..]. We scan
            // the serialized storage for a UTF-16LE string value that looks like a
            // host or workgroup name.
            //
            // GUID for the relevant property set (System.ItemNameDisplay):
            // {B725F130-47EF-101A-A5F1-02608C9EEBAC}
            // Stored mixed-endian as: 30 F1 25 B7 EF 47 1A 10 A5 F1 02 60 8C 9E EB AC
            const ITEM_NAME_GUID: [u8; 16] = [
                0x30, 0xF1, 0x25, 0xB7, 0xEF, 0x47, 0x1A, 0x10, 0xA5, 0xF1, 0x02, 0x60, 0x8C, 0x9E,
                0xEB, 0xAC,
            ];
            if let Some(s) = extract_psps_string(body, &ITEM_NAME_GUID, 0x0A) {
                return s;
            }
            // Fallback: scan the whole body for a printable UTF-16LE string.
            if let Some(s) = first_utf16_string(body, 16, 3, 256) {
                return s;
            }
        }
        _ => {}
    }
    "Unknown-0x00".to_string()
}

/// 0xC3 network share / UNC item (`ShellBagNetworkShareItem`). The UNC path
/// (e.g. `\\server\share`) is stored as a NUL-terminated ASCII string at
/// body offset 3. Anything before the NUL is the path; "Network location" is
/// the SBECmd ShellType for these items. Returns the UNC string when present.
fn network_share_value(body: &[u8]) -> String {
    // body[0]=0xC3, body[1]=flags, body[2]=extra, body[3..]=NUL-terminated UNC
    const PATH_OFF: usize = 3;
    if body.len() > PATH_OFF {
        let slice = &body[PATH_OFF..];
        let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        let s = String::from_utf8_lossy(&slice[..end]);
        let s = s.trim_matches('\0');
        if !s.is_empty() {
            return s.to_string();
        }
    }
    "Unknown-0xC3".to_string()
}

/// Scan `body` starting at `start` for a UTF-16LE PSPS property block whose
/// property set GUID matches `guid` and whose property ID matches `prop_id`.
/// Reads the property value as a VT_LPWSTR (type 0x001F) UTF-16LE string.
/// Returns `None` when not found or truncated.
fn extract_psps_string(body: &[u8], guid: &[u8; 16], prop_id: u32) -> Option<String> {
    // PropertyStore consists of consecutive DWORD-prefixed SerializedPropertyStorage
    // blocks followed by a 0x00000000 terminator. Each block starts with a GUID
    // (16 bytes) followed by property entries. We walk them linearly, looking for
    // a block whose GUID matches, then scan its entries for the target property ID.
    let mut pos = 16usize; // first block size DWORD
    loop {
        let block_size = body
            .get(pos..pos + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)?;
        if block_size == 0 {
            break; // terminator
        }
        let block_start = pos + 4;
        let block_end = block_start.checked_add(block_size)?.min(body.len());
        // Each block begins with "1SPS" (0x31,0x53,0x50,0x53) then 16-byte GUID.
        if body.get(block_start..block_start + 4) == Some(b"1SPS") {
            let guid_start = block_start + 4;
            if body.get(guid_start..guid_start + 16) == Some(guid) {
                // Found our GUID — scan entries for prop_id.
                let mut epos = guid_start + 16;
                while epos + 4 <= block_end {
                    let entry_size =
                        u32::from_le_bytes(body.get(epos..epos + 4)?.try_into().ok()?) as usize;
                    if entry_size == 0 {
                        break;
                    }
                    let pid = u32::from_le_bytes(body.get(epos + 4..epos + 8)?.try_into().ok()?);
                    // Skip 1-byte reserved. Type at offset 9 (4-byte LE).
                    let vt = u32::from_le_bytes(body.get(epos + 9..epos + 13)?.try_into().ok()?);
                    if pid == prop_id && vt == 0x001F {
                        // VT_LPWSTR: DWORD char count then UTF-16LE string.
                        let count =
                            u32::from_le_bytes(body.get(epos + 13..epos + 17)?.try_into().ok()?)
                                as usize;
                        let str_start = epos + 17;
                        let str_bytes = count * 2;
                        if let Some(region) = body.get(str_start..str_start + str_bytes) {
                            let units: Vec<u16> = region
                                .chunks_exact(2)
                                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                                .take_while(|&u| u != 0)
                                .collect();
                            let s = String::from_utf16_lossy(&units);
                            let s = s.trim_matches('\0');
                            if !s.is_empty() {
                                return Some(s.to_string());
                            }
                        }
                    }
                    if epos + entry_size <= epos {
                        break;
                    }
                    epos += entry_size;
                }
            }
        }
        pos = block_end;
        if pos >= body.len() {
            break;
        }
    }
    None
}

/// Scan `body` from `start` for the first plausible UTF-16LE string of at
/// least `min_len` and at most `max_len` printable BMP characters. Returns
/// `None` when nothing plausible is found. Used as a fallback when the
/// PropertyStore GUID scan fails.
fn first_utf16_string(body: &[u8], start: usize, min_len: usize, max_len: usize) -> Option<String> {
    let n = body.len();
    let mut i = start;
    while i + 4 <= n {
        if !i.is_multiple_of(2) {
            i += 1;
            continue;
        }
        // Look for a sequence of printable UTF-16LE code units followed by NUL.
        let mut len = 0;
        let mut j = i;
        while j + 2 <= n {
            let u = u16::from_le_bytes([body[j], body[j + 1]]);
            if u == 0 {
                break;
            }
            if !(0x0020..=0x007E).contains(&u) {
                break;
            } // restrict to ASCII range for safety
            len += 1;
            j += 2;
        }
        if len >= min_len && len <= max_len {
            let units: Vec<u16> = body[i..j]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            return Some(String::from_utf16_lossy(&units));
        }
        i += 2;
    }
    None
}

/// Minimal `Uri.UnescapeDataString` equivalent: decode `%XX` UTF-8 sequences.
fn url_unescape(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 0x1f root/GUID folder. The 16-byte GUID sits at body offset 2 (C# rawBytes
/// offset 4) and maps to a friendly name. Empty/short bodies degrade.
fn root_folder_value(body: &[u8]) -> String {
    if body.len() >= 18 {
        let name = guids::name_from_guid_bytes(&body[2..18]);
        if !name.is_empty() {
            return name;
        }
    }
    format!("Unknown-0x{:02x}", body.first().copied().unwrap_or(0))
}

/// 0x2e control-panel / users-property-view item. When `body[1] == 0x80` it is
/// a GUID root folder (C# ShellBag0X2E.ProcessGuid: GUID at rawBytes[4] = body
/// offset 2). Other variants carry property sheets we do not yet decode; they
/// degrade to a non-empty placeholder.
fn control_panel_value(body: &[u8]) -> String {
    if body.get(1).copied() == Some(0x80) && body.len() >= 18 {
        let name = guids::name_from_guid_bytes(&body[2..18]);
        if !name.is_empty() {
            return name;
        }
    }
    "Unknown-0x2e".to_string()
}

/// 0x2f volume / drive letter. The drive string ("C:\") starts at body
/// offset 1 (C# rawBytes offset 3) and is codepage-encoded ASCII, NUL padded.
/// SBECmd strips the trailing backslash, rendering "C:" not "C:\".
fn volume_value(body: &[u8], codepage: u16) -> String {
    if body.len() > 1 {
        let raw = &body[1..];
        let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
        let s = decode_codepage(&raw[..end], codepage);
        let s = s.trim_end_matches('\0').trim_end_matches('\\');
        if !s.is_empty() {
            return s.to_string();
        }
    }
    "Unknown-0x2f".to_string()
}

/// 0x31/0x32 (and Unicode 0x35/0x36/0xb1) directory/file entry. Prefers the
/// beef0004 long name; otherwise falls back to the short name at the item's
/// name offset (body offset 12 = C# rawBytes offset 14).
fn file_entry_value(body: &[u8], ext: Option<&extension::Beef0004>, codepage: u16) -> String {
    // SBECmd ShellBag0x31/0x32 `.Value`: LongName when non-empty, else the short
    // name. LocalisedName is a resource reference string like `@shell32.dll,-21817`
    // that can only be resolved on Windows via SHLoadIndirectString; we cannot
    // resolve it here (no DLL access on Linux/macOS), so skip it and use the
    // short name as the fallback instead. This matches SBECmd behaviour which
    // resolves the resource to "Program Files (x86)" etc., whose short-name form
    // is "PROGRA~2" — but the test fixtures show the long display name, suggesting
    // SBECmd resolves the resource. We cannot resolve it, so we fall back to the
    // short name for all cases where LongName is absent.
    if let Some(b4) = ext {
        if let Some(name) = b4.long_name.as_deref() {
            let name = name.trim_matches('\0');
            if !name.is_empty() {
                return name.to_string();
            }
        }
        // LECmd/JLECmd fall back to LocalisedName when LongName is absent (2+
        // strings in the multistring). LocalisedName is a resource reference like
        // `@shell32.dll,-21817` that JLECmd / LECmd emit verbatim in their
        // TargetIDAbsolutePath column — the fixtures confirm this. We cannot
        // resolve it (no DLL access on macOS/Linux), but we reproduce the raw
        // reference string faithfully so the compat output matches.
        if let Some(name) = b4.localised_name.as_deref() {
            let name = name.trim_matches('\0');
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }

    // Short name fallback. Name starts at body offset 12 and runs to NUL.
    const NAME_OFF: usize = 12;
    let class_type = body.first().copied().unwrap_or(0);
    let unicode = matches!(class_type, 0x35 | 0x36);
    if body.len() > NAME_OFF {
        let rest = &body[NAME_OFF..];
        // Stop the short name at the beef0004 signature if it appears before a
        // NUL (defensive; the name is normally NUL-terminated well before it).
        let beef = rest
            .windows(4)
            .position(|w| w == [0x04, 0x00, 0xEF, 0xBE])
            .map(|p| p.saturating_sub(4));
        let limit = beef.unwrap_or(rest.len());
        let region = &rest[..limit.min(rest.len())];

        let name = if unicode {
            let end = utf16_nul(region);
            let units: Vec<u16> = region[..end]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            let end = region.iter().position(|&c| c == 0).unwrap_or(region.len());
            decode_codepage(&region[..end], codepage)
        };
        let name = name.trim_matches('\0');
        if !name.is_empty() {
            return name.to_string();
        }
    }
    format!("Unknown-0x{class_type:02x}")
}

/// Byte length up to (not including) a UTF-16LE NUL pair.
fn utf16_nul(b: &[u8]) -> usize {
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == 0 && b[i + 1] == 0 {
            return i;
        }
        i += 2;
    }
    b.len() & !1
}

/// Resolve a Windows codepage number to an `encoding_rs` encoding, defaulting
/// to Windows-1252 for unrecognized codepages. Mirrors the mapping in
/// `triage-lnk`'s `stringdata::encoding_for` so legacy names decode identically.
// MUST stay in sync with triage-lnk/src/stringdata.rs::encoding_for — both
// map legacy code pages identically so an LNK's strings and its shell-item
// short names decode consistently. Update both sites together.
fn encoding_for(codepage: u16) -> &'static encoding_rs::Encoding {
    match codepage {
        932 => encoding_rs::SHIFT_JIS,
        936 => encoding_rs::GBK,
        949 => encoding_rs::EUC_KR,
        950 => encoding_rs::BIG5,
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1252 => encoding_rs::WINDOWS_1252,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        1257 => encoding_rs::WINDOWS_1257,
        1258 => encoding_rs::WINDOWS_1258,
        _ => encoding_rs::WINDOWS_1252,
    }
}

/// Decode legacy (non-Unicode) bytes using the given Windows `codepage`,
/// defaulting to Windows-1252. Lossless for ASCII; maps high bytes to their
/// codepage glyphs the way LECmd does.
fn decode_codepage(b: &[u8], codepage: u16) -> String {
    let (cow, _, _) = encoding_for(codepage).decode(b);
    cow.into_owned()
}

#[cfg(test)]
mod tests {
    use super::parse_item_cp;

    #[test]
    fn file_entry_parses_modified_dos_time() {
        // ShellBag0x31 body layout (body = rawBytes[2..], i.e. size prefix stripped):
        //   body[0]    = class type (0x31 = directory)
        //   body[1]    = flags
        //   body[2..6] = FileSize (4 bytes)
        //   body[6..8] = DOSDate  (2 bytes LE)
        //   body[8..10]= DOSTime  (2 bytes LE)
        //   body[10..12]= Attributes (2 bytes)
        //   body[12..] = ShortName (NUL-terminated ASCII)
        //
        // DOS 2023-04-20 14:19:56:
        //   DOSDate = ((2023-1980)<<9)|(4<<5)|20 = 0x5694 → LE bytes: 0x94, 0x56
        //   DOSTime = (14<<11)|(19<<5)|(56/2)   = 0x727C → LE bytes: 0x7C, 0x72
        let mut body = vec![
            0x31u8, 0x00, // class, flags
            0, 0, 0, 0, // FileSize
            0x94, 0x56, // DOSDate LE
            0x7C, 0x72, // DOSTime LE
            0, 0, // Attributes
        ];
        body.extend_from_slice(b"x\0"); // minimal ShortName
        let item = parse_item_cp(&body, 1252);
        assert_eq!(
            item.modified
                .unwrap()
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            "2023-04-20 14:19:56"
        );
    }

    #[test]
    fn empty_idlist_is_empty_path() {
        assert_eq!(crate::absolute_path(&crate::parse_id_list(&[0, 0])), "");
    }

    #[test]
    fn unknown_class_type_degrades_not_panics() {
        let data = [0x06, 0x00, 0xAA, 0x01, 0x02, 0x03, 0x00, 0x00];
        let items = crate::parse_id_list(&data);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].class_type, 0xAA);
        assert!(!items[0].value.is_empty()); // never empty for unknown
        assert!(!items[0].raw.is_empty());
    }

    #[test]
    fn truncated_item_size_stops_cleanly() {
        let data = [0x40, 0x00, 0x31, 0x00];
        let _ = crate::parse_id_list(&data); // must not panic
    }

    #[test]
    fn volume_renders_drive_letter() {
        // size(2) + body: class 0x2f, then "C:\" NUL-padded.
        // SBECmd strips the trailing backslash → "C:".
        let mut body = vec![0x2f];
        body.extend_from_slice(b"C:\\\0\0");
        assert_eq!(super::volume_value(&body, 1252), "C:");
    }

    #[test]
    fn root_folder_renders_known_guid() {
        // 0x1f + sort byte + Downloads GUID (raw mixed-endian).
        let mut body = vec![0x1f, 0x00];
        body.extend_from_slice(&[
            0x05, 0x39, 0x8e, 0x08, 0x23, 0x03, 0x02, 0x4b, 0x98, 0x26, 0x5d, 0x99, 0x42, 0x8e,
            0x11, 0x5f,
        ]);
        assert_eq!(super::root_folder_value(&body), "Downloads");
    }

    #[test]
    fn file_entry_prefers_beef_long_name() {
        let ext = crate::extension::Beef0004 {
            long_name: Some("Program Files (x86)".to_string()),
            localised_name: None,
            primary_name: Some("Program Files (x86)".to_string()),
            mft_entry: Some(1),
            mft_sequence: Some(1),
            created: None,
            accessed: None,
        };
        let body = vec![
            0x31, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'P', b'R', b'O', 0,
        ];
        assert_eq!(
            super::file_entry_value(&body, Some(&ext), 1252),
            "Program Files (x86)"
        );
    }

    #[test]
    fn file_entry_short_name_fallback() {
        // No beef block: short ASCII name at offset 12.
        let mut body = vec![0x31, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        body.extend_from_slice(b"PROGRA~2\0");
        assert_eq!(super::file_entry_value(&body, None, 1252), "PROGRA~2");
    }

    /// Build a minimal 0x32 file item: a 2-byte size prefix wrapping a body
    /// whose class byte is 0x32, with the short name placed at body offset 12
    /// (the file-entry name offset), then a 2-byte 0x0000 terminator. No
    /// beef0004 block, so `render_value` falls back to the short name.
    fn build_file_item_with_short_name(name: &[u8]) -> Vec<u8> {
        // body[0] = class 0x32; body[1..12] zero-filled; name at body[12..].
        let mut body = vec![0x32u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        body.extend_from_slice(name);
        let size = (2 + body.len()) as u16;
        let mut item = size.to_le_bytes().to_vec();
        item.extend_from_slice(&body);
        item.extend_from_slice(&[0x00, 0x00]); // IDList terminator
        item
    }

    #[test]
    fn codepage_threads_to_short_name_decode() {
        // A 0x32 file item whose short name contains a byte (0x81) that decodes
        // differently under cp1252 (a control/undefined glyph) vs cp932 (a
        // Shift-JIS lead byte forming a multibyte char with the following byte).
        // No beef0004 block => render_value uses the short name => codepage matters.
        let item = build_file_item_with_short_name(b"A\x81\x40B\x00");
        let v1252 = crate::parse_id_list_with_codepage(&item, 1252);
        let v932 = crate::parse_id_list_with_codepage(&item, 932);
        assert_eq!(v1252.len(), 1);
        assert_eq!(v932.len(), 1);
        assert_ne!(
            v1252[0].value, v932[0].value,
            "short name should decode differently under 1252 vs 932"
        );
    }
}
