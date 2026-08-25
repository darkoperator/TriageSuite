//! Custom Destinations (`*.customDestinations-ms`) parser.
//!
//! Unlike Automatic Destinations, this is NOT a compound (CFB) file. It is a
//! flat binary: a sequence of *category* chunks, each terminated by the footer
//! signature `0xBABFFBAB` (little-endian bytes `AB FB BF BA`). A category chunk
//! begins with a small fixed header (an optional UTF-16 display name) followed
//! by one or more embedded Shell Link (LNK) structures.
//!
//! Framing is signature-driven, mirroring Eric Zimmerman's `JumpList` library
//! (`Custom/CustomDestination.cs`, `Custom/Entry.cs`):
//!
//! 1. Locate every footer signature; the bytes between consecutive footers
//!    (inclusive of the 4-byte footer) form one category chunk.
//! 2. Within a chunk, locate every embedded LNK by its 20-byte header signature
//!    (`4C 00 00 00` + the LNK class id). Each LNK runs from its own offset to
//!    the next LNK offset, or to the footer position for the final LNK. No LNK
//!    self-length is needed — the next signature / footer bounds it.
//!
//! Leniency: a truncated or garbage tail yields whatever entries parsed so far;
//! only a missing footer or an empty/too-small file errors. Never panics.

use crate::ParseError;

/// The footer signature `0xBABFFBAB` as little-endian bytes.
const FOOTER: [u8; 4] = [0xAB, 0xFB, 0xBF, 0xBA];

/// The 20-byte signature that opens every embedded LNK: the 4-byte header size
/// `4C 00 00 00` followed by the 16-byte LNK class id.
const LNK_HEADER: [u8; 20] = [
    0x4C, 0x00, 0x00, 0x00, 0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x46,
];

/// A single embedded Shell Link inside a Custom Destinations file, tagged with
/// the display name of the category (`entry_name`) it was found in. One
/// category may contribute several entries (one per embedded LNK); each is
/// flattened to its own [`CustomEntry`], matching JLECmd's per-LNK CSV rows
/// where `EntryName = entry.Name`.
pub struct CustomEntry {
    /// The category display name (empty when the category header type is not 0).
    pub entry_name: String,
    /// The parsed embedded Shell Link.
    pub lnk: triage_lnk::LnkFile,
}

/// A parsed Custom Destinations file: the flattened list of embedded LNKs.
pub struct CustomDestination {
    pub entries: Vec<CustomEntry>,
}

/// Parse a `customDestinations-ms` file: walk the footer-delimited category
/// chunks, extract each embedded LNK via [`triage_lnk::parse_with_codepage`],
/// and tag it with its category name.
///
/// Errors only when the file is empty, too small (<= 24 bytes), or missing the
/// terminal footer signature. A malformed tail stops cleanly, returning the
/// entries parsed so far (JLECmd leniency). Bounds-checked throughout; never
/// panics.
pub fn parse(data: &[u8], codepage: u16) -> Result<CustomDestination, ParseError> {
    // An empty or footer-less stub carries no extractable categories. The
    // on-disk format routinely produces 24-byte "header + footer, zero
    // categories" files (a valid-but-empty jump list) and occasional tiny
    // header-only fragments. Neither is a parse failure of our extraction:
    // return an empty result rather than erroring, matching JLECmd leniency
    // (it would simply emit no rows for these). Only a file large enough to
    // plausibly hold categories yet *missing* the terminal footer is treated
    // as malformed.
    if data.len() <= 24 {
        return Ok(CustomDestination { entries: vec![] });
    }
    // The file must end with the footer signature.
    if data.get(data.len() - 4..) != Some(&FOOTER[..]) {
        return Err(ParseError::BadFormat("invalid signature (footer missing)"));
    }

    // 1. Find every footer offset.
    let mut footer_offsets = Vec::new();
    let mut index = 0usize;
    while index < data.len() {
        match byte_search(data, &FOOTER, index) {
            Some(lo) => {
                footer_offsets.push(lo);
                index = lo + FOOTER.len();
            }
            None => break,
        }
    }

    // 2. Split into chunks: [chunk_start, footer_offset + 4). Track each chunk's
    //    absolute offset (unused for parsing, but mirrors the reference).
    let mut entries = Vec::new();
    let mut chunk_start = 0usize;
    for &footer_offset in &footer_offsets {
        // chunk_size = footer_offset - chunk_start + 4 (include the footer)
        let chunk_end = footer_offset.saturating_add(4).min(data.len());
        if chunk_end <= chunk_start {
            break;
        }
        let chunk = &data[chunk_start..chunk_end];
        // The reference skips chunks <= 30 bytes (no real category payload).
        if chunk.len() > 30 {
            parse_category(chunk, codepage, &mut entries);
        }
        chunk_start = chunk_end;
    }

    Ok(CustomDestination { entries })
}

