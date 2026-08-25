use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use triage_core::error::TriageError;

/// Reference snapshots compiled into the binary so a copied `LolTriage` works
/// with no `--refs` directory present on the host.
const EMBEDDED_LOLDRIVERS: &str = include_str!("../refdata/loldrivers_refs.json");
const EMBEDDED_LOLRMM: &str = include_str!("../refdata/lolrmm_refs.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LolDriverEntry {
    pub id: String,
    pub category: String,
    pub mitre_id: String,
    pub tags: Vec<String>,
    pub md5: String,
    pub sha1: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LolRmmEntry {
    pub name: String,
    pub category: String,
    pub install_basenames: Vec<String>,
    pub sha256_hashes: Vec<String>,
}

/// Reference lists plus the lower-cased lookup indexes built once at load time.
///
/// The entry vectors are private so the indexes can never drift out of sync
/// with them; construct with [`LolRefs::new`], [`LolRefs::load`], or
/// [`LolRefs::embedded`].
pub struct LolRefs {
    drivers: Vec<LolDriverEntry>,
    rmm: Vec<LolRmmEntry>,
    /// lower-cased md5/sha1/sha256 -> first driver index carrying it
    driver_by_hash: HashMap<String, usize>,
    /// lower-cased tag -> first driver index carrying it
    driver_by_tag: HashMap<String, usize>,
    /// lower-cased install basename -> first RMM index carrying it
    rmm_by_install_basename: HashMap<String, usize>,
}

impl LolRefs {
    /// Take ownership of both reference lists and build the lookup indexes.
    ///
    /// Empty hash/tag/basename values are never indexed: the vendored data
    /// contains empty strings (e.g. an install path with a trailing separator
    /// reduces to `""`), and indexing them would make every source row with a
    /// blank path/filename column match those entries.
    pub fn new(drivers: Vec<LolDriverEntry>, rmm: Vec<LolRmmEntry>) -> LolRefs {
        let mut driver_by_hash: HashMap<String, usize> = HashMap::new();
        let mut driver_by_tag: HashMap<String, usize> = HashMap::new();
        for (i, d) in drivers.iter().enumerate() {
            for hash in [&d.md5, &d.sha1, &d.sha256] {
                if !hash.is_empty() {
                    // `or_insert` keeps the lowest index, preserving the
                    // first-match semantics of the previous linear scan.
                    driver_by_hash.entry(hash.to_ascii_lowercase()).or_insert(i);
                }
            }
            for tag in &d.tags {
                if !tag.is_empty() {
                    driver_by_tag.entry(tag.to_ascii_lowercase()).or_insert(i);
                }
            }
        }

        let mut rmm_by_install_basename: HashMap<String, usize> = HashMap::new();
        for (i, r) in rmm.iter().enumerate() {
            for basename in &r.install_basenames {
                if !basename.is_empty() {
                    rmm_by_install_basename
                        .entry(basename.to_ascii_lowercase())
                        .or_insert(i);
                }
            }
        }

        LolRefs {
            drivers,
            rmm,
            driver_by_hash,
            driver_by_tag,
            rmm_by_install_basename,
        }
    }

    /// Load both reference snapshots from a directory (the `--refs` override).
    pub fn load(dir: &Path) -> Result<LolRefs, TriageError> {
        let drivers = load_json(&dir.join("loldrivers_refs.json"))?;
        let rmm = load_json(&dir.join("lolrmm_refs.json"))?;
        Ok(LolRefs::new(drivers, rmm))
    }

    /// Parse the snapshots embedded in this binary at build time.
    pub fn embedded() -> Result<LolRefs, TriageError> {
        let drivers: Vec<LolDriverEntry> = serde_json::from_str(EMBEDDED_LOLDRIVERS)
            .map_err(|e| TriageError::Fatal(format!("embedded loldrivers_refs.json: {e}")))?;
        let rmm: Vec<LolRmmEntry> = serde_json::from_str(EMBEDDED_LOLRMM)
            .map_err(|e| TriageError::Fatal(format!("embedded lolrmm_refs.json: {e}")))?;
        Ok(LolRefs::new(drivers, rmm))
    }

    pub fn drivers(&self) -> &[LolDriverEntry] {
        &self.drivers
    }

    pub fn rmm(&self) -> &[LolRmmEntry] {
        &self.rmm
    }

    pub fn driver_by_hash(&self, hash: &str) -> Option<&LolDriverEntry> {
        if hash.is_empty() {
            return None;
        }
        self.driver_by_hash
            .get(&hash.to_ascii_lowercase())
            .map(|&i| &self.drivers[i])
    }

    pub fn driver_by_basename(&self, basename: &str) -> Option<&LolDriverEntry> {
        if basename.is_empty() {
            return None;
        }
        self.driver_by_tag
            .get(&basename.to_ascii_lowercase())
            .map(|&i| &self.drivers[i])
    }

    pub fn rmm_by_basename(&self, basename: &str) -> Option<&LolRmmEntry> {
        if basename.is_empty() {
            return None;
        }
        self.rmm_by_install_basename
            .get(&basename.to_ascii_lowercase())
            .map(|&i| &self.rmm[i])
    }
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>, TriageError> {
    let text = std::fs::read_to_string(path).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    serde_json::from_str(&text).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fixture(dir: &Path) {
        std::fs::write(
            dir.join("loldrivers_refs.json"),
            r#"[
                {"id":"2a6a38ca-f2e6-456e-9ccf-db59d8c80c9e","category":"vulnerable driver","mitre_id":"T1068","tags":["nvflash.sys"],"md5":"ba86e444ae837476e7ccdd06f8867795","sha1":"b9c3f4dcc7463cbec84b808d880194bbc304ccd0","sha256":"9368e51ec98e2ad20893a5fc21e6a8b20c5bee158d5c49ca58649cff84db9d68"}
            ]"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("lolrmm_refs.json"),
            r#"[
                {"name":"KiTTY","category":"RAT","install_basenames":["kitty.exe"],"sha256_hashes":[]}
            ]"#,
        )
        .unwrap();
    }

    #[test]
    fn load_and_lookup_by_hash_and_basename() {
        let tmp = tempfile::tempdir().unwrap();
        write_fixture(tmp.path());
        let refs = LolRefs::load(tmp.path()).unwrap();

        let by_hash = refs
            .driver_by_hash("B9C3F4DCC7463CBEC84B808D880194BBC304CCD0")
            .unwrap();
        assert_eq!(by_hash.id, "2a6a38ca-f2e6-456e-9ccf-db59d8c80c9e");

        let by_tag = refs.driver_by_basename("NVFLASH.SYS").unwrap();
        assert_eq!(by_tag.mitre_id, "T1068");

        let rmm = refs.rmm_by_basename("KiTTY.exe").unwrap();
        assert_eq!(rmm.name, "KiTTY");

        assert!(refs.driver_by_hash("deadbeef").is_none());
        assert!(refs.rmm_by_basename("notepad.exe").is_none());
    }

    /// Reference entries carry empty hash/tag/basename fields (LOLDrivers rows
    /// without an MD5, LOLRMM install paths that reduce to ""). An empty query
    /// — from a source row with a blank path/filename column — must never
    /// match them.
    #[test]
    fn empty_query_never_matches_empty_reference_fields() {
        let refs = LolRefs::new(
            vec![LolDriverEntry {
                id: "driver-with-blank-fields".into(),
                category: "vulnerable driver".into(),
                mitre_id: "T1068".into(),
                tags: vec!["".into(), "nvflash.sys".into()],
                md5: String::new(),
                sha1: "b9c3f4dcc7463cbec84b808d880194bbc304ccd0".into(),
                sha256: String::new(),
            }],
            vec![LolRmmEntry {
                name: "TeamViewer".into(),
                category: "RMM".into(),
                install_basenames: vec!["".into(), "teamviewer.exe".into()],
                sha256_hashes: vec![],
            }],
        );

        // The bug: an empty query used to match the empty reference values.
        assert!(refs.driver_by_hash("").is_none());
        assert!(refs.driver_by_basename("").is_none());
        assert!(refs.rmm_by_basename("").is_none());

        // Real lookups on the same entries still work.
        assert_eq!(
            refs.driver_by_hash("B9C3F4DCC7463CBEC84B808D880194BBC304CCD0")
                .unwrap()
                .id,
            "driver-with-blank-fields"
        );
        assert_eq!(
            refs.driver_by_basename("NVFLASH.SYS").unwrap().id,
            "driver-with-blank-fields"
        );
        assert_eq!(
            refs.rmm_by_basename("TeamViewer.exe").unwrap().name,
            "TeamViewer"
        );
    }

    #[test]
    fn embedded_snapshots_parse_and_index() {
        let refs = LolRefs::embedded().unwrap();
        assert!(!refs.drivers().is_empty());
        assert!(!refs.rmm().is_empty());
        // Belt-and-suspenders: the vendored data must carry no empty basenames.
        assert!(
            refs.rmm()
                .iter()
                .all(|r| !r.install_basenames.iter().any(|b| b.is_empty())),
            "vendored lolrmm_refs.json still contains empty install_basenames"
        );
        assert!(refs.rmm_by_basename("").is_none());
        assert!(refs.driver_by_basename("").is_none());
        assert!(refs.driver_by_hash("").is_none());
    }

    #[test]
    fn first_match_wins_when_two_entries_share_a_tag() {
        let refs = LolRefs::new(
            vec![
                LolDriverEntry {
                    id: "first".into(),
                    category: "vulnerable driver".into(),
                    mitre_id: "T1068".into(),
                    tags: vec!["shared.sys".into()],
                    md5: String::new(),
                    sha1: String::new(),
                    sha256: String::new(),
                },
                LolDriverEntry {
                    id: "second".into(),
                    category: "vulnerable driver".into(),
                    mitre_id: "T1068".into(),
                    tags: vec!["shared.sys".into()],
                    md5: String::new(),
                    sha1: String::new(),
                    sha256: String::new(),
                },
            ],
            vec![],
        );
        assert_eq!(refs.driver_by_basename("shared.sys").unwrap().id, "first");
    }
}
