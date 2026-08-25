//! Port of RegistryPlugin.Taskband (Taskband.cs + ValuesOut.cs).
//!
//! Decodes the taskbar-pin list from the NTUSER `Taskband\Favorites` REG_BINARY
//! value. Fires on the key:
//!   `Software\Microsoft\Windows\CurrentVersion\Explorer\Taskband`
//! and reads its `Favorites` value.
//!
//! Favorites framing (C# Taskband.ProcessValues):
//!   - 1 leading unknown byte, then a sequence of CHUNKS. Each chunk is an
//!     int32-size-prefixed blob followed by a 5-byte marker. The size field is
//!     part of the chunk bytes.
//!   - Within each chunk: a leading int32 (the chunk size again), then a run of
//!     2-byte-size-prefixed SHELL ITEMS until a zero size.
//!
//! For each shell item we derive three strings, matching the C# ValuesOut:
//!   LnkName    = ShellBag.Value
//!   Executable = Beef001d.Executable    (else "(unknown)")
//!   PinType    = Beef001e.PinType        (else "(unknown)")
//! with the special case: when the bag's FriendlyName is "Directory" (class
//! 0x31), Executable and PinType are forced to "(Directory)".
//!
//! ShellBag.Value rendering is Taskband-specific (NOT identical to LECmd):
//!   - 0x1f (root/GUID folder): friendly name from the 16-byte GUID at item
//!     offset 4 (e.g. "User Pinned", "Applications").
//!   - 0x31 (Directory) / 0x32 (File): the Beef0004 LongName ONLY, when
//!     non-empty. Taskband never falls back to the LocalisedName or short name,
//!     so a file whose Beef0004 carries two strings (LongName empty) renders an
//!     EMPTY Value — verified against the fixture (the Explorer pin's LnkName
//!     is "").
//!   - 0x00 (property-view): the value of integer property ID 10 from the
//!     embedded serialized PropertyStore (e.g. "Microsoft Store").
//!
//! Detail-CSV column order (fixture-authoritative):
//!   LnkName, BatchKeyPath, Executable, BatchValueName, PinType
//!
//! Batch row format (C# ValuesOut.cs):
//!   ValueData1 = "Lnk Name: {LnkName}"
//!   ValueData2 = "Executable: {Executable}"
//!   ValueData3 = "Pin Type: {PinType}"

use notatin::cell_key_node::CellKeyNode;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct Taskband;

/// One decoded pinned taskbar item.
struct PinnedItem {
    lnk_name: String,
    executable: String,
    pin_type: String,
}

/// Split the Favorites blob into chunks per the C# framing (1 lead byte, then
/// int32-size-prefixed chunks each trailed by a 5-byte marker).
fn split_chunks(data: &[u8]) -> Vec<&[u8]> {
    let mut chunks = Vec::new();
    if data.is_empty() {
        return chunks;
    }
    let mut pos = 1usize; // skip the leading unknown byte
    while pos + 4 <= data.len() {
        let size = i32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        if size <= 0 {
            break;
        }
        let size = size as usize;
        let end = match pos.checked_add(size) {
            Some(e) if e <= data.len() => e,
            _ => break,
        };
        chunks.push(&data[pos..end]);
        // Advance past the chunk and its 5-byte marker.
        pos = end + 5;
    }
    chunks
}

/// Extract the 2-byte-size-prefixed shell items from one chunk. The returned
/// slices INCLUDE the 2-byte size prefix (matching the C# `rawBytes`, where the
/// class byte sits at index 2). A zero/negative size terminates.
///
/// The final item's declared size often runs a few bytes PAST the chunk
/// boundary (the chunk size field under-counts the trailing item). The C# reads
/// it with `BinaryReader.ReadBytes(siSize)`, which returns the truncated tail
/// rather than failing; we mirror that by clamping the item end to the chunk
/// length. The truncated bytes are only the item's trailing VersionOffset, well
/// past every extension block we parse, so the values are unaffected.
fn chunk_items(chunk: &[u8]) -> Vec<&[u8]> {
    let mut items = Vec::new();
    let mut cp = 4usize; // skip the leading int32 chunk size
    while cp + 2 <= chunk.len() {
        let size = i16::from_le_bytes([chunk[cp], chunk[cp + 1]]);
        if size <= 0 {
            break;
        }
        let size = size as usize;
        // Clamp to the chunk end (the last item may declare a size that spills
        // a few bytes past the chunk boundary; ReadBytes would truncate it).
        let end = cp.saturating_add(size).min(chunk.len());
        items.push(&chunk[cp..end]);
        if end == chunk.len() {
            break;
        }
        cp = end;
    }
    items
}

