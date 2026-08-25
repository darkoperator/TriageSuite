//! Fetches and reduces the LOLDrivers/LOLRMM reference datasets, replacing
//! the old `refdata/fetch_refs.py` offline script — invoked via `LolTriage
//! --update-refs`. Reduction logic (the part that matters for correctness)
//! is unit-tested against fixture JSON; the network fetch is not (no live
//! network in tests), matching the crate's existing testing discipline.

use crate::refdata::{LolDriverEntry, LolRmmEntry};
use serde_json::Value;
use std::path::Path;
use triage_core::error::TriageError;

pub const LOLDRIVERS_URL: &str = "https://www.loldrivers.io/api/drivers.json";
pub const LOLRMM_URL: &str = "https://lolrmm.io/api/rmm_tools.json";

/// Writes reduced entries to `<dir>/loldrivers_refs.json` and
/// `<dir>/lolrmm_refs.json` (pretty-printed, matching the vendored
/// snapshots' existing style).
pub fn write_refs(
    dir: &Path,
    drivers: &[LolDriverEntry],
    rmm: &[LolRmmEntry],
) -> Result<(), TriageError> {
    write_json(&dir.join("loldrivers_refs.json"), drivers)?;
    write_json(&dir.join("lolrmm_refs.json"), rmm)?;
    Ok(())
}

/// Fetches both upstream datasets, reduces them, and writes the result to
/// `out_dir`. Returns `(driver_entry_count, rmm_entry_count)`. Not unit
/// tested (requires live network access to loldrivers.io/lolrmm.io) —
/// `reduce_loldrivers`/`reduce_lolrmm`/`write_refs` above carry the tested
/// logic; this function is thin I/O glue over them, verified manually the
/// same way the original offline `fetch_refs.py` script was.
pub fn run(out_dir: &Path) -> Result<(usize, usize), TriageError> {
    let drivers_raw = fetch_json(LOLDRIVERS_URL)?;
    let rmm_raw = fetch_json(LOLRMM_URL)?;
    let drivers = reduce_loldrivers(&drivers_raw);
    let rmm = reduce_lolrmm(&rmm_raw);
    write_refs(out_dir, &drivers, &rmm)?;
    Ok((drivers.len(), rmm.len()))
}

fn fetch_json(url: &str) -> Result<Value, TriageError> {
    // `into_string()` caps response size well below the LOLDrivers payload
    // (tens of MB); stream-parse from the reader instead, which has no such cap.
    let reader = ureq::get(url)
        .call()
        .map_err(|e| TriageError::Fatal(format!("fetching {url}: {e}")))?
        .into_reader();
    serde_json::from_reader(reader)
        .map_err(|e| TriageError::Fatal(format!("parsing JSON from {url}: {e}")))
}

