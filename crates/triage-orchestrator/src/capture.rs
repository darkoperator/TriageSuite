use crate::file_name_lossy;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use triage_core::attribution::{sanitize_component, ComponentAllocator, MAX_COMPONENT_CHARS};

/// Files that make a directory a Velociraptor collection.
pub(crate) const COLLECTION_MARKERS: [&str; 2] = ["uploads.json", "client_info.json"];

const UNKNOWN_OS: &str = "unknown";

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureType {
    Velociraptor,
    Raw,
}

#[derive(Debug, Clone)]
pub struct HostCapture {
    pub host: String,
    pub output_id: String,
    pub os: String,
    pub collection_dir: PathBuf,
    pub artifact_root: PathBuf,
    /// Set when this host came out of a `.zip`, for manifest provenance.
    pub source_archive: Option<PathBuf>,
}

#[derive(Deserialize)]
struct ClientInfo {
    #[serde(rename = "Hostname")]
    hostname: Option<String>,
    #[serde(rename = "Platform")]
    platform: Option<String>,
    #[serde(rename = "PlatformVersion")]
    platform_version: Option<String>,
}

fn is_collection(dir: &Path) -> bool {
    COLLECTION_MARKERS.iter().all(|m| dir.join(m).is_file())
}

fn host_from_collection(dir: &Path) -> HostCapture {
    let fallback = file_name_lossy(dir);
    let (host, os) = match std::fs::read_to_string(dir.join("client_info.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<ClientInfo>(&s).ok())
    {
        Some(ci) => {
            let host = ci.hostname.filter(|h| !h.is_empty()).unwrap_or(fallback);
            let os = match (ci.platform, ci.platform_version) {
                (Some(p), Some(v)) => format!("{p} {v}"),
                (Some(p), None) => p,
                _ => UNKNOWN_OS.into(),
            };
            (host, os)
        }
        None => (fallback, UNKNOWN_OS.into()),
    };
    HostCapture {
        output_id: sanitize_component(&host),
        host,
        os,
        collection_dir: dir.to_path_buf(),
        artifact_root: dir.join("uploads"),
        source_archive: None,
    }
}

/// Collections at `path`: the directory itself if it is one, otherwise its
/// immediate children that are. No Raw fallback and no `output_id` allocation —
/// those belong to the caller, which may be merging several roots.
///
/// Checking both levels is what lets an extracted archive be passed in
/// directly, regardless of whether the collection sat at the archive root or
/// under a wrapper directory.
pub fn collect_collections(path: &Path) -> Vec<HostCapture> {
    if !path.is_dir() {
        return Vec::new();
    }
    if is_collection(path) {
        return vec![host_from_collection(path)];
    }
    let mut children: Vec<HostCapture> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(path) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() && is_collection(&p) {
                children.push(host_from_collection(&p));
            }
        }
    }
    children
}

/// A compact `YYYYMMDDThhmmss` lifted from a collection directory name, e.g.
/// `Collection-HOST-2026-07-24T04_57_26Z` -> `20260724T045726`. Velociraptor
/// stamps every collection this way, which makes it a stable identity for one
/// capture of a host.
fn collection_timestamp(name: &str) -> Option<String> {
    let b = name.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(|c| c.is_ascii_digit());
    let sep = |i: usize| b.get(i).is_some_and(|c| !c.is_ascii_alphanumeric());
    // YYYY-MM-DD T hh?mm?ss  (Velociraptor uses `_` where a time would use `:`)
    for i in 0..b.len() {
        let shape = (0..4).all(|k| digit(i + k))
            && sep(i + 4)
            && digit(i + 5)
            && digit(i + 6)
            && sep(i + 7)
            && digit(i + 8)
            && digit(i + 9)
            && b.get(i + 10).is_some_and(|c| *c == b'T' || *c == b't')
            && digit(i + 11)
            && digit(i + 12)
            && sep(i + 13)
            && digit(i + 14)
            && digit(i + 15)
            && sep(i + 16)
            && digit(i + 17)
            && digit(i + 18);
        if shape {
            let d = |r: std::ops::Range<usize>| -> String { r.map(|k| b[i + k] as char).collect() };
            return Some(format!(
                "{}{}{}T{}{}{}",
                d(0..4),
                d(5..7),
                d(8..10),
                d(11..13),
                d(14..16),
                d(17..19)
            ));
        }
    }
    None
}

