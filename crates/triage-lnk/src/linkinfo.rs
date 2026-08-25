//! LinkInfo structure: volume (local) and network share information.
//!
//! Ported from LECmd's `LnkFile.cs` LinkInfo block plus `VolumeInfo.cs` and
//! `NetworkShareInfo.cs`. All reads are bounds-checked; the caller treats a
//! parse error here as a non-fatal, partial section (see `lib.rs`).

use crate::{read_u32, ParseError};

/// LinkInfoFlags bit0: VolumeID and LocalBasePath are present.
const VOLUME_ID_AND_LOCAL_BASE_PATH: u32 = 0x0000_0001;
/// LinkInfoFlags bit1: CommonNetworkRelativeLink and CommonPathSuffix present.
const COMMON_NETWORK_RELATIVE_LINK_AND_PATH_SUFFIX: u32 = 0x0000_0002;

/// Parsed LinkInfo: drive/volume details, the local base path, and the
/// network share path when the link points at a UNC location.
#[derive(Debug, Clone, Default)]
pub struct LinkInfo {
    /// Human-readable drive type description (`VolumeInfo.DriveTypes`).
    pub drive_type: Option<String>,
    /// Raw drive-type enum value, exposed so callers (e.g. LETriage's
    /// `--removable-only`) can test for `DriveRemovable` (2) directly.
    pub drive_type_raw: Option<u32>,
    /// Volume serial number as uppercase 8-digit hex (matches LECmd's `X8`).
    pub volume_serial_number: Option<String>,
    pub volume_label: Option<String>,
    pub local_path: Option<String>,
    pub network_path: Option<String>,
    pub common_path: Option<String>,
}

/// Map a `VolumeInfo.DriveTypes` enum value to LECmd's exact `[Description]`
/// string. Unknown values fall through to the numeric debug form C# would
/// render for an undescribed enum value.
fn drive_type_description(value: u32) -> String {
    match value {
        0 => "Unknown".to_string(),
        1 => "No root directory".to_string(),
        2 => "Removable storage media (Floppy, USB)".to_string(),
        3 => "Fixed storage media (Hard drive)".to_string(),
        4 => "Remote storage".to_string(),
        5 => "Optical disc (CD-ROM, DVD, BD)".to_string(),
        6 => "RAM drive".to_string(),
        other => other.to_string(),
    }
}

/// Decode `bytes` using the given Windows codepage, truncating at the first
/// NUL (C# does `.Split('\0').First()`).
fn decode_codepage_cstr(bytes: &[u8], codepage: u16) -> String {
    let enc = crate::stringdata::encoding_for(codepage);
    let s = enc.decode(bytes).0.into_owned();
    s.split('\0').next().unwrap_or("").to_string()
}

/// Decode a NUL-terminated UTF-16LE string starting at `off` within `data`.
fn decode_utf16_cstr(data: &[u8], off: usize) -> String {
    let mut units = Vec::new();
    let mut i = off;
    while i + 1 < data.len() {
        let u = u16::from_le_bytes([data[i], data[i + 1]]);
        if u == 0 {
            break;
        }
        units.push(u);
        i += 2;
    }
    String::from_utf16_lossy(&units)
}

/// Parse a VolumeID structure (located at `vol[..]`).
fn parse_volume(vol: &[u8], codepage: u16, info: &mut LinkInfo) -> Result<(), ParseError> {
    let drive_type = read_u32(vol, 4)?;
    let serial = read_u32(vol, 8)?;
    let label_offset = read_u32(vol, 12)? as usize;

    info.drive_type_raw = Some(drive_type);
    info.drive_type = Some(drive_type_description(drive_type));
    // LECmd formats the serial with "X8": uppercase, zero-padded to 8 digits.
    info.volume_serial_number = Some(format!("{serial:08X}"));

    // VolumeLabelOffset > 16 indicates an extended (Unicode) label; otherwise
    // the label is codepage-encoded at the offset (typically 0x10).
    let label = if label_offset > 16 {
        // Unicode label at the offset.
        decode_utf16_cstr(vol, label_offset)
    } else {
        let slice = vol.get(label_offset..).unwrap_or(&[]);
        decode_codepage_cstr(slice, codepage)
    };
    if !label.is_empty() {
        info.volume_label = Some(label);
    }
    Ok(())
}

