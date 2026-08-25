//! RETriage: RECmd-compatible registry triage. Discovers hives, pairs them with
//! transaction logs, runs the embedded DFIRBatch.reb batch + plugins, and emits
//! BatchCsvOut rows through the shared runner.

pub mod batch;
pub mod cli;
pub mod plugins;
pub mod reb;
pub mod search_record;

#[cfg(test)]
pub(crate) mod testsupport;

use std::path::{Path, PathBuf};

use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};
use triage_registry::hive::Hive;

use batch::process_entry;
use plugins::registry;
use reb::{parse_reb, DFIR_BATCH};

/// The `regf` magic bytes at the start of every Windows registry hive.
const REGF_MAGIC: [u8; 4] = [0x72, 0x65, 0x67, 0x66]; // b"regf"

pub const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        id: "batch",
        default_basename: "RETriage_Batch_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    },
    DatasetSpec {
        id: "search",
        default_basename: "RETriage_Search_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    },
];

/// RETriage tool: discovers registry hives and runs the DFIRBatch.reb batch
/// (plus any custom `.reb` file) through the batch engine.
pub struct RegistryTool {
    /// Skip pairing the primary hive with `.LOG1`/`.LOG2` siblings.
    pub no_logs: bool,
    /// Recover deleted registry records during hive open.
    pub recover_deleted: bool,
    /// Optional custom batch file path (overrides the embedded DFIRBatch.reb).
    pub batch_file: Option<PathBuf>,
    /// Disable all plugin dispatch (mirrors RECmd global DisablePlugin flag).
    /// When true, every key takes the default dump path regardless of plugin
    /// matches. Used by `*__batch_noplugins.csv` fixture tests as an engine
    /// regression guard.
    pub no_plugins: bool,
    // ── Search mode (mutually exclusive with batch when any is Some) ──────────
    pub search_key: Option<String>,
    pub search_value: Option<String>,
    pub search_data: Option<String>,
    pub use_regex: bool,
    pub literal: bool,
    pub min_size: usize,
}

impl Default for RegistryTool {
    fn default() -> Self {
        RegistryTool {
            no_logs: false,
            recover_deleted: true,
            batch_file: None,
            no_plugins: false,
            search_key: None,
            search_value: None,
            search_data: None,
            use_regex: false,
            literal: false,
            min_size: 0,
        }
    }
}

impl Tool for RegistryTool {
    fn binary_name(&self) -> &'static str {
        "RETriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &[
            "NTUSER.DAT",
            "UsrClass.dat",
            "SOFTWARE",
            "SYSTEM",
            "SAM",
            "SECURITY",
            "DEFAULT",
            "Amcache.hve",
        ]
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
        Scope::UserElseSystem
    }

    fn resource_class(&self) -> triage_core::tool::ResourceClass {
        triage_core::tool::ResourceClass::Heavy
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        // Pair the primary hive with its transaction log siblings.
        let logs: Vec<PathBuf> = if self.no_logs {
            Vec::new()
        } else {
            find_log_siblings(path)
        };

        let mut hive =
            Hive::open(path, &logs, self.recover_deleted).map_err(|e| TriageError::Artifact {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;

        let hive_path = path.display().to_string();

        // Route to search mode when any search flag is set.
        let search_mode =
            self.search_key.is_some() || self.search_value.is_some() || self.search_data.is_some();
        if search_mode {
            return self.run_search(&mut hive, &hive_path, out);
        }

        // Load the batch entries.
        let batch_yaml = match &self.batch_file {
            Some(p) => std::fs::read_to_string(p).map_err(|e| TriageError::Artifact {
                path: p.clone(),
                message: e.to_string(),
            })?,
            None => DFIR_BATCH.to_string(),
        };
        let entries = parse_reb(&batch_yaml).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: format!("batch parse error: {e}"),
        })?;

        let mut count = 0u64;

        // Build the plugin registry once per parse call (empty when --no-plugins).
        let plugin_registry: Vec<Box<dyn triage_registry::plugin::RegistryPlugin>> =
            if self.no_plugins {
                Vec::new()
            } else {
                registry()
            };

        // ── Per-plugin detail-CSV writer ─────────────────────────────────────
        //
        // Plugin detail rows have dynamic, plugin-specific schemas (different
        // columns per plugin). The standard OutputRouter requires static serde
        // records, so it cannot express dynamic schemas cleanly. We use a
        // dedicated detail writer keyed by (plugin_name, hive_stem) that
        // co-locates its files in the same identity directory as the batch CSV.
        //
        // Design decision (Task 9): dedicated detail writer, not router.
        // Rationale: plugin columns are `Vec<(String,String)>` — dynamic at
        // runtime, not statically known. Adding 33+ DatasetSpecs (one per plugin
        // per hive) to the router would require static lifetime string slices
        // and special-case DatasetSpec construction. A lightweight dedicated
        // writer using `triage_core::output::layout::OutputLayout` is simpler,
        // reuses the same directory layout as the router, and doesn't touch the
        // router's public API beyond the two new accessor methods added to it.
        //
        // The detail-CSV basename is `<PluginName>_<HiveStem>.csv`, e.g.
        // `BamDam_SYSTEM.csv` — exactly what RECmd writes (fixture-confirmed).
        // The `PluginDetailFile` column in the (plugin) batch rows is set to
        // this basename so consumers can locate the side-car file.
        // Hive stem (e.g. "SYSTEM" from "SYSTEM.LOG1"-stripped path).
        let hive_stem = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("UNKNOWN")
            .to_string();

        for entry in &entries {
            let mut details = Vec::new();
            process_entry(
                &mut hive,
                &hive_path,
                entry,
                &plugin_registry,
                &mut |record| {
                    out.write("batch", &record)?;
                    count += 1;
                    Ok(())
                },
                &mut |plugin_name, row| {
                    details.push((plugin_name.to_string(), row));
                    Ok(())
                },
            )?;
            for (plugin_name, row) in details {
                if row.detail_columns.is_empty() {
                    continue;
                }
                let basename = format!("{plugin_name}_{hive_stem}");
                let headers: Vec<String> =
                    row.detail_columns.iter().map(|(k, _)| k.clone()).collect();
                let values: Vec<String> = row.detail_columns.into_iter().map(|(_, v)| v).collect();
                out.write_dynamic_csv_row(&basename, &headers, &values)?;
            }
        }

        Ok(count)
    }
}