/// Token distinguishing one collection of a host from another. Derived from
/// the collection itself — never from its position in the run — so a given
/// capture always resolves to the same output directory regardless of what
/// else is being processed alongside it.
fn collection_token(collection_dir: &Path) -> String {
    let name = file_name_lossy(collection_dir);
    collection_timestamp(&name).unwrap_or_else(|| sanitize_component(&name))
}

/// `base` cut so that `base + suffix` stays within the output-component limit.
fn fit_with_suffix(base: &str, suffix: &str) -> String {
    let keep = MAX_COMPONENT_CHARS.saturating_sub(suffix.chars().count());
    let trimmed: String = base.chars().take(keep).collect();
    format!("{trimmed}{suffix}")
}

/// Sort deterministically and assign collision-free `output_id`s across the
/// whole set. The `collection_dir` tie-break matters once several roots are
/// merged: the same hostname can legitimately appear twice (once zipped, once
/// already unzipped), and sorting on host alone would order them by `read_dir`,
/// i.e. non-deterministically.
fn finalize(hosts: &mut [HostCapture]) {
    hosts.sort_by(|a, b| {
        a.host
            .cmp(&b.host)
            .then_with(|| a.collection_dir.cmp(&b.collection_dir))
    });

    // ComponentAllocator disambiguates *different* hostnames that sanitize to
    // the same component, but deliberately returns the same id for the same
    // identity. Two collections of one host therefore land on one component —
    // and would write into a single output directory, silently overwriting one
    // capture's results with the other's.
    let mut allocator = ComponentAllocator::default();
    let bases: Vec<String> = hosts.iter().map(|h| allocator.allocate(&h.host)).collect();

    let mut contested: HashMap<&str, usize> = HashMap::new();
    for b in &bases {
        *contested.entry(b).or_default() += 1;
    }

    let mut used: HashSet<String> = HashSet::new();
    for (host, base) in hosts.iter_mut().zip(bases.iter()) {
        // Only a contested hostname gets a suffix, so the ordinary one
        // collection per host layout is exactly as it always was.
        let seed = if contested[base.as_str()] > 1 {
            let token = collection_token(&host.collection_dir);
            fit_with_suffix(base, &format!("_{token}"))
        } else {
            base.clone()
        };
        // Safety net: two collections could still share a token (same second,
        // or identically named dirs under different roots). Never silently
        // reuse a directory.
        let mut id = seed.clone();
        let mut n = 2u32;
        while !used.insert(id.clone()) {
            id = fit_with_suffix(&seed, &format!("-{n}"));
            n += 1;
        }
        host.output_id = id;
    }
}

/// Gather collections from several roots at once.
///
/// `raw_fallback` is the directory to treat as a single raw mounted tree when
/// no collection is found anywhere. Passing `None` makes "nothing found" an
/// error instead — which is what the archive path wants: a folder holding only
/// ZIPs must never be mistaken for a raw capture named after the folder.
pub fn enumerate_multi(
    roots: &[PathBuf],
    raw_fallback: Option<&Path>,
) -> Result<(CaptureType, Vec<HostCapture>), String> {
    let mut hosts: Vec<HostCapture> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for root in roots {
        for host in collect_collections(root) {
            // Canonicalize so the same collection reached through two roots
            // (e.g. --out nested inside the capture) is only counted once.
            let key = host
                .collection_dir
                .canonicalize()
                .unwrap_or_else(|_| host.collection_dir.clone());
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            hosts.push(host);
        }
    }
    if !hosts.is_empty() {
        finalize(&mut hosts);
        return Ok((CaptureType::Velociraptor, hosts));
    }
    let Some(path) = raw_fallback else {
        return Err("no Velociraptor collection found".to_string());
    };
    let host = file_name_lossy(path);
    Ok((
        CaptureType::Raw,
        vec![HostCapture {
            output_id: sanitize_component(&host),
            host,
            os: UNKNOWN_OS.into(),
            collection_dir: path.to_path_buf(),
            artifact_root: path.to_path_buf(),
            source_archive: None,
        }],
    ))
}

