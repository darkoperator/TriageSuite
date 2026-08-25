use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
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
    dir.join("uploads.json").is_file() && dir.join("client_info.json").is_file()
}

fn host_from_collection(dir: &Path) -> HostCapture {
    let fallback = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (host, os) = match std::fs::read_to_string(dir.join("client_info.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<ClientInfo>(&s).ok())
    {
        Some(ci) => {
            let host = ci.hostname.filter(|h| !h.is_empty()).unwrap_or(fallback);
            let os = match (ci.platform, ci.platform_version) {
                (Some(p), Some(v)) => format!("{p} {v}"),
                (Some(p), None) => p,
                _ => "unknown".into(),
            };
            (host, os)
        }
        None => (fallback, "unknown".into()),
    };
    HostCapture {
        output_id: triage_core::attribution::sanitize_component(&host),
        host,
        os,
        collection_dir: dir.to_path_buf(),
        artifact_root: dir.join("uploads"),
    }
}

pub fn enumerate(path: &Path) -> Result<(CaptureType, Vec<HostCapture>), String> {
    if !path.is_dir() {
        return Err(format!("not a directory: {}", path.display()));
    }
    if is_collection(path) {
        return Ok((CaptureType::Velociraptor, vec![host_from_collection(path)]));
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
    if !children.is_empty() {
        children.sort_by(|a, b| a.host.cmp(&b.host));
        let mut allocator = triage_core::attribution::ComponentAllocator::default();
        for host in &mut children {
            host.output_id = allocator.allocate(&host.host);
        }
        return Ok((CaptureType::Velociraptor, children));
    }
    let host = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok((
        CaptureType::Raw,
        vec![HostCapture {
            output_id: triage_core::attribution::sanitize_component(&host),
            host,
            os: "unknown".into(),
            collection_dir: path.to_path_buf(),
            artifact_root: path.to_path_buf(),
        }],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_collection(dir: &std::path::Path, host: &str) {
        fs::create_dir_all(dir.join("uploads/auto")).unwrap();
        fs::write(dir.join("uploads.json"), "{}").unwrap();
        fs::write(
            dir.join("client_info.json"),
            format!(r#"{{"Hostname":"{host}","Platform":"Microsoft Windows 11 Enterprise","PlatformVersion":"23H2"}}"#),
        ).unwrap();
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
        assert_eq!(hosts[0].os, "Microsoft Windows 11 Enterprise 23H2");
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