/// Render the Taskband `ShellBag.Value` (LnkName) for one shell item whose
/// bytes still carry the 2-byte size prefix. Also reports whether the item is a
/// directory (class 0x31), which forces Executable/PinType to "(Directory)".
fn render_lnk_name(item: &[u8]) -> (String, bool) {
    // Class byte is at index 2 (the 2-byte size prefix precedes the body).
    let class = item.get(2).copied().unwrap_or(0);
    // Body = item without the 2-byte size prefix; offsets below are body-relative.
    let body = item.get(2..).unwrap_or(&[]);
    match class {
        0x1f => {
            // Root/GUID folder: 16-byte GUID at body offset 2 (C# rawBytes[4]).
            let name = body
                .get(2..18)
                .map(triage_shellitems::guids::name_from_guid_bytes)
                .unwrap_or_default();
            (name, false)
        }
        0x31 => {
            // Directory: Beef0004 LongName only.
            let b4 = triage_shellitems::extension::parse_beef0004(body);
            (b4.long_name.unwrap_or_default(), true)
        }
        0x32 => {
            // File: Beef0004 LongName only (no LocalisedName/short-name fallback).
            let b4 = triage_shellitems::extension::parse_beef0004(body);
            (b4.long_name.unwrap_or_default(), false)
        }
        0x00 => {
            // Property-view item: value of integer property ID 10.
            (property_view_value(body).unwrap_or_default(), false)
        }
        _ => (String::new(), false),
    }
}

/// Decode the `0x00` property-view shell item's Value: the C# selects the
/// PropertyStore property whose integer key is 10 (e.g. "Microsoft Store").
///
/// `body` is the item without the 2-byte size prefix. Per C#
/// `ProcessPropertyViewDefault`, which indexes into `rawBytes` (the item WITH
/// its 2-byte size prefix): C# `index = 10` → body offset 8. There: int16
/// property-sheet-list size, int16 identifier size, then the identifier bytes,
/// then the serialized PropertyStore of `psls` bytes.
fn property_view_value(body: &[u8]) -> Option<String> {
    // C# rawBytes[10] == body[8] (the 2-byte size prefix is stripped from body).
    let psls = read_i16(body, 8)?;
    if psls <= 0 {
        return None;
    }
    let id_size = read_i16(body, 10)? as usize;
    let prop_start = 12 + id_size;
    let end = prop_start + psls as usize;
    // The trailing item is sometimes truncated by a couple of bytes; clamp so we
    // still parse the property store (PID 10 sits well before the end).
    let end = end.min(body.len());
    let prop_bytes = body.get(prop_start..end)?;
    property_store_pid10(prop_bytes)
}

