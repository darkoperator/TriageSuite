//! The AppCompatCacheParser row — one per cache entry. Field order IS the
//! 7-column CSV order. All columns are String (empty = absent).

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AppCompatRecord {
    #[serde(rename = "ControlSet")]
    pub control_set: String,
    #[serde(rename = "CacheEntryPosition")]
    pub cache_entry_position: String,
    #[serde(rename = "Path")]
    pub path: String,
    #[serde(rename = "LastModifiedTimeUTC")]
    pub last_modified: String,
    #[serde(rename = "Executed")]
    pub executed: String,
    #[serde(rename = "Duplicate")]
    pub duplicate: String,
    #[serde(rename = "SourceFile")]
    pub source_file: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_matches_appcompatcacheparser() {
        let mut w = csv::Writer::from_writer(vec![]);
        w.serialize(AppCompatRecord {
            control_set: "1".into(),
            cache_entry_position: "0".into(),
            path: r"C:\x.exe".into(),
            last_modified: String::new(),
            executed: "No".into(),
            duplicate: "False".into(),
            source_file: "SYSTEM".into(),
        })
        .unwrap();
        let out = String::from_utf8(w.into_inner().unwrap()).unwrap();
        assert_eq!(
            out.lines().next().unwrap(),
            "ControlSet,CacheEntryPosition,Path,LastModifiedTimeUTC,Executed,Duplicate,SourceFile"
        );
    }
}
