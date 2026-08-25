//! ExtraData block chain parsing.
//!
//! After StringData, a Shell Link carries a chain of ExtraData blocks. Each
//! block is `u32 size` + `u32 signature` followed by `size - 8` payload bytes.
//! The chain ends with a terminal block whose size field is `< 4` (commonly a
//! lone `u32 0x00000000`). This module enumerates the blocks present (matching
//! LECmd's `ExtraBlocksPresent` naming) and fully decodes the
//! `TrackerDataBlock` (signature `0xA0000003`), from which the originating
//! machine's NetBIOS name, MAC address, and the link's creation time are
//! recovered via the file-droid version-1 UUID.

use crate::read_u32;

/// Number of 100-nanosecond intervals between the Gregorian calendar epoch
/// (1582-10-15, the base for version-1 UUID timestamps) and the Windows
/// FILETIME epoch (1601-01-01). Subtracting this from a UUID-v1 timestamp
/// yields a FILETIME, which renders identically to LECmd's `CreationTime`.
const GREGORIAN_TO_FILETIME_100NS: u64 = 5_748_192_000_000_000;

/// Decoded `TrackerDataBlock` (signature `0xA0000003`). Mirrors LECmd's
/// `TrackerDataBaseBlock`: a machine identifier plus two droid GUID pairs.
/// The MAC address and creation time are derived from the file droid, a
/// version-1 UUID whose trailing 6 bytes are the node (MAC) and whose 60-bit
/// time field is the creation timestamp.
#[derive(Debug, Clone, Default)]
pub struct TrackerData {
    /// NetBIOS machine name, 16-byte ASCII trimmed at the first NUL. Stored
    /// as-is (LECmd does not change case).
    pub machine_id: Option<String>,
    /// MAC of the originating machine, lowercase `xx:xx:xx:xx:xx:xx`, matching
    /// LECmd's `MacAddress` (derived from `Guid.ToString()`, which is lowercase).
    pub mac_address: Option<String>,
    /// Link creation time as a Windows FILETIME (100ns since 1601-01-01).
    /// `0` means none/undecodable. Render via `WinTimestamp::from_filetime`.
    pub created_on: u64,
    /// Volume droid GUID (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, lowercase).
    pub volume_droid: Option<String>,
    /// File droid GUID — the version-1 UUID carrying MAC + creation time.
    pub file_droid: Option<String>,
    /// Volume droid birth GUID.
    pub volume_droid_birth: Option<String>,
    /// File droid birth GUID.
    pub file_droid_birth: Option<String>,
}

/// Parsed ExtraData section: the ordered list of block type names present and
/// the decoded tracker block (if any).
#[derive(Debug, Clone, Default)]
pub struct ExtraData {
    /// Names of present blocks, in chain order. Spellings match LECmd's
    /// `ExtraBlocksPresent` (the C# `GetType().Name` of each block class).
    pub blocks_present: Vec<String>,
    /// The `TrackerDataBlock`, decoded, if the chain contained one.
    pub tracker: Option<TrackerData>,
}

/// Map an ExtraData block signature to its LECmd block-type name. Returns
/// `None` for unknown signatures (which terminate enumeration of that block
/// as "unknown" without panicking). Names mirror the C# class `GetType().Name`
/// used to build `ExtraBlocksPresent`.
fn block_name(sig: u32) -> Option<&'static str> {
    Some(match sig {
        0xA000_0001 => "EnvironmentVariableDataBlock",
        0xA000_0002 => "ConsoleDataBlock",
        0xA000_0003 => "TrackerDataBaseBlock",
        0xA000_0004 => "ConsoleFeDataBlock",
        0xA000_0005 => "SpecialFolderDataBlock",
        0xA000_0006 => "DarwinDataBlock",
        0xA000_0007 => "IconEnvironmentDataBlock",
        0xA000_0008 => "ShimDataBlock",
        0xA000_0009 => "PropertyStoreDataBlock",
        0xA000_000B => "KnownFolderDataBlock",
        0xA000_000C => "VistaAndAboveIdListDataBlock",
        _ => return None,
    })
}

/// Parse the ExtraData block chain starting at `data[off..]`. Never panics;
/// every read is bounds-checked and a malformed block simply ends enumeration.
pub fn parse(data: &[u8], off: usize) -> ExtraData {
    let mut out = ExtraData::default();
    let mut pos = off;

    // Terminal block: size < 4 (or no room for the size field) ends the chain.
    while let Ok(size) = read_u32(data, pos) {
        let size = size as usize;
        if size < 8 {
            // < 4 is the documented terminal marker; < 8 cannot hold the
            // size+signature header, so there is no usable block either way.
            break;
        }
        // The whole block must fit within the buffer.
        let Some(block_end) = pos.checked_add(size) else {
            break;
        };
        if block_end > data.len() {
            break;
        }
        let block = &data[pos..block_end];

        // Signature follows the size field.
        let Ok(sig) = read_u32(block, 4) else { break };

        match block_name(sig) {
            Some(name) => {
                out.blocks_present.push(name.to_string());
                if sig == 0xA000_0003 {
                    out.tracker = parse_tracker(block);
                }
            }
            None => {
                // Unknown signature: record it generically and keep walking the
                // chain by the size field (LECmd would tag this a DamagedDataBlock).
                out.blocks_present
                    .push(format!("UnknownDataBlock_0x{sig:08X}"));
            }
        }

        pos = block_end;
    }

    out
}

