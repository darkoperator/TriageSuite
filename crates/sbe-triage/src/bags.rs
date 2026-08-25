//! Bags\<NodeSlot> explored-state. A NodeSlot is "explored" when
//! Bags\<n>\Shell exists AND the Shell key itself has at least one child
//! subkey (a GUID-named folder-view key such as `{5C4F28B5-...}`). Built once
//! BEFORE the BagMRU walk to avoid notatin parser-state interleaving
//! (get_key must not be called mid-walk).
//!
//! ### Why subkeys, not Shell existence alone
//! SBECmd's HasExplored=True maps to Bags\<n>\Shell having at least one
//! GUID-named child subkey (folder view-mode storage). Some NodeSlots have a
//! Shell key with only values (e.g. SniffedFolderType) but no child subkeys;
//! those are items whose folder settings were initialised but not "explored"
//! (the view mode was never saved). SBECmd emits HasExplored=False for those.
//! Verified against the STCL1 cperez hive: NS=7 (C:, True) has Shell with
//! a GUID subkey; NS=16 (D:, False) has Shell with only a SniffedFolderType
//! value and no subkeys.

use std::collections::HashSet;
use triage_registry::hive::Hive;

/// Collect the set of NodeSlot numbers where `Bags\<n>\Shell` exists AND has
/// at least one child subkey (a folder-view GUID key). NodeSlots where Shell
/// exists with only values (no GUID subkeys) are excluded — SBECmd reports
/// those as HasExplored=False.
///
/// `bags_key_path` is the BagMRU-sibling `Bags` key path (root-stripped form
/// accepted by Hive::get_key). Empty set when the Bags key is absent.
pub fn explored_node_slots(hive: &mut Hive, bags_key_path: &str) -> HashSet<u32> {
    let mut set = HashSet::new();
    let Some(mut bags) = hive.get_key(bags_key_path) else {
        return set;
    };
    for mut n_key in hive.sub_keys(&mut bags) {
        let Ok(num) = n_key.key_name.parse::<u32>() else {
            continue;
        };
        // Find the Shell subkey of Bags\<n>.
        let shell_opt = hive
            .sub_keys(&mut n_key)
            .into_iter()
            .find(|k| k.key_name.eq_ignore_ascii_case("Shell"));
        let Some(mut shell) = shell_opt else { continue };
        // HasExplored=True only when Shell has at least one CHILD subkey
        // (a GUID-named folder-view storage key). Shell with only values
        // (e.g. SniffedFolderType) but no GUID subkeys → HasExplored=False.
        if !hive.sub_keys(&mut shell).is_empty() {
            set.insert(num);
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // Reuse the capture-finding pattern: walk `../../test captures` for a
    // UsrClass.dat with a .LOG1 sibling; open it; build the explored set from
    // `Local Settings\Software\Microsoft\Windows\Shell\Bags`. Assert it's a
    // valid set (possibly empty) without panic. SKIP if no capture.
    #[test]
    fn explored_set_builds_on_real_hive() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
        if !root.exists() {
            eprintln!("SKIP: no captures");
            return;
        }
        // find first UsrClass.dat + LOG1
        let mut stack = vec![root];
        let mut found: Option<(PathBuf, Vec<PathBuf>)> = None;
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
                    let l1 = dir.join("UsrClass.dat.LOG1");
                    if l1.exists() {
                        let mut logs = vec![l1];
                        let l2 = dir.join("UsrClass.dat.LOG2");
                        if l2.exists() {
                            logs.push(l2);
                        }
                        found = Some((p, logs));
                    }
                }
            }
            if found.is_some() {
                break;
            }
        }
        let Some((primary, logs)) = found else {
            eprintln!("SKIP: no UsrClass+logs");
            return;
        };
        let mut hive = Hive::open(&primary, &logs, false).expect("open");
        let _set = explored_node_slots(
            &mut hive,
            r"Local Settings\Software\Microsoft\Windows\Shell\Bags",
        );
        // No assertion on contents (varies); the point is it builds without panic.
    }
}