/// Walk a serialized PropertyStore (a sequence of `1SPS` integer-named property
/// storages) and return the string value of property ID 10. Mirrors the
/// MS-PROPSTORE layout: each storage is `u32 size, u32 version(0x53505331),
/// 16-byte FMTID`, then a list of values `u32 valueSize, u32 id, u8 reserved,
/// TypedPropertyValue`. We only need VT_LPWSTR (0x1F) values here.
fn property_store_pid10(prop_bytes: &[u8]) -> Option<String> {
    let mut off = 0usize;
    while off + 4 <= prop_bytes.len() {
        let stor_size = read_u32(prop_bytes, off)? as usize;
        if stor_size == 0 {
            break;
        }
        let stor_end = off
            .checked_add(stor_size)
            .filter(|&e| e <= prop_bytes.len())?;
        let storage = &prop_bytes[off..stor_end];
        // Values begin after u32 size + u32 version + 16-byte FMTID = 24.
        let mut p = 24usize;
        while p + 13 <= storage.len() {
            let vsize = read_u32(storage, p)? as usize;
            if vsize == 0 {
                break;
            }
            let pid = read_u32(storage, p + 4)?;
            let vtype = read_u16(storage, p + 9)?;
            if pid == 10 && vtype == 0x001F {
                // VT_LPWSTR: u32 char count, then UTF-16LE chars.
                let val = storage.get(p + 13..)?;
                let count = read_u32(val, 0)? as usize;
                let chars = val.get(4..4 + count * 2)?;
                let units: Vec<u16> = chars
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let s = String::from_utf16_lossy(&units);
                let s = s.trim_matches('\0');
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
            let next = p.checked_add(vsize)?;
            if next <= p {
                break;
            }
            p = next;
        }
        off = stor_end;
    }
    None
}

fn read_u16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn read_i16(d: &[u8], o: usize) -> Option<i16> {
    d.get(o..o + 2).map(|b| i16::from_le_bytes([b[0], b[1]]))
}

fn read_u32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Decode all pinned items from the Favorites blob.
fn decode_favorites(data: &[u8]) -> Vec<PinnedItem> {
    let mut out = Vec::new();
    for chunk in split_chunks(data) {
        for item in chunk_items(chunk) {
            // Beef001d/001e are scanned over the whole item (they can be embedded
            // inside a property store for 0x00 items, or follow a 0x31/0x32 body).
            let body = item.get(2..).unwrap_or(&[]);
            let (lnk_name, is_directory) = render_lnk_name(item);

            let (executable, pin_type) = if is_directory {
                ("(Directory)".to_string(), "(Directory)".to_string())
            } else {
                let exe = triage_shellitems::extension::parse_beef001d_executable(body)
                    .unwrap_or_else(|| "(unknown)".to_string());
                let pt = triage_shellitems::extension::parse_beef001e_pintype(body)
                    .unwrap_or_else(|| "(unknown)".to_string());
                (exe, pt)
            };

            out.push(PinnedItem {
                lnk_name,
                executable,
                pin_type,
            });
        }
    }
    out
}

impl RegistryPlugin for Taskband {
    fn plugin_name(&self) -> &'static str {
        "Taskband"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Software\Microsoft\Windows\CurrentVersion\Explorer\Taskband"]
    }

    fn value_name(&self) -> Option<&'static str> {
        Some("Favorites")
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        values: &[PluginValue],
        _hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let key_path = key.path.trim_start_matches('\\').to_string();

        let Some(fav) = values.iter().find(|v| v.name == "Favorites") else {
            return Vec::new();
        };

        decode_favorites(&fav.raw)
            .into_iter()
            .map(|p| PluginRow {
                batch_value_name: "Favorites".to_string(),
                batch_value_data1: format!("Lnk Name: {}", p.lnk_name),
                batch_value_data2: format!("Executable: {}", p.executable),
                batch_value_data3: format!("Pin Type: {}", p.pin_type),
                detail_columns: vec![
                    ("LnkName".to_string(), p.lnk_name),
                    ("BatchKeyPath".to_string(), key_path.clone()),
                    ("Executable".to_string(), p.executable),
                    ("BatchValueName".to_string(), "Favorites".to_string()),
                    ("PinType".to_string(), p.pin_type),
                ],
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_and_key_paths() {
        let p = Taskband;
        assert_eq!(p.plugin_name(), "Taskband");
        assert_eq!(p.value_name(), Some("Favorites"));
        assert!(p
            .key_paths()
            .contains(&r"Software\Microsoft\Windows\CurrentVersion\Explorer\Taskband"));
    }

    /// Wrap a shell-item body in a 2-byte size prefix (size includes the prefix),
    /// as it appears inside a Favorites chunk.
    fn size_prefixed(body: &[u8]) -> Vec<u8> {
        let size = (body.len() + 2) as u16;
        let mut v = size.to_le_bytes().to_vec();
        v.extend_from_slice(body);
        v
    }

    /// Build a synthetic Beef001d/001e block (size, version, sig, unk, utf16, voff).
    fn beef001x(sig: [u8; 4], s: &str) -> Vec<u8> {
        let mut blk = vec![0u8, 0]; // size placeholder
        blk.extend_from_slice(&0u16.to_le_bytes()); // version
        blk.extend_from_slice(&sig); // signature
        blk.extend_from_slice(&[0, 0]); // unknown
        for u in s.encode_utf16() {
            blk.extend_from_slice(&u.to_le_bytes());
        }
        blk.extend_from_slice(&[0, 0]); // NUL
        blk.extend_from_slice(&0u16.to_le_bytes()); // VersionOffset
        let size = blk.len() as u16;
        blk[0..2].copy_from_slice(&size.to_le_bytes());
        blk
    }

    #[test]
    fn directory_item_forces_directory_strings() {
        // 0x31 directory with a Beef0004 carrying LongName "TaskBar".
        // Body: class 0x31 at offset 0, then a beef0004 block. We reuse the
        // shellitems beef0004 layout via a minimal version-9 block.
        let mut body = vec![0x31u8, 0x00];
        body.extend_from_slice(&[0u8; 10]); // pad to beef start
        let mut blk = vec![0u8, 0]; // size
        blk.extend_from_slice(&9u16.to_le_bytes()); // version 9
        blk.extend_from_slice(&[0x04, 0x00, 0xEF, 0xBE]); // beef0004 sig
        blk.extend_from_slice(&[0u8; 4]); // created
        blk.extend_from_slice(&[0u8; 4]); // accessed
        blk.extend_from_slice(&[0u8; 2]); // 0x10 unk
        blk.extend_from_slice(&[0u8; 2]); // 0x12 unk
        blk.extend_from_slice(&[0u8; 8]); // file ref
        while blk.len() < 0x2E {
            blk.push(0);
        }
        for u in "TaskBar".encode_utf16() {
            blk.extend_from_slice(&u.to_le_bytes());
        }
        blk.extend_from_slice(&[0, 0, 0, 0]); // NUL + voff
        let size = blk.len() as u16;
        blk[0..2].copy_from_slice(&size.to_le_bytes());
        body.extend_from_slice(&blk);

        let item = size_prefixed(&body);
        let (lnk, is_dir) = render_lnk_name(&item);
        assert_eq!(lnk, "TaskBar");
        assert!(is_dir);
    }

    #[test]
    fn file_item_with_beef001d_and_001e() {
        // 0x32 file with a Beef0004 single-string LongName, a Beef001d, and a
        // Beef001e — the Edge-pin shape minus the property store.
        let mut body = vec![0x32u8, 0x00];
        body.extend_from_slice(&[0u8; 10]);
        // Beef0004 version-9 block with LongName "Microsoft Edge.lnk".
        let mut b4 = vec![0u8, 0];
        b4.extend_from_slice(&9u16.to_le_bytes());
        b4.extend_from_slice(&[0x04, 0x00, 0xEF, 0xBE]);
        b4.extend_from_slice(&[0u8; 4]);
        b4.extend_from_slice(&[0u8; 4]);
        b4.extend_from_slice(&[0u8; 2]);
        b4.extend_from_slice(&[0u8; 2]);
        b4.extend_from_slice(&[0u8; 8]);
        while b4.len() < 0x2E {
            b4.push(0);
        }
        for u in "Microsoft Edge.lnk".encode_utf16() {
            b4.extend_from_slice(&u.to_le_bytes());
        }
        b4.extend_from_slice(&[0, 0, 0, 0]);
        let sz = b4.len() as u16;
        b4[0..2].copy_from_slice(&sz.to_le_bytes());
        body.extend_from_slice(&b4);
        body.extend_from_slice(&beef001x([0x1D, 0x00, 0xEF, 0xBE], "MSEdge"));
        body.extend_from_slice(&beef001x([0x1E, 0x00, 0xEF, 0xBE], "SystemPinned"));

        let item = size_prefixed(&body);
        let (lnk, is_dir) = render_lnk_name(&item);
        assert_eq!(lnk, "Microsoft Edge.lnk");
        assert!(!is_dir);
        let b = item.get(2..).unwrap();
        assert_eq!(
            triage_shellitems::extension::parse_beef001d_executable(b).as_deref(),
            Some("MSEdge")
        );
        assert_eq!(
            triage_shellitems::extension::parse_beef001e_pintype(b).as_deref(),
            Some("SystemPinned")
        );
    }

    #[test]
    fn file_item_two_string_beef0004_renders_empty() {
        // Beef0004 carrying TWO strings -> LongName empty -> Taskband Value "".
        let mut body = vec![0x32u8, 0x00];
        body.extend_from_slice(&[0u8; 10]);
        let mut b4 = vec![0u8, 0];
        b4.extend_from_slice(&9u16.to_le_bytes());
        b4.extend_from_slice(&[0x04, 0x00, 0xEF, 0xBE]);
        b4.extend_from_slice(&[0u8; 4]);
        b4.extend_from_slice(&[0u8; 4]);
        b4.extend_from_slice(&[0u8; 2]);
        b4.extend_from_slice(&[0u8; 2]);
        b4.extend_from_slice(&[0u8; 8]);
        while b4.len() < 0x2E {
            b4.push(0);
        }
        for u in "File Explorer.lnk".encode_utf16() {
            b4.extend_from_slice(&u.to_le_bytes());
        }
        b4.extend_from_slice(&[0, 0]); // NUL between the two strings
        for u in "@shell32.dll,-22067".encode_utf16() {
            b4.extend_from_slice(&u.to_le_bytes());
        }
        b4.extend_from_slice(&[0, 0, 0, 0]);
        let sz = b4.len() as u16;
        b4[0..2].copy_from_slice(&sz.to_le_bytes());
        body.extend_from_slice(&b4);

        let item = size_prefixed(&body);
        let (lnk, _) = render_lnk_name(&item);
        assert_eq!(lnk, "", "two-string beef0004 -> empty Taskband Value");
    }

    #[test]
    fn root_folder_renders_user_pinned_guid() {
        // 0x1f with the "User Pinned" GUID (raw mixed-endian shell-item form).
        let mut body = vec![0x1fu8, 0x00];
        body.extend_from_slice(&[
            0xc8, 0x27, 0x34, 0x1f, 0x10, 0x5c, 0x10, 0x42, 0xaa, 0x03, 0x2e, 0xe4, 0x52, 0x87,
            0xd6, 0x68,
        ]);
        let item = size_prefixed(&body);
        let (lnk, is_dir) = render_lnk_name(&item);
        assert_eq!(lnk, "User Pinned");
        assert!(!is_dir);
    }

    #[test]
    fn property_view_value_reads_pid10() {
        // Build a single `1SPS` integer-named storage carrying PID 10 = VT_LPWSTR
        // "Microsoft Store", wrap it in the 0x00 property-view item framing, and
        // assert property_view_value recovers it (offsets are C# rawBytes-relative,
        // i.e. body offset 8 for the property-sheet-list size).
        fn lpwstr_value(pid: u32, s: &str) -> Vec<u8> {
            let mut chars: Vec<u8> = Vec::new();
            for u in s.encode_utf16() {
                chars.extend_from_slice(&u.to_le_bytes());
            }
            chars.extend_from_slice(&[0, 0]); // NUL
            let char_count = (chars.len() / 2) as u32;
            // TypedPropertyValue: u16 type(0x1F) + u16 pad + u32 count + chars.
            let mut tpv = Vec::new();
            tpv.extend_from_slice(&0x001Fu16.to_le_bytes());
            tpv.extend_from_slice(&0u16.to_le_bytes());
            tpv.extend_from_slice(&char_count.to_le_bytes());
            tpv.extend_from_slice(&chars);
            // Value: u32 valueSize + u32 id + u8 reserved + TypedPropertyValue.
            let mut val = Vec::new();
            let vsize = (4 + 4 + 1 + tpv.len()) as u32;
            val.extend_from_slice(&vsize.to_le_bytes());
            val.extend_from_slice(&pid.to_le_bytes());
            val.push(0); // reserved
            val.extend_from_slice(&tpv);
            val
        }

        let val = lpwstr_value(10, "Microsoft Store");
        let mut storage = Vec::new();
        // size placeholder (u32) + version + 16-byte FMTID + values + 4-byte 0 end.
        storage.extend_from_slice(&[0, 0, 0, 0]);
        storage.extend_from_slice(&0x53505331u32.to_le_bytes());
        storage.extend_from_slice(&[0u8; 16]); // FMTID
        storage.extend_from_slice(&val);
        storage.extend_from_slice(&[0, 0, 0, 0]); // value-list terminator
        let ssize = storage.len() as u32;
        storage[0..4].copy_from_slice(&ssize.to_le_bytes());

        // PropertyStore = storage(s) + 4-byte 0 terminator.
        let mut prop = storage.clone();
        prop.extend_from_slice(&[0, 0, 0, 0]);

        // Item body: C# ProcessPropertyViewDefault reads rawBytes[10] (= body[8])
        // as the property-sheet-list size, body[10] as identifier size, then the
        // identifier bytes, then the property store. Body offsets 0..8 are header
        // (class 0x00 + unknown + dataSize + dataSig).
        let mut body = vec![0x00u8; 8];
        body.extend_from_slice(&(prop.len() as i16).to_le_bytes()); // body[8] psls
        body.extend_from_slice(&0i16.to_le_bytes()); // body[10] identifier size = 0
        body.extend_from_slice(&prop);

        assert_eq!(
            property_view_value(&body).as_deref(),
            Some("Microsoft Store")
        );
    }

    #[test]
    fn empty_favorites_yields_no_items() {
        assert!(decode_favorites(&[]).is_empty());
        assert!(decode_favorites(&[0x00]).is_empty());
    }
}
