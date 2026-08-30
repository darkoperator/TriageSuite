//! Recursive BagMRU walk producing ShellbagRecords (SBECmd ProcessBags model).
//!
//! Each BagMRU key holds: numbered REG_BINARY values "0".."N" (each a single
//! shell-item blob, u16-size-prefixed), a "MRUListEx" value (4-byte LE slot
//! indices = ordering), and a "NodeSlot" value (u32). A numbered SUBKEY "<slot>"
//! continues the tree for that slot. We recurse via `hive.sub_keys` (children
//! enumeration) — never `get_key` mid-walk, which corrupts notatin parser state.
//!
//! ### NodeSlot per-row (fixture-pinned, Task 10)
//! SBECmd's NodeSlot column follows different rules at different depths:
//!
//! **Root level (`BagMRU`):** every item at this level gets the BagMRU key's own
//! NodeSlot value (ABSENT → 0). In practice every observed hive has ABSENT at the
//! root, so all root items show NodeSlot=0. The root virtual-folder items are not
//! assigned individual Bags\ slots.
//!
//! **All deeper levels (`BagMRU\1`, `BagMRU\1\3`, etc.):** each row's NodeSlot is
//! read from the child subkey named `<slot>` under the current BagMRU key.
//! Example: for slot 0 under `BagMRU\1`, the NodeSlot comes from `BagMRU\1\0`'s
//! own NodeSlot value (ABSENT → 0). This gives the per-row variation seen in the
//! fixture (e.g. `BagMRU\1` rows show 2, 4, 5, 7, 16, 19, 21).
//!
//! Rationale: verified by binary inspection of the reference evidence hives
//! and cross-checked against all 10 SBECmd reference fixtures. The root-level
//! special case is confirmed by the fact that root child subkeys (BagMRU\0,
//! BagMRU\1, …) have non-zero NodeSlot values in multiple hives, yet ALL root
//! fixture rows show NodeSlot=0.

use crate::record::ShellbagRecord;
use crate::shelltype::shell_type_name;
use notatin::cell_key_node::CellKeyNode;
use std::collections::{HashMap, HashSet};
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::value::raw_bytes;
use triage_shellitems::items::parse_item_cp;

/// Render an optional chrono UTC datetime to the standalone ISO column form
/// (empty when `None`); the compat harness normalizes SBECmd's form to match.
fn ts(dt: Option<chrono::DateTime<chrono::Utc>>) -> String {
    match dt {
        Some(d) => {
            WinTimestamp::from_unix_nanos(d.timestamp(), d.timestamp_subsec_nanos()).to_string()
        }
        None => String::new(),
    }
}

/// Parse a key's `MRUListEx` into a slot -> position map (position = the slot's
/// index in the MRU list; 0xFFFFFFFF terminates).
fn mru_positions(key: &CellKeyNode) -> HashMap<u32, usize> {
    let mut m = HashMap::new();
    if let Some(v) = key.get_value("MRUListEx") {
        let raw = raw_bytes(&v);
        for (pos, chunk) in raw.chunks_exact(4).enumerate() {
            let slot = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            if slot == 0xFFFF_FFFF {
                break;
            }
            m.insert(slot, pos);
        }
    }
    m
}

