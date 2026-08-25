//! `Hive`: a notatin-backed registry hive opened with transaction-log replay
//! and deleted-record recovery. Wraps notatin's `Parser`, exposing the
//! traversal primitives RETriage's batch engine needs.

use crate::hivetype::HiveType;
use notatin::cell_key_node::CellKeyNode;
use notatin::parser::Parser;
use notatin::parser_builder::ParserBuilder;
use std::path::{Path, PathBuf};

/// An opened registry hive plus its detected type.
pub struct Hive {
    parser: Parser,
    hive_type: HiveType,
    hive_name: String,
}

/// Error opening or reading a hive.
#[derive(Debug)]
pub enum HiveError {
    Open(String),
}

impl std::fmt::Display for HiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HiveError::Open(m) => write!(f, "hive open error: {m}"),
        }
    }
}

impl std::error::Error for HiveError {}

impl Hive {
    /// Open `primary`, replaying each transaction log in `logs` and recovering
    /// deleted records. Hive type is detected from the primary's file name.
    /// `logs` is an ordered slice of `.LOG1`/`.LOG2` siblings (LOG1 then LOG2).
    pub fn open(
        primary: &Path,
        logs: &[PathBuf],
        recover_deleted: bool,
    ) -> Result<Hive, HiveError> {
        // `from_path` requires `P: 'static`, so a borrowed `&Path` does not satisfy
        // the bound; an owned `PathBuf` is required — `.to_path_buf()` is not redundant.
        #[allow(clippy::unnecessary_to_owned)]
        let mut builder = ParserBuilder::from_path(primary.to_path_buf());
        builder.recover_deleted(recover_deleted);
        for log in logs {
            builder.with_transaction_log(log.clone());
        }
        let parser = builder
            .build()
            .map_err(|e| HiveError::Open(format!("{e}")))?;
        let hive_name = primary
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let hive_type = HiveType::from_filename(&hive_name);
        Ok(Hive {
            parser,
            hive_type,
            hive_name,
        })
    }

    pub fn hive_type(&self) -> HiveType {
        self.hive_type
    }

    pub fn hive_name(&self) -> &str {
        &self.hive_name
    }

    /// The root key, if present.
    pub fn root(&mut self) -> Option<CellKeyNode> {
        self.parser.get_root_key().ok().flatten()
    }

    /// Resolve a key by path **without** the root key name (RECmd `KeyPath`
    /// form, e.g. `Microsoft\Windows\CurrentVersion\Run`).
    pub fn get_key(&mut self, key_path: &str) -> Option<CellKeyNode> {
        self.parser.get_key(key_path, false).ok().flatten()
    }

    /// Direct subkeys of `key`.
    pub fn sub_keys(&mut self, key: &mut CellKeyNode) -> Vec<CellKeyNode> {
        key.read_sub_keys(&mut self.parser)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locate a SOFTWARE hive in the read-only captures, if present.
    fn find_software_hive() -> Option<(PathBuf, Vec<PathBuf>)> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
        if !root.exists() {
            return None;
        }
        for entry in walkdir_min(&root) {
            if entry.file_name().and_then(|s| s.to_str()) == Some("SOFTWARE") {
                let dir = entry.parent().unwrap();
                let mut logs = Vec::new();
                for ext in ["SOFTWARE.LOG1", "SOFTWARE.LOG2"] {
                    let p = dir.join(ext);
                    if p.exists() {
                        logs.push(p);
                    }
                }
                return Some((entry, logs));
            }
        }
        None
    }

    // Tiny recursive file walk to avoid pulling walkdir into this crate's deps.
    fn walkdir_min(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
        out
    }

    #[test]
    fn opens_software_hive_and_reads_root() {
        let Some((primary, logs)) = find_software_hive() else {
            eprintln!("SKIP: no SOFTWARE hive in test captures");
            return;
        };
        let mut hive = Hive::open(&primary, &logs, true).expect("open");
        assert_eq!(hive.hive_type(), HiveType::Software);
        let root = hive.root().expect("root key");
        let mut root = root;
        let subs = hive.sub_keys(&mut root);
        assert!(!subs.is_empty(), "SOFTWARE root should have subkeys");
        assert!(subs
            .iter()
            .any(|k| k.key_name.eq_ignore_ascii_case("Microsoft")));
    }
}
