//! SBETriage: SBECmd-compatible Windows shellbags parser. Walks BagMRU/Bags in
//! NTUSER.DAT and UsrClass.dat, reconstructing each shellbag's path + timestamps.

pub mod bags;
pub mod cli;
pub mod record;
pub mod shelltype;
pub mod walk;

use std::path::{Path, PathBuf};

use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};
use triage_registry::hive::Hive;

use bags::explored_node_slots;
use walk::walk;

/// The `regf` magic bytes at the start of every Windows registry hive.
const REGF_MAGIC: [u8; 4] = [0x72, 0x65, 0x67, 0x66]; // b"regf"

pub const DATASETS: &[DatasetSpec] = &[DatasetSpec {
    id: "shellbags",
    default_basename: "SBETriage_Shellbags_Output",
    framing: JsonFraming::Ndjson,
    csv_only: false,
    override_suffix: None,
}];

/// SBETriage tool: discovers NTUSER.DAT / UsrClass.dat hives, walks
/// BagMRU/Bags, and emits ShellbagRecords through the shared runner.
#[derive(Default)]
pub struct ShellbagTool {
    /// Skip pairing the primary hive with `.LOG1`/`.LOG2` siblings.
    pub no_logs: bool,
}

impl Tool for ShellbagTool {
    fn binary_name(&self) -> &'static str {
        "SBETriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &["NTUSER.DAT", "UsrClass.dat"]
    }

    /// Content check: first 4 bytes == `regf`, AND the file name must not end
    /// in `.LOG`, `.LOG1`, or `.LOG2` (case-insensitive) — those are transaction
    /// log siblings, not primary hives.
    fn validate_legacy(&self, path: &Path) -> bool {
        // Reject transaction log siblings by name suffix.
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
        // Content-based: must start with the `regf` magic.
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
        Scope::UserSpecific
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        // Pair the primary hive with its transaction log siblings.
        let logs: Vec<PathBuf> = if self.no_logs {
            Vec::new()
        } else {
            find_log_siblings(path)
        };

        // recover=true mirrors SBECmd's default (replays transaction logs and
        // recovers dirty-hive state).
        let mut hive = Hive::open(path, &logs, true).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        // Determine BagMRU + Bags root paths based on the hive file name.
        let name_lower = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_ascii_lowercase())
            .unwrap_or_default();

        // UsrClass.dat supports HasExplored=True (SBECmd reads Bags\<n>\Shell GUID
        // subkeys). NTUSER.DAT never has HasExplored=True in SBECmd's output (verified
        // across all NTUSER fixtures); we pass an empty explored set for those roots so
        // HasExplored is always False for NTUSER items.
        let is_usrclass = name_lower == "usrclass.dat";

        let roots: &[(&str, &str)] = if is_usrclass {
            &[(
                r"Local Settings\Software\Microsoft\Windows\Shell\BagMRU",
                r"Local Settings\Software\Microsoft\Windows\Shell\Bags",
            )]
        } else {
            // NTUSER.DAT: try both Shell and ShellNoRoam.
            &[
                (
                    r"Software\Microsoft\Windows\Shell\BagMRU",
                    r"Software\Microsoft\Windows\Shell\Bags",
                ),
                (
                    r"Software\Microsoft\Windows\ShellNoRoam\BagMRU",
                    r"Software\Microsoft\Windows\ShellNoRoam\Bags",
                ),
            ]
        };

        let mut count = 0u64;

        for (bagmru_path, bags_path) in roots {
            // Build the explored set BEFORE walking (parser-state rule).
            // For UsrClass.dat: build from Bags\<n>\Shell GUID subkeys.
            // For NTUSER.DAT: always empty — SBECmd never emits HasExplored=True
            // for NTUSER items (verified across all NTUSER reference fixtures).
            let explored = if is_usrclass {
                explored_node_slots(&mut hive, bags_path)
            } else {
                std::collections::HashSet::new()
            };

            let Some(root_key) = hive.get_key(bagmru_path) else {
                continue;
            };

            let mut records = Vec::new();
            walk(&mut hive, root_key, "BagMRU", "", &explored, &mut records);

            for record in records {
                out.write("shellbags", &record)?;
                count += 1;
            }
        }

        Ok(count)
    }
}

/// Return existing `.LOG1` and `.LOG2` siblings for `primary` in the same
/// directory, in LOG1-before-LOG2 order.
fn find_log_siblings(primary: &Path) -> Vec<PathBuf> {
    let Some(dir) = primary.parent() else {
        return Vec::new();
    };
    let Some(stem) = primary.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    let stem_lower = stem.to_ascii_lowercase();

    // Collect all directory entries once.
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