/// Decode a `TrackerDataBlock` payload. Layout (offsets into the full block,
/// which begins with `u32 size` + `u32 signature`):
///   0x00 u32 size
///   0x04 u32 signature
///   0x08 u32 version
///   0x0C u32 length
///   0x10 16-byte ASCII MachineID (NUL-trimmed)
///   0x20 Droid:      VolumeDroid GUID, FileDroid GUID
///   0x40 DroidBirth: VolumeDroidBirth GUID, FileDroidBirth GUID
/// Returns `None` only if the block is too short to hold the fixed fields.
fn parse_tracker(block: &[u8]) -> Option<TrackerData> {
    // Need through the four GUIDs: 0x20 + 4*16 = 0x60 bytes.
    if block.len() < 0x60 {
        return None;
    }

    // MachineID is decoded from `GetString(...).Split('\0').First()`. An all-NUL
    // field decodes to an empty string, which LECmd then nukes to a JSON-null
    // (omitted) while the CSV cell stays empty. Mirror that by mapping empty to
    // `None`, so JSON omits the property and CSV renders an empty cell — both
    // matching the fixture (a tracker can carry a MAC yet no machine name).
    let machine_id = {
        let raw = &block[0x10..0x20];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let s: String = raw[..end].iter().map(|&b| b as char).collect();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    };

    let vol: [u8; 16] = block[0x20..0x30].try_into().ok()?;
    let file: [u8; 16] = block[0x30..0x40].try_into().ok()?;
    let vol_birth: [u8; 16] = block[0x40..0x50].try_into().ok()?;
    let file_birth: [u8; 16] = block[0x50..0x60].try_into().ok()?;

    Some(TrackerData {
        machine_id,
        mac_address: Some(mac_from_droid(&file)),
        created_on: filetime_from_droid(&file),
        volume_droid: Some(guid_to_string(&vol)),
        file_droid: Some(guid_to_string(&file)),
        volume_droid_birth: Some(guid_to_string(&vol_birth)),
        file_droid_birth: Some(guid_to_string(&file_birth)),
    })
}

/// MAC address from a version-1 UUID's node field: the final 6 bytes rendered
/// lowercase as `xx:xx:xx:xx:xx:xx`. Matches LECmd, which takes the last group
/// of `Guid.ToString()` (lowercase) and inserts `:` every two hex digits.
fn mac_from_droid(g: &[u8; 16]) -> String {
    let n = &g[10..16];
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        n[0], n[1], n[2], n[3], n[4], n[5]
    )
}

/// Windows FILETIME (100ns since 1601-01-01) from a version-1 UUID's 60-bit
/// time field. The first 8 bytes (little-endian) carry the timestamp; the
/// high nibble of byte 7 is the UUID version and is masked off, exactly as
/// LECmd does before reading the int64. Returns `0` if the result would be
/// negative (i.e. a time before the FILETIME epoch — not a real artifact).
fn filetime_from_droid(g: &[u8; 16]) -> u64 {
    let mut ts = [0u8; 8];
    ts.copy_from_slice(&g[0..8]);
    ts[7] &= 0x0F; // strip the version nibble
    let gregorian_100ns = u64::from_le_bytes(ts);
    gregorian_100ns.saturating_sub(GREGORIAN_TO_FILETIME_100NS)
}