pub fn enumerate(path: &Path) -> Result<(CaptureType, Vec<HostCapture>), String> {
    if !path.is_dir() {
        return Err(format!("not a directory: {}", path.display()));
    }
    enumerate_multi(&[path.to_path_buf()], Some(path))
}

/// A host named `H` whose collection and artifact root are both `root`.
#[cfg(test)]
pub(crate) fn test_host(root: &Path) -> HostCapture {
    HostCapture {
        host: "H".into(),
        output_id: "H".into(),
        os: UNKNOWN_OS.into(),
        collection_dir: root.to_path_buf(),
        artifact_root: root.to_path_buf(),
        source_archive: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use triage_testkit::synthetic::{write_collection, COLLECTION_OS};

    #[test]
    fn enumerate_multi_merges_roots_with_unique_output_ids() {
        let td = TempDir::new().unwrap();
        let a = td.path().join("rootA");
        let b = td.path().join("rootB");
        write_collection(&a.join("Collection-A"), "A");
        write_collection(&b.join("Collection-B"), "B");
        let (ty, hosts) = enumerate_multi(&[a, b], None).unwrap();
        assert!(matches!(ty, CaptureType::Velociraptor));
        let names: Vec<_> = hosts.iter().map(|h| h.host.as_str()).collect();
        assert_eq!(names, vec!["A", "B"]);
        assert_ne!(hosts[0].output_id, hosts[1].output_id);
    }

    #[test]
    fn collection_timestamp_is_lifted_from_velociraptor_names() {
        assert_eq!(
            collection_timestamp("Collection-IT02877_rlc_com-2026-07-24T04_57_26Z").as_deref(),
            Some("20260724T045726")
        );
        // Colons instead of underscores, as some exports write them.
        assert_eq!(
            collection_timestamp("Collection-H-2026-03-11T21:20:14Z").as_deref(),
            Some("20260311T212014")
        );
        assert_eq!(collection_timestamp("Collection-NoTimestampHere"), None);
    }

    #[test]
    fn one_collection_per_host_keeps_the_bare_hostname() {
        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        write_collection(&root.join("Collection-A-2026-07-24T04_57_26Z"), "HOSTA");
        write_collection(&root.join("Collection-B-2026-07-24T04_57_26Z"), "HOSTB");
        let (_, hosts) = enumerate_multi(&[root], None).unwrap();
        let ids: Vec<_> = hosts.iter().map(|h| h.output_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["HOSTA", "HOSTB"],
            "uncontested names must not move"
        );
    }

    /// The property this scheme exists for: an output directory is a function
    /// of the collection alone, so adding another capture of the same host
    /// later must not relocate the ones already written.
    #[test]
    fn output_id_does_not_move_when_another_collection_is_added() {
        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        write_collection(&root.join("Collection-H-2026-07-24T04_57_26Z"), "H");
        write_collection(&root.join("Collection-H-2026-07-28T11_02_13Z"), "H");
        let (_, before) = enumerate_multi(std::slice::from_ref(&root), None).unwrap();
        let ids_before: Vec<_> = before.iter().map(|h| h.output_id.clone()).collect();
        assert_eq!(
            ids_before,
            vec!["H_20260724T045726", "H_20260728T110213"],
            "contested names carry their own collection timestamp"
        );

        // An *earlier* capture turns up and is added to the same folder.
        write_collection(&root.join("Collection-H-2026-06-01T09_00_00Z"), "H");
        let (_, after) = enumerate_multi(&[root], None).unwrap();
        for id in &ids_before {
            assert!(
                after.iter().any(|h| &h.output_id == id),
                "{id} moved after a third collection was added: {:?}",
                after.iter().map(|h| &h.output_id).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn identical_tokens_still_get_unique_directories() {
        // Same host, same timestamp, two roots: the safety net must fire
        // rather than let both write into one directory.
        let td = TempDir::new().unwrap();
        let a = td.path().join("a");
        let b = td.path().join("b");
        write_collection(&a.join("Collection-H-2026-07-24T04_57_26Z"), "H");
        write_collection(&b.join("Collection-H-2026-07-24T04_57_26Z"), "H");
        let (_, hosts) = enumerate_multi(&[a, b], None).unwrap();
        assert_eq!(hosts.len(), 2);
        assert_ne!(hosts[0].output_id, hosts[1].output_id);
    }

    #[test]
    fn same_hostname_in_two_roots_gets_distinct_output_ids() {
        let td = TempDir::new().unwrap();
        let a = td.path().join("rootA");
        let b = td.path().join("rootB");
        write_collection(&a.join("Collection-1"), "DUPE");
        write_collection(&b.join("Collection-2"), "DUPE");
        let (_, hosts) = enumerate_multi(&[a, b], None).unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].host, "DUPE");
        assert_eq!(hosts[1].host, "DUPE");
        assert_ne!(
            hosts[0].output_id, hosts[1].output_id,
            "same hostname from two roots must not share an output dir"
        );
    }

    #[test]
    fn enumerate_multi_without_raw_fallback_errors_instead_of_inventing_a_host() {
        // The folder-of-zips case: nothing found must be an error, never a
        // Raw host named after the folder.
        let td = TempDir::new().unwrap();
        let empty = td.path().join("only-zips");
        fs::create_dir_all(&empty).unwrap();
        fs::write(empty.join("a.zip"), b"x").unwrap();
        assert!(enumerate_multi(&[empty], None).is_err());
    }

    #[test]
    fn enumerate_multi_tolerates_a_missing_root() {
        let td = TempDir::new().unwrap();
        let real = td.path().join("real");
        write_collection(&real.join("Collection-A"), "A");
        let missing = td.path().join("does-not-exist");
        let (_, hosts) = enumerate_multi(&[missing, real], None).unwrap();
        assert_eq!(hosts.len(), 1);
    }

    #[test]
    fn enumerate_multi_deduplicates_a_collection_reached_twice() {
        let td = TempDir::new().unwrap();
        let root = td.path().join("root");
        let coll = root.join("Collection-A");
        write_collection(&coll, "A");
        // Same collection via its parent and via itself.
        let (_, hosts) = enumerate_multi(&[root, coll], None).unwrap();
        assert_eq!(hosts.len(), 1, "collection counted twice");
    }

    #[test]
    fn detects_single_velociraptor_collection() {
        let td = TempDir::new().unwrap();
        let coll = td.path().join("Collection-HOST1-2026");
        write_collection(&coll, "HOST1");
        let (ty, hosts) = enumerate(&coll).unwrap();
        assert!(matches!(ty, CaptureType::Velociraptor));
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].host, "HOST1");
        assert_eq!(hosts[0].os, COLLECTION_OS);
        assert_eq!(hosts[0].artifact_root, coll.join("uploads"));
    }

    #[test]
    fn detects_parent_of_multiple_collections() {
        let td = TempDir::new().unwrap();
        write_collection(&td.path().join("Collection-A-2026"), "A");
        write_collection(&td.path().join("Collection-B-2026"), "B");
        let (_, hosts) = enumerate(td.path()).unwrap();
        let mut names: Vec<_> = hosts.iter().map(|h| h.host.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["A", "B"]);
    }

    #[test]
    fn falls_back_to_raw() {
        let td = TempDir::new().unwrap();
        fs::create_dir_all(td.path().join("Windows/Prefetch")).unwrap();
        let (ty, hosts) = enumerate(td.path()).unwrap();
        assert!(matches!(ty, CaptureType::Raw));
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].artifact_root, td.path());
    }

    #[test]
    fn hostile_and_colliding_hostnames_get_safe_unique_output_ids() {
        let td = TempDir::new().unwrap();
        write_collection(&td.path().join("Collection-A"), "a/b");
        write_collection(&td.path().join("Collection-B"), "a\\b");
        let (_, hosts) = enumerate(td.path()).unwrap();
        assert_eq!(hosts.len(), 2);
        assert!(hosts
            .iter()
            .all(|host| !host.output_id.contains(['/', '\\'])));
        assert_ne!(hosts[0].output_id, hosts[1].output_id);
        assert!(hosts.iter().any(|host| host.host == "a/b"));
    }
}