/// Parse one footer-delimited category chunk, appending one [`CustomEntry`] per
/// embedded LNK. A malformed LNK is skipped; parsing continues with the rest.
fn parse_category(chunk: &[u8], codepage: u16, out: &mut Vec<CustomEntry>) {
    // Category header: Unknown0 (u32@0), Rank (f32@4), Unknown2 (u32@8),
    // HeaderType (u32@12). When HeaderType == 0 a UTF-16 display name follows:
    // nameLen (i16@16) characters at offset 18.
    let header_type = read_u32(chunk, 12).unwrap_or(u32::MAX);
    let mut name = String::new();
    if header_type == 0 {
        if let Some(name_len) = read_u16(chunk, 16) {
            let char_count = name_len as usize;
            let byte_len = char_count.saturating_mul(2);
            let start = 18usize;
            if let Some(name_bytes) = chunk.get(start..start.saturating_add(byte_len)) {
                name = utf16le_until_nul(name_bytes);
            }
        }
    }

    // Locate every embedded LNK header signature within the chunk.
    let mut lnk_offsets = Vec::new();
    let mut index = 0usize;
    while index < chunk.len() {
        match byte_search(chunk, &LNK_HEADER, index) {
            Some(lo) => {
                lnk_offsets.push(lo);
                index = lo + 1; // advance by 1 so we don't re-hit the same offset
            }
            None => break,
        }
    }

    // The footer position bounds the final LNK.
    let footer_pos = byte_search(chunk, &FOOTER, 0).unwrap_or(chunk.len());

    let last = lnk_offsets.len().saturating_sub(1);
    for (i, &start) in lnk_offsets.iter().enumerate() {
        let end = if i == last {
            footer_pos
        } else {
            lnk_offsets[i + 1]
        };
        if end <= start {
            continue;
        }
        let Some(lnk_bytes) = chunk.get(start..end) else {
            continue;
        };
        if let Ok(lnk) = triage_lnk::parse_with_codepage(lnk_bytes, codepage) {
            out.push(CustomEntry {
                entry_name: name.clone(),
                lnk,
            });
        }
    }
}

/// Decode a UTF-16LE byte slice up to the first NUL terminator, lossily.
fn utf16le_until_nul(bytes: &[u8]) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let u = u16::from_le_bytes([pair[0], pair[1]]);
        if u == 0 {
            break;
        }
        units.push(u);
    }
    String::from_utf16_lossy(&units)
}

/// Find the first offset >= `start` where `needle` occurs in `haystack`.
fn byte_search(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() || start > haystack.len() - needle.len() {
        return None;
    }
    (start..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

/// Read a little-endian u32 at `off`, bounds-checked.
fn read_u32(d: &[u8], off: usize) -> Option<u32> {
    d.get(off..off.checked_add(4)?)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// Read a little-endian u16 at `off`, bounds-checked.
fn read_u16(d: &[u8], off: usize) -> Option<u16> {
    d.get(off..off.checked_add(2)?)
        .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
}