/// Render a 16-byte GUID as a lowercase `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`
/// string, matching C# `Guid.ToString("D")`: the first three fields are
/// little-endian, the trailing eight bytes are in stored order.
fn guid_to_string(g: &[u8; 16]) -> String {
    let d1 = u32::from_le_bytes([g[0], g[1], g[2], g[3]]);
    let d2 = u16::from_le_bytes([g[4], g[5]]);
    let d3 = u16::from_le_bytes([g[6], g[7]]);
    format!(
        "{d1:08x}-{d2:04x}-{d3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic TrackerDataBlock with a known MachineID and a
    /// version-1 file droid, then assert the decoded fields. The droid's MAC
    /// and timestamp are derived from a real-ish v1 UUID so the UUID-v1 ->
    /// FILETIME math is exercised end to end.
    #[test]
    fn decodes_synthetic_tracker_block() {
        // A known v1 UUID. Time fields and node chosen so we can predict the
        // decode. UUID (string form): the bytes below in file order.
        //   time_low  (LE u32) = 0x11223344
        //   time_mid  (LE u16) = 0x5566
        //   time_hi+v (LE u16) = 0x1788  (version nibble = 1)
        //   clock_seq          = 0x99 0xAA
        //   node (MAC)         = 00 0C 29 3E 1F 2C
        let mut droid = [0u8; 16];
        droid[0..4].copy_from_slice(&0x1122_3344u32.to_le_bytes());
        droid[4..6].copy_from_slice(&0x5566u16.to_le_bytes());
        droid[6..8].copy_from_slice(&0x1788u16.to_le_bytes());
        droid[8] = 0x99;
        droid[9] = 0xAA;
        droid[10..16].copy_from_slice(&[0x00, 0x0C, 0x29, 0x3E, 0x1F, 0x2C]);

        // Compute the expected FILETIME from the v1 time field.
        // time field (60-bit) little-endian = bytes 0..8 with byte7 high nibble
        // masked. byte7 = 0x17 -> masked 0x07.
        let mut ts = [0u8; 8];
        ts.copy_from_slice(&droid[0..8]);
        ts[7] &= 0x0F;
        let gregorian = u64::from_le_bytes(ts);
        let expected_ft = gregorian - GREGORIAN_TO_FILETIME_100NS;
        assert!(
            expected_ft > 0,
            "chosen UUID must postdate the FILETIME epoch"
        );

        // Assemble the full block.
        let mut block = vec![0u8; 0x60];
        // size and signature
        block[0..4].copy_from_slice(&(0x60u32).to_le_bytes());
        block[4..8].copy_from_slice(&0xA000_0003u32.to_le_bytes());
        // version + length (informational)
        block[8..12].copy_from_slice(&0u32.to_le_bytes());
        block[12..16].copy_from_slice(&0x58u32.to_le_bytes());
        // MachineID, NUL-padded
        let mid = b"my-machine-01";
        block[0x10..0x10 + mid.len()].copy_from_slice(mid);
        // VolumeDroid (arbitrary), FileDroid (the v1 UUID), births (arbitrary)
        block[0x30..0x40].copy_from_slice(&droid);

        let t = parse_tracker(&block).expect("tracker decodes");
        assert_eq!(t.machine_id.as_deref(), Some("my-machine-01"));
        assert_eq!(t.mac_address.as_deref(), Some("00:0c:29:3e:1f:2c"));
        assert_eq!(t.created_on, expected_ft);
        assert_eq!(
            t.file_droid.as_deref(),
            Some("11223344-5566-1788-99aa-000c293e1f2c")
        );
    }

    /// Walking a chain: a tracker block followed by a terminal zero u32 yields
    /// exactly one block name and a populated tracker.
    #[test]
    fn walks_chain_with_terminal() {
        let mut block = vec![0u8; 0x60];
        block[0..4].copy_from_slice(&(0x60u32).to_le_bytes());
        block[4..8].copy_from_slice(&0xA000_0003u32.to_le_bytes());
        // 4-byte terminal block (size 0)
        block.extend_from_slice(&0u32.to_le_bytes());

        let ed = parse(&block, 0);
        assert_eq!(ed.blocks_present, vec!["TrackerDataBaseBlock".to_string()]);
        assert!(ed.tracker.is_some());
    }

    /// Truncated / empty input must not panic and yields nothing.
    #[test]
    fn empty_and_truncated_are_safe() {
        assert!(parse(&[], 0).blocks_present.is_empty());
        assert!(parse(&[0x01, 0x02], 0).blocks_present.is_empty());
        // size says 0x60 but buffer is short -> no block
        let mut b = vec![0u8; 16];
        b[0..4].copy_from_slice(&0x60u32.to_le_bytes());
        b[4..8].copy_from_slice(&0xA000_0003u32.to_le_bytes());
        assert!(parse(&b, 0).blocks_present.is_empty());
    }

    /// A block whose size field is 0xFFFFFFFF must not panic (overruns buffer).
    #[test]
    fn giant_size_no_panic() {
        let mut b = vec![0u8; 8];
        b[0..4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        b[4..8].copy_from_slice(&0xA000_0003u32.to_le_bytes());
        // block_end = 0xFFFFFFFF > data.len()==8 -> break immediately
        let ed = parse(&b, 0);
        assert!(ed.blocks_present.is_empty());
    }

    /// A TrackerDataBlock whose payload is truncated mid-droid must not panic;
    /// the block name is recorded but the tracker field is None.
    #[test]
    fn truncated_mid_droid_no_panic() {
        // Block size = 0x40 (too short for full tracker payload which needs 0x60)
        let mut block = vec![0u8; 0x40];
        block[0..4].copy_from_slice(&(0x40u32).to_le_bytes());
        block[4..8].copy_from_slice(&0xA000_0003u32.to_le_bytes());
        let ed = parse(&block, 0);
        assert_eq!(ed.blocks_present, vec!["TrackerDataBaseBlock".to_string()]);
        assert!(ed.tracker.is_none(), "truncated tracker should yield None");
    }
}