/// Parse the LinkInfo at `data[off..]`. Returns the parsed [`LinkInfo`] and the
/// total LinkInfo byte length so the caller can advance past it. Never panics;
/// all indexing is bounds-checked.
pub fn parse(data: &[u8], off: usize, codepage: u16) -> Result<(LinkInfo, usize), ParseError> {
    let total_size = read_u32(data, off)? as usize;
    if total_size == 0 {
        return Err(ParseError::Corrupt("linkinfo size"));
    }
    let end = off
        .checked_add(total_size)
        .ok_or(ParseError::Corrupt("linkinfo range"))?;
    let block = data
        .get(off..end)
        .ok_or(ParseError::Corrupt("linkinfo truncated"))?;

    let mut info = LinkInfo::default();

    // The LinkInfo body requires at least the fixed header (20 bytes) plus the
    // CommonPathSuffix offset at 24; LECmd guards `locationBytes.Length > 20`.
    if block.len() <= 20 {
        return Ok((info, total_size));
    }

    let header_size = read_u32(block, 4)? as usize;
    let flags = read_u32(block, 8)?;
    let vol_offset = read_u32(block, 12)? as usize;
    let local_path_offset = read_u32(block, 16)? as usize;
    let network_offset = read_u32(block, 20)? as usize;

    // VolumeID + LocalBasePath
    if vol_offset > 0 {
        if let Some(vol_slice) = block.get(vol_offset..) {
            let vsize = read_u32(block, vol_offset)? as usize;
            if vsize > 0 {
                if let Some(vol) = vol_slice.get(..vsize) {
                    let _ = parse_volume(vol, codepage, &mut info);
                }
            }
        }
    }

    if flags & VOLUME_ID_AND_LOCAL_BASE_PATH == VOLUME_ID_AND_LOCAL_BASE_PATH {
        let slice = block.get(local_path_offset..).unwrap_or(&[]);
        let lp = decode_codepage_cstr(slice, codepage);
        info.local_path = if lp.is_empty() { None } else { Some(lp) };
    }

    // CommonNetworkRelativeLink
    if flags & COMMON_NETWORK_RELATIVE_LINK_AND_PATH_SUFFIX
        == COMMON_NETWORK_RELATIVE_LINK_AND_PATH_SUFFIX
    {
        if let Some(net_slice) = block.get(network_offset..) {
            let nsize = read_u32(block, network_offset)? as usize;
            if nsize > 0 {
                if let Some(net) = net_slice.get(..nsize) {
                    let _ = parse_network(net, codepage, &mut info);
                }
            }
        }
    }

    // CommonPathSuffix (codepage) — requires the offset field at 24.
    if block.len() >= 28 {
        let common_offset = read_u32(block, 24)? as usize;
        let slice = block.get(common_offset..).unwrap_or(&[]);
        let cp = decode_codepage_cstr(slice, codepage);
        info.common_path = if cp.is_empty() { None } else { Some(cp) };
    }

    // Unicode LocalBasePath / CommonPathSuffix override when the optional
    // offsets are present (header size > 28 / > 32, i.e. >= 0x24 carries both).
    if header_size > 28 {
        if let Ok(uni_off) = read_u32(block, 28) {
            let lp = decode_utf16_cstr(block, uni_off as usize);
            info.local_path = if lp.is_empty() { None } else { Some(lp) };
        }
    }
    if header_size > 32 {
        if let Ok(uni_off) = read_u32(block, 32) {
            let cp = decode_utf16_cstr(block, uni_off as usize);
            info.common_path = if cp.is_empty() { None } else { Some(cp) };
        }
    }

    Ok((info, total_size))
}

/// Parse a CommonNetworkRelativeLink structure (located at `net[..]`).
fn parse_network(net: &[u8], codepage: u16, info: &mut LinkInfo) -> Result<(), ParseError> {
    let net_name_offset = read_u32(net, 8)? as usize;

    if net_name_offset > 20 {
        // Unicode variants present: offsets at 20 (name) and 24 (device).
        let uni_name_offset = read_u32(net, 20)? as usize;
        let np = decode_utf16_cstr(net, uni_name_offset);
        info.network_path = if np.is_empty() { None } else { Some(np) };
    } else {
        let slice = net.get(net_name_offset..).unwrap_or(&[]);
        let np = decode_codepage_cstr(slice, codepage);
        info.network_path = if np.is_empty() { None } else { Some(np) };
    }
    Ok(())
}
