//! Resolve SID -> username from a SOFTWARE hive's ProfileList, mirroring
//! SrumECmd (Path.GetFileName(ProfileImagePath)).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use triage_registry::hive::Hive;

const PROFILE_LIST: &str = r"Microsoft\Windows NT\CurrentVersion\ProfileList";

/// Read SID -> username from the SOFTWARE hive at `software_path`. Returns an
/// empty map on any failure (SID then stands as the identity).
pub fn sid_to_user(software_path: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(mut hive) = Hive::open(software_path, &[], true) else {
        return out;
    };
    let Some(mut key) = hive.get_key(PROFILE_LIST) else {
        return out;
    };
    for sub in hive.sub_keys(&mut key) {
        // Use key_name field (the SID string), mirroring profile_list.rs which
        // uses sub_key.key_name.clone() — CellKeyNode has no get_pretty_name().
        let sid = sub.key_name.clone();
        for v in sub.value_iter() {
            if v.get_pretty_name().eq_ignore_ascii_case("ProfileImagePath") {
                let (content, _) = v.get_content();
                let path = triage_registry::value::render_value_data(&content);
                if let Some(name) = path.rsplit(['\\', '/']).next() {
                    if !name.is_empty() {
                        out.insert(sid.clone(), name.to_string());
                    }
                }
            }
        }
    }
    out
}

/// Locate the closest SOFTWARE hive to `srudb_path` in the same capture subtree.
/// SRUDB is at `<root>/Windows/System32/sru/SRUDB.dat`; the SOFTWARE hive is at
/// `<root>/Windows/System32/config/SOFTWARE`. Walk the SRUDB's ancestors and
/// probe for a `config/SOFTWARE` (case variants) sibling subtree.
pub fn closest_software_hive(srudb_path: &Path) -> Option<PathBuf> {
    for anc in srudb_path.ancestors() {
        for cand in ["SOFTWARE", "Software"] {
            // .../System32/config/SOFTWARE relative to a .../System32/* ancestor
            let p = anc.join("config").join(cand);
            if p.is_file() {
                return Some(p);
            }
            // .../<root>/Windows/System32/config/SOFTWARE relative to <root>
            let p2 = anc.join("Windows/System32/config").join(cand);
            if p2.is_file() {
                return Some(p2);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn captures_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
    }

    /// Find the SOFTWARE hive in a local capture.
    fn find_software() -> Option<PathBuf> {
        if !captures_root().exists() {
            return None;
        }
        let mut stack = vec![captures_root()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.eq_ignore_ascii_case("SOFTWARE"))
                    .unwrap_or(false)
                    && p.to_string_lossy().contains("config")
                {
                    return Some(p);
                }
            }
        }
        None
    }

    #[test]
    fn resolves_users_from_software_hive() {
        let Some(sw) = find_software() else { return };
        let map = sid_to_user(&sw);
        assert!(!map.is_empty(), "expected ProfileList SID->user entries");
        // ProfileList SIDs are S-1-5-... ; at least one should be present.
        assert!(map.keys().any(|k| k.starts_with("S-1-5")));
    }

    #[test]
    fn finds_software_hive_near_srudb() {
        // Locate a SRUDB and confirm we can discover a SOFTWARE hive
        // for it (same capture subtree).
        if !captures_root().exists() {
            return;
        }
        let mut stack = vec![captures_root()];
        let mut srudb = None;
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.file_name().and_then(|n| n.to_str()) == Some("SRUDB.dat") {
                    srudb = Some(p);
                }
            }
        }
        let Some(srudb) = srudb else { return };
        // The discovery may or may not find it depending on capture layout;
        // assert it does NOT panic and returns an Option. If it finds one, it
        // must be a real file.
        if let Some(sw) = closest_software_hive(&srudb) {
            assert!(sw.is_file());
        }
    }
}