/// Count extension blocks in an item body by walking the declared block sizes.
///
/// Shell-item extension blocks form a chain immediately following the item's
/// primary data. Each block has the layout (block-relative offsets):
///
/// | off | field |
/// |-----|-------|
/// | 0   | u16 block size (includes these 2 bytes; < 6 → chain terminator) |
/// | 2   | u16 version |
/// | 4   | u32 signature stored little-endian (`?? ?? EF BE`, so bytes [6..8] == `EF BE`) |
///
/// The `BEEF????` family of signatures always has `EF BE` in the **last two
/// bytes** of the LE u32, i.e. at block offsets 6 and 7. We scan for a
/// position `i` where `body[i+6] == 0xEF && body[i+7] == 0xBE` and the u16
/// at `body[i..i+2]` is a plausible block size (≥ 8 to cover sig + header).
///
/// ### Limitation
///
/// For complex virtual-folder items (class `0x1F`, e.g. CLSID_SearchFolder),
/// the item body may embed an IDList whose items also carry BEEF blocks. The
/// forward scan finds the FIRST chain (inside the IDList) and terminates when
/// the IDList ends, potentially missing additional BEEF blocks that follow. This
/// leads to an undercount for some CLSID_SearchFolder variants. A complete fix
/// requires class-specific item parsing to find the correct extension-block
/// chain start offset.
pub fn extension_block_count(body: &[u8]) -> usize {
    let n = body.len();
    if n < 8 {
        return 0;
    }
    // Find the first extension block: scan for a position where the LE u32
    // signature bytes [6] and [7] == `EF BE`, and the declared block size
    // (u16 at [0..2]) is plausible (≥ 8 bytes: 2 size + 2 ver + 4 sig).
    let chain_start = (0..n.saturating_sub(7)).find(|&i| {
        i + 8 <= n && body[i + 6] == 0xEF && body[i + 7] == 0xBE && {
            let sz = u16::from_le_bytes([body[i], body[i + 1]]) as usize;
            sz >= 8
        }
    });

    let Some(start) = chain_start else {
        return 0;
    };

    let mut count = 0usize;
    let mut pos = start;
    loop {
        if pos + 8 > n {
            break;
        }
        let block_size = u16::from_le_bytes([body[pos], body[pos + 1]]) as usize;
        // Block size < 8 terminates the chain (0 = absent, 2/4/6 = empty sentinel).
        if block_size < 8 {
            break;
        }
        // Verify the `EF BE` bytes in the signature field (block offsets 6–7).
        if body[pos + 6] != 0xEF || body[pos + 7] != 0xBE {
            break;
        }
        count += 1;
        let next = pos + block_size;
        if next <= pos {
            break;
        }
        pos = next;
    }
    count
}

/// Read a key's `NodeSlot` value as a decimal string (empty is used internally
/// as "absent" but the caller normalises to "0" before storing in the record).
fn node_slot_u32(key: &CellKeyNode) -> Option<u32> {
    key.get_value("NodeSlot").and_then(|v| {
        let b = raw_bytes(&v);
        if b.len() >= 4 {
            Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        } else {
            None
        }
    })
}

/// Format an optional NodeSlot: None (absent) → "0"; Some(n) → n.to_string().
fn ns_str(n: Option<u32>) -> String {
    n.map(|v| v.to_string()).unwrap_or_else(|| "0".to_string())
}

/// Join a parent absolute path with one item's display value. Root level (no
/// parent) is always rooted at `Desktop`.
fn join_abs(parent_abs: &str, value: &str) -> String {
    if parent_abs.is_empty() {
        format!("Desktop\\{value}")
    } else {
        format!("{parent_abs}\\{}", value.trim_matches('\\'))
    }
}

/// A BagMRU numbered value's bytes start with a u16 size prefix; `parse_item_cp`
/// expects the body WITHOUT that prefix. Strip it (empty for < 2 bytes).
fn item_body(value_bytes: &[u8]) -> &[u8] {
    value_bytes.get(2..).unwrap_or(&[])
}

/// True when a shell-item class byte belongs to a class for which SBECmd can
/// emit HasExplored=True, OUTSIDE of a zip-archive path context.
///
/// Zip-archive items (all class bytes) are handled by the `in_zip_context` check
/// in `walk_inner`; this function only applies when the item is NOT inside a zip.
///
/// Folder-capable classes (verified against all 10 SBECmd reference fixtures):
///   - 0x1F, 0x2E   → Root folder: GUID (virtual root folders)
///   - 0x23, 0x25, 0x2F → Drive letter (volumes)
///   - 0x30..=0x3F with bit 0x02 CLEAR → Directory only (NOT File)
///   - 0x71          → Users property view / GUID: Control panel
///
/// NOT folder-capable outside zip context:
///   - File (0x30..=0x3F bit 0x02 SET) — a zip archive file counts as a "File"
///   - Variable (0x00) — credential manager / network-computer items
///   - Variable: Users property view (0x74)
///   - Control Panel Category (0x01)
///   - Network location (0x40..=0x4F, 0xC0..=0xFF)
///   - URI (0x61)
///   - Zip file contents class bytes (0x07, 0x52, 0x7E) — outside zip context
///     these class bytes are rare / not meaningful for HasExplored
///   - Unknown class (Type ID: 0xXX)
fn is_folder_capable_class_outside_zip(ct: u8) -> bool {
    match ct {
        0x1F | 0x2E => true,           // Root folder: GUID
        0x23 | 0x25 | 0x2F => true,    // Drive letter
        0x30..=0x3F => ct & 0x02 == 0, // Directory (bit 0x02 clear); NOT File
        0x71 => true,                  // Users property view / GUID: Control panel
        _ => false,
    }
}