fn write_json<T: serde::Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), TriageError> {
    let text = serde_json::to_string_pretty(value).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    std::fs::write(path, text).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

/// Final path segment of a Windows-style glob pattern, lower-cased.
/// `""` in, `""` out — callers must filter empty results themselves (an
/// install path with a trailing separator, e.g. `C:\Program Files\Foo\`,
/// reduces to `""`, and indexing that would match every source row with a
/// blank path/filename column).
fn basename(pattern: &str) -> String {
    pattern
        .replace('/', "\\")
        .rsplit('\\')
        .next()
        .unwrap_or(pattern)
        .to_ascii_lowercase()
}

pub fn reduce_loldrivers(raw: &Value) -> Vec<LolDriverEntry> {
    let mut out = Vec::new();
    let Some(entries) = raw.as_array() else {
        return out;
    };
    for e in entries {
        let id = e
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let category = e
            .get("Category")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mitre_id = e
            .get("MitreID")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let tags: Vec<String> = e
            .get("Tags")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let empty_sample = vec![Value::Object(serde_json::Map::new())];
        let samples = e
            .get("KnownVulnerableSamples")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
            .unwrap_or(&empty_sample);

        for s in samples {
            out.push(LolDriverEntry {
                id: id.clone(),
                category: category.clone(),
                mitre_id: mitre_id.clone(),
                tags: tags.clone(),
                md5: s
                    .get("MD5")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                sha1: s
                    .get("SHA1")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                sha256: s
                    .get("SHA256")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            });
        }
    }
    out
}

pub fn reduce_lolrmm(raw: &Value) -> Vec<LolRmmEntry> {
    let mut out = Vec::new();
    let Some(entries) = raw.as_array() else {
        return out;
    };
    for e in entries {
        let name = e
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let category = e
            .get("Category")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let install_paths = e
            .get("Details")
            .and_then(|d| d.get("InstallationPaths"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut install_basenames: Vec<String> = install_paths
            .iter()
            .filter_map(Value::as_str)
            .map(basename)
            .filter(|b| !b.is_empty())
            .collect();
        install_basenames.sort();
        install_basenames.dedup();

        let sha256_hashes: Vec<String> = e
            .get("CodeSigning")
            .and_then(|c| c.get("certificates"))
            .and_then(Value::as_array)
            .map(|certs| {
                certs
                    .iter()
                    .filter_map(|c| c.get("src_file_sha256").and_then(Value::as_str))
                    .filter(|h| !h.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        out.push(LolRmmEntry {
            name,
            category,
            install_basenames,
            sha256_hashes,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reduce_loldrivers_expands_one_row_per_known_vulnerable_sample() {
        let raw = json!([{
            "Id": "2a6a38ca-f2e6-456e-9ccf-db59d8c80c9e",
            "Category": "vulnerable driver",
            "MitreID": "T1068",
            "Tags": ["nvflash.sys"],
            "KnownVulnerableSamples": [
                {"MD5": "ba86e444ae837476e7ccdd06f8867795", "SHA1": "b9c3f4dcc7463cbec84b808d880194bbc304ccd0", "SHA256": "9368e51ec98e2ad20893a5fc21e6a8b20c5bee158d5c49ca58649cff84db9d68"},
                {"MD5": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "SHA1": "", "SHA256": ""}
            ]
        }]);

        let entries = reduce_loldrivers(&raw);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "2a6a38ca-f2e6-456e-9ccf-db59d8c80c9e");
        assert_eq!(entries[0].category, "vulnerable driver");
        assert_eq!(entries[0].mitre_id, "T1068");
        assert_eq!(entries[0].tags, vec!["nvflash.sys".to_string()]);
        assert_eq!(entries[0].sha1, "b9c3f4dcc7463cbec84b808d880194bbc304ccd0");
        assert_eq!(entries[1].md5, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(entries[1].sha1, "");
    }

    #[test]
    fn reduce_loldrivers_with_no_samples_yields_one_entry_with_empty_hashes() {
        let raw = json!([{
            "Id": "no-samples-entry",
            "Category": "vulnerable driver",
            "MitreID": "T1068",
            "Tags": ["onlytag.sys"]
        }]);

        let entries = reduce_loldrivers(&raw);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tags, vec!["onlytag.sys".to_string()]);
        assert_eq!(entries[0].md5, "");
        assert_eq!(entries[0].sha1, "");
        assert_eq!(entries[0].sha256, "");
    }

    #[test]
    fn reduce_lolrmm_filters_empty_basenames_from_trailing_separator_paths() {
        let raw = json!([{
            "Name": "TeamViewer",
            "Category": "RMM",
            "Details": {
                "InstallationPaths": [
                    "C:\\Program Files\\TeamViewer\\",
                    "C:\\*\\teamviewer.exe"
                ]
            }
        }]);

        let entries = reduce_lolrmm(&raw);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "TeamViewer");
        assert_eq!(
            entries[0].install_basenames,
            vec!["teamviewer.exe".to_string()],
            "the trailing-separator path must reduce to an empty basename and be filtered out"
        );
    }

    #[test]
    fn reduce_lolrmm_collects_nonempty_code_signing_sha256_hashes() {
        let raw = json!([{
            "Name": "KiTTY",
            "Category": "RAT",
            "Details": {"InstallationPaths": ["C:\\*\\kitty.exe"]},
            "CodeSigning": {
                "certificates": [
                    {"src_file_sha256": "abc123"},
                    {"src_file_sha256": ""},
                    {}
                ]
            }
        }]);

        let entries = reduce_lolrmm(&raw);

        assert_eq!(entries[0].sha256_hashes, vec!["abc123".to_string()]);
    }

    #[test]
    fn write_refs_writes_both_files_as_parseable_reduced_json() {
        let tmp = tempfile::tempdir().unwrap();
        let drivers = vec![LolDriverEntry {
            id: "an-id".into(),
            category: "vulnerable driver".into(),
            mitre_id: "T1068".into(),
            tags: vec!["nvflash.sys".into()],
            md5: String::new(),
            sha1: "b9c3f4dcc7463cbec84b808d880194bbc304ccd0".into(),
            sha256: String::new(),
        }];
        let rmm = vec![LolRmmEntry {
            name: "KiTTY".into(),
            category: "RAT".into(),
            install_basenames: vec!["kitty.exe".into()],
            sha256_hashes: vec![],
        }];

        write_refs(tmp.path(), &drivers, &rmm).unwrap();

        let written_drivers: Vec<LolDriverEntry> = serde_json::from_str(
            &std::fs::read_to_string(tmp.path().join("loldrivers_refs.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(written_drivers.len(), 1);
        assert_eq!(written_drivers[0].id, "an-id");
        assert_eq!(
            written_drivers[0].sha1,
            "b9c3f4dcc7463cbec84b808d880194bbc304ccd0"
        );

        let written_rmm: Vec<LolRmmEntry> = serde_json::from_str(
            &std::fs::read_to_string(tmp.path().join("lolrmm_refs.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(written_rmm.len(), 1);
        assert_eq!(written_rmm[0].name, "KiTTY");
    }

    #[test]
    fn basename_handles_mixed_separators_and_trailing_separator() {
        assert_eq!(basename("C:\\*\\kitty.exe"), "kitty.exe");
        assert_eq!(basename("C:/Program Files/Foo/bar.exe"), "bar.exe");
        assert_eq!(basename("C:\\Program Files\\TeamViewer\\"), "");
        assert_eq!(basename(""), "");
    }
}