impl RegistryTool {
    /// Run registry search mode. Called when any of --sk/--sv/--sd is set.
    ///
    /// AcceptedDelta: RECmd search mode does NOT write CSV output — it writes
    /// only to console/log. RETriage search CSV is therefore a pure RETriage
    /// extension with no RECmd reference schema to compare against.
    fn run_search(
        &self,
        hive: &mut Hive,
        hive_path: &str,
        out: &mut OutputRouter,
    ) -> Result<u64, TriageError> {
        use crate::search_record::SearchRecord;
        use triage_core::timestamp::WinTimestamp;
        use triage_registry::search::{search_subtree, HitType, Matcher};

        let Some(root) = hive.root() else {
            return Ok(0);
        };

        let mut count = 0u64;

        // Run a separate subtree pass per search flag so each flag uses its own
        // needle. Flags are additive: all matching hits are emitted.
        let passes: &[(&Option<String>, bool, bool, bool)] = &[
            (&self.search_key, true, false, false),
            (&self.search_value, false, true, false),
            (&self.search_data, false, false, true),
        ];

        for (term_opt, sk, sv, sd) in passes {
            let Some(needle) = term_opt.as_deref() else {
                continue;
            };
            let matcher = Matcher::new(needle, self.use_regex, self.literal);

            // Each pass needs a fresh root handle.
            let Some(root_node) = hive.root() else {
                continue;
            };
            let hits = search_subtree(hive, root_node, &matcher, *sk, *sv, *sd, self.min_size);

            for hit in hits {
                let lw = hit
                    .last_write
                    .map(|dt| {
                        WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos())
                            .to_string()
                    })
                    .unwrap_or_default();

                let hit_type_str = match hit.hit_type {
                    HitType::KeyName => "KeyName",
                    HitType::ValueName => "ValueName",
                    HitType::ValueData => "ValueData",
                    HitType::ValueSlack => "ValueSlack",
                };

                let record = SearchRecord {
                    hive_path: hive_path.to_string(),
                    hit_type: hit_type_str.to_string(),
                    key_path: hit.key_path,
                    value_name: hit.value_name,
                    value_data: hit.value_data,
                    deleted: hit.deleted,
                    last_write_timestamp: lw,
                };
                out.write("search", &record)?;
                count += 1;
            }
        }

        // Suppress unused variable warning — root was consumed by the first pass
        // or early-returned. Keep a reference to silence the lint.
        let _ = root;

        Ok(count)
    }
}

/// Return existing `.LOG1` and `.LOG2` siblings for `primary` in the same
/// directory, in LOG1-before-LOG2 order.
///
/// Case-insensitive: on evidence captured from Windows, the primary hive may
/// be `NTUSER.DAT` (uppercase) while the logs are `ntuser.dat.LOG1` (lowercase).
/// macOS's HFS+ / APFS can be case-sensitive, so a direct `join(stem + ext)`
/// would fail. We scan the directory entries and match by case-insensitive name.
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