/// Walk `key` (BagMRU-relative path `bag_path`, accumulated ancestor path
/// `parent_abs`), appending one ShellbagRecord per numbered slot, then recursing
/// into each numbered subkey. `explored` is the set of NodeSlot numbers that have
/// a `Bags\<n>\Shell` subkey with at least one child GUID key, built once before
/// the walk via `crate::bags::explored_node_slots`.
///
/// `is_root` — true when this is the top-level BagMRU key (bag_path == "BagMRU").
/// At root level, all items share the BagMRU key's own NodeSlot (ABSENT → 0).
/// At all other levels, each slot's NodeSlot is read from the child subkey named
/// `<slot>` (per-row variation; ABSENT → 0).
pub fn walk(
    hive: &mut Hive,
    key: CellKeyNode,
    bag_path: &str,
    parent_abs: &str,
    explored: &HashSet<u32>,
    out: &mut Vec<ShellbagRecord>,
) {
    walk_inner(hive, key, bag_path, parent_abs, explored, out, true)
}

fn walk_inner(
    hive: &mut Hive,
    mut key: CellKeyNode,
    bag_path: &str,
    parent_abs: &str,
    explored: &HashSet<u32>,
    out: &mut Vec<ShellbagRecord>,
    is_root: bool,
) {
    // Root-level NodeSlot: SBECmd always emits 0 for every item at the root
    // BagMRU level, even when the BagMRU key itself carries a non-zero NodeSlot
    // value (observed: administrator hive NodeSlot=14 on BagMRU key, yet all
    // root items show NodeSlot=0 in the fixture). "0" is the hardcoded constant.
    let root_ns = if is_root {
        "0".to_string()
    } else {
        String::new()
    };

    // Parent key's LastWriteTime — used as LastInteracted for the MRU[0] slot.
    // Verified against the reference fixtures: the MRU[0] slot (most recently accessed
    // item) gets LastInteracted = this parent key's LWT.
    let kw = key.last_key_written_date_and_time();
    let parent_lwt =
        WinTimestamp::from_unix_nanos(kw.timestamp(), kw.timestamp_subsec_nanos()).to_string();
    let last_write = parent_lwt.clone();
    let mru = mru_positions(&key);

    // Determine the MRU[0] slot (most recently accessed = position 0).
    let mru0_slot: Option<u32> = mru
        .iter()
        .find_map(|(slot, &pos)| if pos == 0 { Some(*slot) } else { None });

    // Numbered values at this level = the shell items (slot -> raw value bytes).
    let mut slots: Vec<(u32, Vec<u8>)> = Vec::new();
    for v in key.value_iter() {
        if let Ok(slot) = v.get_pretty_name().parse::<u32>() {
            slots.push((slot, raw_bytes(&v)));
        }
    }
    slots.sort_by_key(|(s, _)| *s);

    let subkeys = hive.sub_keys(&mut key);

    // For non-root levels: build a per-slot NodeSlot map from child subkeys.
    // For root level: all items share `root_ns` (computed above).
    // Also track: for each child subkey, its LastWriteTime (used as FirstInteracted)
    // and whether it has grandchild numbered values (used to suppress FirstInteracted
    // when the child has further bag navigation history).
    let mut child_ns: HashMap<u32, String> = HashMap::new();
    let mut child_value_counts: HashMap<u32, usize> = HashMap::new();
    // child_lwt: the LWT string for child subkey BagMRU\<slot>, used as FirstInteracted.
    // Only populated when: child subkey exists AND has a NodeSlot value present.
    let mut child_lwt: HashMap<u32, String> = HashMap::new();
    // child_has_grandchildren: true when the child subkey has further numbered
    // values (child_bags > 0). FirstInteracted is suppressed in that case.
    let mut child_has_grandchildren: HashMap<u32, bool> = HashMap::new();

    for sk in &subkeys {
        if let Ok(s) = sk.key_name.parse::<u32>() {
            if !is_root {
                child_ns.insert(s, ns_str(node_slot_u32(sk)));
            }
            let cnt = sk
                .value_iter()
                .filter(|v| v.get_pretty_name().parse::<u32>().is_ok())
                .count();
            child_value_counts.insert(s, cnt);

            // Collect child LWT for FirstInteracted, but only when the child
            // subkey has a NodeSlot value present (NodeSlot absent → no timestamp).
            // Verified: root BagMRU\6 (Computers and Devices) has NodeSlot=absent
            // and fixture shows empty FirstInteracted even though child LWT exists.
            let child_node_slot_present = node_slot_u32(sk).is_some();
            if child_node_slot_present {
                let ck = sk.last_key_written_date_and_time();
                let cts =
                    WinTimestamp::from_unix_nanos(ck.timestamp(), ck.timestamp_subsec_nanos())
                        .to_string();
                child_lwt.insert(s, cts);
            }
            // Track whether child has grandchildren (numbered values in its own BagMRU).
            child_has_grandchildren.insert(s, cnt > 0);
        }
    }

    for (slot, value_bytes) in &slots {
        let body = item_body(value_bytes);
        let item = parse_item_cp(body, 1252);
        // SBETriage: prefer primary_name (always pieces[0], never a @shell32.dll
        // resource ref) over item.value, which for 2+-string items would be the
        // unresolvable LocalisedName that JLECmd/LECmd emit verbatim but SBECmd
        // resolves to the real display name — we use primary_name as the closest
        // cross-platform approximation of that resolved value.
        let sbe_value: String = item
            .extension
            .as_ref()
            .and_then(|b| b.primary_name.as_deref())
            .filter(|s| !s.trim_matches('\0').is_empty())
            .map(|s| s.trim_matches('\0').to_string())
            .unwrap_or_else(|| item.value.clone());
        let abs = join_abs(parent_abs, &sbe_value);
        let bb = item.extension.as_ref();

        let node_slot = if is_root {
            root_ns.clone()
        } else {
            child_ns
                .get(slot)
                .cloned()
                .unwrap_or_else(|| "0".to_string())
        };

        // HasExplored: True only when BOTH conditions hold:
        //   1. The item's class byte is folder-capable (can be navigated into).
        //      File items (zip archives viewed as files), Variable (credential-manager /
        //      network-computer), Network location, URI, etc. always yield False.
        //      EXCEPTION: items inside a zip archive path (any class byte) CAN be
        //      explored — zip content items use varying class bytes (0x00, 0x07, 0x61,
        //      0x40..=0x4F etc.) that our table can't fully classify, but SBECmd can.
        //      We detect the zip-context via `parent_abs` ending in ".zip" or containing
        //      ".zip\" — indicating the current level is inside an archive.
        //   2. The NodeSlot's Bags\<n>\Shell key has at least one GUID-named child
        //      subkey (pre-built `explored` set from `bags::explored_node_slots`).
        //
        // For NTUSER.DAT roots the caller passes an empty `explored` set (SBECmd
        // never emits HasExplored=True for NTUSER.DAT items — verified across all
        // NTUSER reference fixtures), so condition 2 always fails.
        let abs_lower = parent_abs.to_ascii_lowercase();
        let in_zip_context = abs_lower.ends_with(".zip")
            || abs_lower.contains(".zip\\")
            || abs_lower.contains(".zip/");
        let is_folder_capable =
            in_zip_context || is_folder_capable_class_outside_zip(item.class_type);
        let has_explored = is_folder_capable
            && node_slot
                .parse::<u32>()
                .map(|n| explored.contains(&n))
                .unwrap_or(false);

        // ── FirstInteracted / LastInteracted ─────────────────────────────────────
        // SBECmd derives these from the BagMRU key/subkey LastWriteTime (LWT),
        // NOT from the beef0026 extension block. Verified against reference evidence.
        //
        // Rule (verified across all 60 rows of the reference fixture):
        //
        //   FirstInteracted = child subkey (BagMRU_parent\slot) LWT, BUT ONLY when:
        //     a) The child subkey exists AND its NodeSlot value is present (non-absent)
        //     b) The child subkey has NO further numbered items (ChildBags == 0)
        //        (items with deeper navigation history don't get FirstInteracted)
        //     c) The slot is NOT MRU[0] (most recently accessed in the parent)
        //     EXCEPTION: if child exists, NodeSlot present, ChildBags==0 AND slot IS
        //     MRU[0] → BOTH First and Last get values (First = child LWT, Last = parent LWT).
        //
        //   LastInteracted = PARENT BagMRU key LWT, ONLY for the MRU[0] slot.
        //
        // Slots at NodeSlot=0 (root level) always get empty First/Last because the
        // root BagMRU items don't have individual child subkeys with NodeSlots.
        let is_mru0 = mru0_slot == Some(*slot);
        let child_has_no_grandchildren =
            !child_has_grandchildren.get(slot).copied().unwrap_or(false);
        let this_child_lwt = child_lwt.get(slot).cloned().unwrap_or_default();

        // FirstInteracted: child LWT when child exists (LWT present), has no grandchildren,
        // and (if is_mru0, we still emit it when child_bags==0).
        let first_interacted = if !this_child_lwt.is_empty() && child_has_no_grandchildren {
            this_child_lwt.clone()
        } else {
            String::new()
        };

        // LastInteracted: parent LWT for the MRU[0] slot.
        let last_interacted = if is_mru0 {
            parent_lwt.clone()
        } else {
            String::new()
        };

        // Suppress FirstInteracted for MRU[0] when child has grandchildren
        // (most recently accessed node with deeper history → only Last is shown).
        let first_interacted = if is_mru0 && !child_has_no_grandchildren {
            String::new()
        } else {
            first_interacted
        };

        out.push(ShellbagRecord {
            bag_path: bag_path.to_string(),
            slot: slot.to_string(),
            node_slot: node_slot.clone(),
            mru_position: mru.get(slot).map(|p| p.to_string()).unwrap_or_default(),
            absolute_path: abs,
            shell_type: shell_type_name(&item),
            value: sbe_value.clone(),
            child_bags: child_value_counts
                .get(slot)
                .copied()
                .unwrap_or(0)
                .to_string(),
            created_on: ts(bb.and_then(|b| b.created)),
            modified_on: ts(item.modified),
            accessed_on: ts(bb.and_then(|b| b.accessed)),
            last_write_time: last_write.clone(),
            mft_entry: bb
                .and_then(|b| b.mft_entry)
                .map(|n| n.to_string())
                .unwrap_or_default(),
            mft_sequence_number: bb
                .and_then(|b| b.mft_sequence)
                .map(|n| n.to_string())
                .unwrap_or_default(),
            extension_block_count: extension_block_count(body).to_string(),
            first_interacted,
            last_interacted,
            has_explored: if has_explored { "True" } else { "False" }.to_string(),
            // SBECmd emits "NTFS file system" in Miscellaneous only when an MFT
            // entry > 0 is present (entry=0 means $MFT / unset; Misc stays empty).
            miscellaneous: if bb.and_then(|b| b.mft_entry).is_some_and(|n| n > 0) {
                "NTFS file system".to_string()
            } else {
                String::new()
            },
        });
    }

    // Recurse into numbered subkeys, carrying the matching slot's absolute path.
    for sub in subkeys {
        if let Ok(slot) = sub.key_name.parse::<u32>() {
            let child_abs = slots
                .iter()
                .find(|(s, _)| *s == slot)
                .map(|(_, vb)| {
                    let child_item = parse_item_cp(item_body(vb), 1252);
                    let child_val = child_item
                        .extension
                        .as_ref()
                        .and_then(|b| b.primary_name.as_deref())
                        .filter(|s| !s.trim_matches('\0').is_empty())
                        .map(|s| s.trim_matches('\0').to_string())
                        .unwrap_or_else(|| child_item.value.clone());
                    join_abs(parent_abs, &child_val)
                })
                .unwrap_or_else(|| parent_abs.to_string());
            let child_bag_path = format!("{bag_path}\\{slot}");
            walk_inner(hive, sub, &child_bag_path, &child_abs, explored, out, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn extension_block_count_single_block() {
        // A single beef0004 block. Layout (block-relative offsets):
        //   [0..2] u16 size=0x0012 (18 bytes total), [2..4] version=9,
        //   [4..8] sig=0xBEEF0004 LE = [04, 00, EF, BE], [8..18] padding.
        // Signature LE bytes: sig[0]=0x04, sig[1]=0x00, sig[2]=0xEF, sig[3]=0xBE.
        // The "EF BE" appear at block offsets 6 and 7.
        let mut body = vec![0x00u8; 4]; // item prefix bytes before the chain
        body.extend_from_slice(&[
            0x12, 0x00, // size = 18
            0x09, 0x00, // version = 9
            0x04, 0x00, 0xEF, 0xBE, // sig = 0xBEEF0004 LE (EF BE at block[6..8])
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // body (10 bytes)
        ]);
        assert_eq!(extension_block_count(&body), 1);
    }

    #[test]
    fn extension_block_count_two_blocks() {
        // Two consecutive blocks.
        let mut body: Vec<u8> = vec![0u8; 4]; // item prefix
                                              // Block 1: size=0x12 (18 bytes), beef0004 sig (EF BE at block[6..8])
        body.extend_from_slice(&[0x12, 0x00, 0x09, 0x00, 0x04, 0x00, 0xEF, 0xBE]);
        body.extend_from_slice(&[0u8; 10]); // remaining 10 bytes of block 1
                                            // Block 2: size=0x14 (20 bytes), beef0026 sig (EF BE at block[6..8])
        body.extend_from_slice(&[0x14, 0x00, 0x03, 0x00, 0x26, 0x00, 0xEF, 0xBE]);
        body.extend_from_slice(&[0u8; 12]); // remaining 12 bytes of block 2
                                            // Terminator: size=0x00
        body.extend_from_slice(&[0x00, 0x00]);
        assert_eq!(extension_block_count(&body), 2);
    }

    #[test]
    fn extension_block_count_no_blocks() {
        assert_eq!(extension_block_count(&[0u8; 8]), 0);
        assert_eq!(extension_block_count(&[]), 0);
    }

    #[test]
    fn join_abs_roots_at_desktop() {
        assert_eq!(join_abs("", "Quick Access"), "Desktop\\Quick Access");
        assert_eq!(join_abs("Desktop\\This PC", "C:"), "Desktop\\This PC\\C:");
    }

    fn find_all_usrclass_with_logs() -> Vec<(PathBuf, Vec<PathBuf>)> {
        let mut found = Vec::new();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
        if !root.exists() {
            return found;
        }
        let mut stack = vec![root];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.file_name().and_then(|s| s.to_str()) == Some("UsrClass.dat") {
                    let dir = p.parent().unwrap();
                    let log1 = dir.join("UsrClass.dat.LOG1");
                    if log1.exists() {
                        let mut logs = vec![log1];
                        let log2 = dir.join("UsrClass.dat.LOG2");
                        if log2.exists() {
                            logs.push(log2);
                        }
                        found.push((p, logs));
                    }
                }
            }
        }
        found
    }

    #[test]
    fn walks_real_usrclass_bagmru() {
        let candidates = find_all_usrclass_with_logs();
        if candidates.is_empty() {
            eprintln!("SKIP: no UsrClass.dat with logs in test captures");
            return;
        }
        // Some users have empty/absent BagMRU; walk every candidate and validate
        // the first one that yields records (at least one MUST, proving the walker
        // works against real evidence).
        let mut first_nonempty: Option<Vec<ShellbagRecord>> = None;
        for (primary, logs) in &candidates {
            let mut hive = Hive::open(primary, logs, false).expect("open hive");
            let explored = crate::bags::explored_node_slots(
                &mut hive,
                r"Local Settings\Software\Microsoft\Windows\Shell\Bags",
            );
            let Some(root) =
                hive.get_key(r"Local Settings\Software\Microsoft\Windows\Shell\BagMRU")
            else {
                continue;
            };
            let mut out = Vec::new();
            walk(&mut hive, root, "BagMRU", "", &explored, &mut out);
            if !out.is_empty() {
                first_nonempty = Some(out);
                break;
            }
        }
        let out = first_nonempty.expect("at least one UsrClass hive should yield shellbags");
        assert_eq!(out[0].bag_path, "BagMRU");
        assert!(
            out[0].absolute_path.starts_with("Desktop\\"),
            "abs path was {:?}",
            out[0].absolute_path
        );
        assert!(!out[0].value.is_empty(), "value should be a real string");
    }
}
