//! StringData blocks: Name, RelativePath, WorkingDir, Arguments, IconLocation.
//!
//! Each present block (per the header's DataFlags) is a `u16` character count
//! followed by the string itself: UTF-16LE when the link `IsUnicode`, otherwise
//! codepage-decoded bytes. The blocks always appear in this fixed order.
//! Ported from LECmd's `LnkFile.cs`. Bounds-checked; never panics.

use crate::{read_u16, ParseError};

/// DataFlag bits selecting which StringData blocks are present, in read order.
const HAS_NAME: u32 = 0x0000_0004;
const HAS_RELATIVE_PATH: u32 = 0x0000_0008;
const HAS_WORKING_DIR: u32 = 0x0000_0010;
const HAS_ARGUMENTS: u32 = 0x0000_0020;
const HAS_ICON_LOCATION: u32 = 0x0000_0040;

/// The five optional StringData strings carried by a shell link.
#[derive(Debug, Clone, Default)]
pub struct StringData {
    pub name: Option<String>,
    pub relative_path: Option<String>,
    pub working_directory: Option<String>,
    pub arguments: Option<String>,
    pub icon_location: Option<String>,
}

/// Resolve a Windows codepage number to an `encoding_rs` encoding, defaulting
/// to Windows-1252 for unrecognized codepages.
// MUST stay in sync with triage-shellitems/src/items.rs::encoding_for — both
// map legacy code pages identically so an LNK's strings and its shell-item
// short names decode consistently. Update both sites together.
pub(crate) fn encoding_for(codepage: u16) -> &'static encoding_rs::Encoding {
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

/// Decode `bytes` using the given Windows codepage.
fn decode_codepage(bytes: &[u8], codepage: u16) -> String {
    encoding_for(codepage).decode(bytes).0.into_owned()
}

/// Read one StringData block at `data[off..]`: a `u16` character count followed
/// by the string. Returns the decoded string and the new offset past it.
fn read_block(
    data: &[u8],
    off: usize,
    is_unicode: bool,
    codepage: u16,
) -> Result<(String, usize), ParseError> {
    let char_count = read_u16(data, off)? as usize;
    let mut cur = off + 2;
    let s = if is_unicode {
        let byte_len = char_count
            .checked_mul(2)
            .ok_or(ParseError::Corrupt("stringdata len"))?;
        let end = cur
            .checked_add(byte_len)
            .ok_or(ParseError::Corrupt("stringdata range"))?;
        let bytes = data
            .get(cur..end)
            .ok_or(ParseError::Corrupt("stringdata truncated"))?;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        cur = end;
        String::from_utf16_lossy(&units)
    } else {
        let end = cur
            .checked_add(char_count)
            .ok_or(ParseError::Corrupt("stringdata range"))?;
        let bytes = data
            .get(cur..end)
            .ok_or(ParseError::Corrupt("stringdata truncated"))?;
        cur = end;
        decode_codepage(bytes, codepage)
    };
    Ok((s, cur))
}

/// Parse all present StringData blocks beginning at `data[off..]`, honoring the
/// link's `flags` (which blocks exist) and `is_unicode` (string encoding).
/// Returns the parsed [`StringData`] and the total number of bytes consumed.
pub fn parse(
    data: &[u8],
    off: usize,
    flags: u32,
    is_unicode: bool,
    codepage: u16,
) -> Result<(StringData, usize), ParseError> {
    let mut sd = StringData::default();
    let mut cur = off;

    for (bit, slot) in [
        (HAS_NAME, 0u8),
        (HAS_RELATIVE_PATH, 1),
        (HAS_WORKING_DIR, 2),
        (HAS_ARGUMENTS, 3),
        (HAS_ICON_LOCATION, 4),
    ] {
        if flags & bit != bit {
            continue;
        }
        let (s, next) = read_block(data, cur, is_unicode, codepage)?;
        cur = next;
        match slot {
            0 => sd.name = Some(s),
            1 => sd.relative_path = Some(s),
            2 => sd.working_directory = Some(s),
            3 => sd.arguments = Some(s),
            _ => sd.icon_location = Some(s),
        }
    }

    Ok((sd, cur - off))
}
