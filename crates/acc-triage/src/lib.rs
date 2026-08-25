//! AppCompatTriage: AppCompatCacheParser-compatible AppCompatCache/ShimCache parser.
//! Enumerates ControlSet000..009 in a SYSTEM hive, parses each Win10 cache, and
//! marks duplicates across sets in order.

pub mod cli;
pub mod record;

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use triage_appcompat::{is_year_1601, parse_win10};
use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_core::tool::{Scope, Tool};
use triage_registry::hive::Hive;

use record::AppCompatRecord;

const REGF_MAGIC: [u8; 4] = [0x72, 0x65, 0x67, 0x66];

pub const DATASETS: &[DatasetSpec] = &[DatasetSpec {
    id: "appcompat",
    default_basename: "AppCompatTriage_AppCompatCache_Output",
    framing: JsonFraming::Ndjson,
    csv_only: false,
    override_suffix: None,
}];

#[derive(Default)]
pub struct AppCompatTool {
    pub no_logs: bool,
}

/// Render a UTC datetime as the suite ISO-8601 timestamp.
fn ts_string(dt: DateTime<Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

/// Dedup key = "{filetime}{PATH_UPPER}" (CacheEntry.GetKey), filetime=0 when null.
fn dedup_key(ft_for_key: i64, path: &str) -> String {
    format!("{ft_for_key}{}", path.to_uppercase())
}

/// Mark duplicates over rows in emission order: for each key in order, a row is a
/// duplicate iff its key was already seen earlier. First occurrence => false.
/// Mirrors AppCompatCacheParser's shared cacheKeys HashSet across control sets.
fn assign_duplicates(keys: &[String]) -> Vec<bool> {
    let mut seen = std::collections::HashSet::new();
    keys.iter()
        .map(|k| {
            let dup = seen.contains(k);
            seen.insert(k.clone());
            dup
        })
        .collect()
}

impl Tool for AppCompatTool {
    fn binary_name(&self) -> &'static str {
        "AppCompatTriage"
    }
    fn patterns(&self) -> &[&'static str] {
        &["SYSTEM"]
    }
    fn validate_legacy(&self, path: &Path) -> bool {
        let name_upper = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_ascii_uppercase())
            .unwrap_or_default();
        if name_upper.ends_with(".LOG")
            || name_upper.ends_with(".LOG1")
            || name_upper.ends_with(".LOG2")
        {
            return false;
        }
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        use std::io::Read;
        let mut buf = [0u8; 4];
        matches!(f.read_exact(&mut buf), Ok(())) && buf == REGF_MAGIC
    }
    fn invalid_content_is_corrupt(&self) -> bool {
        true
    }

    fn datasets(&self) -> &'static [DatasetSpec] {
        DATASETS
    }
    fn scope(&self) -> Scope {
        Scope::SystemWide
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        let logs: Vec<PathBuf> = if self.no_logs {
            Vec::new()
        } else {
            find_log_siblings(path)
        };
        let mut hive = Hive::open(path, &logs, true).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let source_file = path.to_string_lossy().into_owned();

        // 1. Collect raw AppCompatCache blobs for each present ControlSet000..009 (ascending).
        let mut blobs: Vec<(u32, Vec<u8>)> = Vec::new();
        for i in 0u32..=9 {
            let key_path = format!(r"ControlSet00{i}\Control\Session Manager\AppCompatCache");
            let Some(key) = hive.get_key(&key_path) else {
                continue;
            };
            let raw = key
                .value_iter()
                .find(|v| v.get_pretty_name().eq_ignore_ascii_case("AppCompatCache"))
                .map(|v| {
                    let (content, _) = v.get_content();
                    triage_registry::value::plugin_raw_bytes(&content)
                })
                .unwrap_or_default();
            if !raw.is_empty() {
                blobs.push((i, raw));
            }
        }

        // 2. Parse each control set; build rows in order, computing the dedup key.
        struct Pending {
            control_set: u32,
            position: usize,
            path: String,
            last_modified: String,
            executed: bool,
            key: String,
        }
        let mut pending: Vec<Pending> = Vec::new();
        for (cs, raw) in &blobs {
            for e in parse_win10(raw) {
                // AppCompatCacheParser strips \??\ from the path.
                let path = e.path.replace(r"\??\", "");
                // Null the timestamp when 0 or year 1601.
                let mtime = e.modified_time.filter(|dt| !is_year_1601(dt));
                let ft_for_key = if mtime.is_some() {
                    e.filetime as i64
                } else {
                    0
                };
                let last_modified = mtime.map(ts_string).unwrap_or_default();
                let key = dedup_key(ft_for_key, &path);
                pending.push(Pending {
                    control_set: *cs,
                    position: e.position,
                    path,
                    last_modified,
                    executed: e.executed,
                    key,
                });
            }
        }

        // 3. Duplicate pass across all rows in order; emit.
        let keys: Vec<String> = pending.iter().map(|p| p.key.clone()).collect();
        let flags = assign_duplicates(&keys);
        let mut count = 0u64;
        for (p, duplicate) in pending.into_iter().zip(flags) {
            let rec = AppCompatRecord {
                control_set: p.control_set.to_string(),
                cache_entry_position: p.position.to_string(),
                path: p.path,
                last_modified: p.last_modified,
                executed: if p.executed {
                    "Yes".into()
                } else {
                    "No".into()
                },
                duplicate: if duplicate {
                    "True".into()
                } else {
                    "False".into()
                },
                source_file: source_file.clone(),
            };
            out.write("appcompat", &rec)?;
            count += 1;
        }
        Ok(count)
    }
}

/// `.LOG1`/`.LOG2` siblings (LOG1 before LOG2). Copied from sbe-triage.
fn find_log_siblings(primary: &Path) -> Vec<PathBuf> {
    let Some(dir) = primary.parent() else {
        return Vec::new();
    };
    let Some(stem) = primary.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    let stem_lower = stem.to_ascii_lowercase();
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    let mut logs = Vec::new();
    for ext in [".log1", ".log2"] {
        let target = format!("{stem_lower}{ext}");
        if let Some(found) = entries.iter().find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_ascii_lowercase() == target)
                .unwrap_or(false)
        }) {
            logs.push(found.clone());
        }
    }
    logs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_key_uppercases_path_and_uses_filetime() {
        assert_eq!(dedup_key(0, r"c:\Foo.exe"), r"0C:\FOO.EXE");
        assert_eq!(dedup_key(132699, r"c:\bar.exe"), r"132699C:\BAR.EXE");
    }

    #[test]
    fn datasets_single_primary() {
        assert_eq!(DATASETS.len(), 1);
        assert!(DATASETS[0].override_suffix.is_none());
    }

    #[test]
    fn validate_rejects_logs() {
        let t = AppCompatTool::default();
        assert!(!t.validate_legacy(Path::new("/x/SYSTEM.LOG1")));
    }

    #[test]
    fn assign_duplicates_marks_repeats_in_order() {
        let keys: Vec<String> = ["a", "b", "a", "c", "b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            assign_duplicates(&keys),
            vec![false, false, true, false, true]
        );
    }

    #[test]
    fn assign_duplicates_empty_input_is_empty() {
        let keys: Vec<String> = Vec::new();
        assert_eq!(assign_duplicates(&keys), Vec::<bool>::new());
    }

    #[test]
    fn assign_duplicates_collides_on_case_insensitive_dedup_key() {
        let k1 = dedup_key(100, r"c:\x.exe");
        let k2 = dedup_key(100, r"C:\X.EXE");
        let keys = vec![k1, k2];
        assert_eq!(assign_duplicates(&keys), vec![false, true]);
    }
}
