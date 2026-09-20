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

use triage_core::attribution::{sanitize_component, Identity};
use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::duckdb::types::{
    ColumnType, DatasetColumnTypes, DynamicColumnTypes, SqlType, TimeSemantics,
};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};
use triage_registry::hive::Hive;

use batch::{process_entry, HiveSource};
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

/// Declared SQL types for the `batch` dataset. RECmd's batch output is a
/// single static schema, so it is declared the ordinary way; the per-plugin
/// detail files are dynamic and are declared below.
pub const COLUMN_TYPES: &[DatasetColumnTypes] = &[DatasetColumnTypes {
    dataset_id: "batch",
    columns: &[ColumnType {
        column: "LastWriteTimestamp",
        sql_type: SqlType::Timestamp,
        time_semantics: Some(TimeSemantics::Utc),
    }],
}];

/// Declared SQL types for the per-plugin detail files.
///
/// These datasets are named `<Plugin>_<hive stem>` at run time -- and the
/// stem is qualified further when two hives would collide, so `BamDam_SYSTEM`
/// on one host is `BamDam_SYSTEM_RegBack` on another. There is no constant to
/// key on, which is why these are matched by prefix.
///
/// The prefix includes the trailing separator so that `Services_` cannot also
/// claim a future `ServicesHub_SYSTEM`. `TypedURLs_` deliberately covers all
/// four of `TypedURLs_NTUSER.DAT{,_Default,_LocalService,_NetworkService}`,
/// which is the point of prefix matching.
///
/// Every column here was proven to `TRY_CAST` with zero failures against a
/// real collection. Five candidates whose *names* say time were rejected
/// because their values are not timestamps:
///
/// - `ETW_SYSTEM.LastWriteTimestamp` -- `8/31/2022 2:17:27 AM +00:00`, RECmd's
///   US-locale rendering rather than ISO-8601; 455 of 455 rows fail.
/// - `NetworkAdapters_SYSTEM.DriverDate` -- `6-21-2006`, ambiguous M-D-Y.
/// - `Products_SOFTWARE.InstallDate` and `UnInstall_*.InstallDate` --
///   `20260220`, the bare MSI `YYYYMMDD` integer.
/// - `UserAssist_NTUSER.DAT.FocusTime` -- `0d, 0h, 00m, 00s`, a duration, not
///   an instant. Declaring it TIMESTAMP would be wrong even if it parsed.
///
/// Declaring any of those would turn every value in the column into a NULL
/// with only its `__text` companion surviving, which is worse than leaving it
/// VARCHAR for the analyst to `TRY_CAST` deliberately.
pub const DYNAMIC_COLUMN_TYPES: &[DynamicColumnTypes] = &[
    DynamicColumnTypes {
        prefix: "AppCompatCache_",
        columns: &[ColumnType {
            column: "ModifiedTime",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DynamicColumnTypes {
        prefix: "AppPaths_",
        columns: &[ColumnType {
            column: "Timestamp",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DynamicColumnTypes {
        prefix: "BamDam_",
        columns: &[ColumnType {
            column: "ExecutionTime",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DynamicColumnTypes {
        prefix: "DeviceClasses_",
        columns: &[ColumnType {
            column: "Timestamp",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DynamicColumnTypes {
        prefix: "NetworkAdapters_",
        columns: &[ColumnType {
            column: "Timestamp",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DynamicColumnTypes {
        prefix: "Products_",
        columns: &[ColumnType {
            column: "Timestamp",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DynamicColumnTypes {
        prefix: "ProfileList_",
        columns: &[
            ColumnType {
                column: "Timestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "LastLogonTime",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "LastLogoffTime",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
        ],
    },
    DynamicColumnTypes {
        prefix: "RADAR_",
        columns: &[ColumnType {
            column: "LastDetectionTime",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DynamicColumnTypes {
        prefix: "SCSI_",
        columns: &[
            ColumnType {
                column: "Timestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "InitialTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
        ],
    },
    DynamicColumnTypes {
        prefix: "Services_",
        columns: &[
            ColumnType {
                column: "NameKeyLastWrite",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "ParametersKeyLastWrite",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
        ],
    },
    DynamicColumnTypes {
        prefix: "TrustedDocuments_",
        columns: &[ColumnType {
            column: "Timestamp",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DynamicColumnTypes {
        prefix: "TypedURLs_",
        columns: &[ColumnType {
            column: "Timestamp",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DynamicColumnTypes {
        prefix: "UnInstall_",
        columns: &[ColumnType {
            column: "Timestamp",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DynamicColumnTypes {
        prefix: "VolumeInfoCache_",
        columns: &[ColumnType {
            column: "Timestamp",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DynamicColumnTypes {
        prefix: "Windows App_",
        columns: &[ColumnType {
            column: "InstallTime",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DynamicColumnTypes {
        prefix: "WordWheelQuery_",
        columns: &[ColumnType {
            column: "LastWriteTimestamp",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
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

    fn column_types(&self) -> &'static [DatasetColumnTypes] {
        COLUMN_TYPES
    }

    fn dynamic_column_types(&self) -> &'static [DynamicColumnTypes] {
        DYNAMIC_COLUMN_TYPES
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
        // The detail-CSV basename is `<PluginName>_<stem>.csv`, e.g.
        // `BamDam_SYSTEM.csv` — for a hive in its canonical location that is
        // exactly what RECmd writes (fixture-confirmed). `detail_stem` below
        // qualifies the stem when the hive's file name alone would not tell
        // two hives apart in one output directory (`BamDam_SYSTEM_RegBack.csv`),
        // which is a deliberate divergence from RECmd -- RECmd relies on a
        // per-run output directory we do not have. See `detail_stem`.
        //
        // The `PluginDetailFile` column in the `(plugin)` batch rows names
        // this same file, and it is computed from this same value: the stem
        // travels into the batch engine as `HiveSource::detail_stem` rather
        // than being re-derived there, because the two were computed
        // independently once and the column ended up pointing at a file that
        // held another hive's rows.
        let hive_stem = detail_stem(path, out.current_identity());

        // The column names the file the router *routes* that basename to, not
        // the basename: the router folds the identity into a per-user
        // side-car's filename and the Velo layout writes per-user output into
        // a directory of its own, and a reference that ignored either named
        // nothing at all. Snapshotted here, before the write loop borrows the
        // router, and fixed to the identity this hive is attributed to -- the
        // identity does not change within one `parse`.
        let detail_reference = out.side_car_reference();

        for entry in &entries {
            let mut details = Vec::new();
            process_entry(
                &mut hive,
                &HiveSource {
                    path: &hive_path,
                    detail_stem: &hive_stem,
                    detail_reference: &detail_reference,
                },
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

/// The source-unique stem a plugin detail CSV's basename is built from
/// (`<PluginName>_<stem>.csv`).
///
/// RECmd names that file `<PluginName>_<HiveFileName>.csv` and keeps two
/// hives with the same file name apart by writing each run into its own
/// output directory. Neither of our layouts reproduces that for a
/// **system-scope** hive: `Nested` puts every system hive's side-cars in one
/// `system/` directory and `Velo` puts them all at the category root, so
/// `.../config/SYSTEM` and `.../config/RegBack/SYSTEM` claim the same file
/// and append into it with nothing recording which rows came from where.
/// Observed on `Collection-STDC1`: one `Registry/AppCompatCache_SYSTEM.csv`
/// holding the live hive's rows and the RegBack copy's, and one
/// `Registry/TypedURLs_NTUSER.DAT.csv` conflating the `Default`,
/// `LocalService` and `NetworkService` profile hives into three
/// indistinguishable rows.
///
/// The qualifier is the hive's parent directory name, **appended** rather
/// than prefixed so the name still begins with what RECmd would call it
/// (`AppCompatCache_SYSTEM_RegBack.csv`, `TypedURLs_NTUSER.DAT_Default.csv`)
/// -- the same shape the layouts already use to fold an identity into a
/// filename. It is omitted for a hive sitting directly in `config`, the
/// canonical location where the file name is unique by Windows' own
/// convention, so the primary system hives keep RECmd's name exactly.
///
/// A per-user hive needs no qualifier: both layouts already carry the user
/// in the path (`Velo`'s `PerUser/<name>_<user>.csv`, `Nested`'s
/// `users/<user>/`), and two hives attributed to the same user are that one
/// profile's hives.
fn detail_stem(hive_path: &Path, identity: &Identity) -> String {
    let name = hive_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("UNKNOWN");
    if !matches!(identity, Identity::System) {
        return name.to_string();
    }
    let parent = hive_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if parent.is_empty() || parent.eq_ignore_ascii_case("config") {
        return name.to_string();
    }
    // The qualifier becomes part of a filename component, so it goes through
    // the same sanitizer every other path-derived label does.
    format!("{name}_{}", sanitize_component(parent))
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
        use triage_registry::search::{search_subtree, HitType, Matcher, SearchTargets};

        let Some(root) = hive.root() else {
            return Ok(0);
        };

        let mut count = 0u64;

        // Run a separate subtree pass per search flag so each flag uses its own
        // needle. Flags are additive: all matching hits are emitted.
        let passes: &[(&Option<String>, SearchTargets)] = &[
            (&self.search_key, SearchTargets::keys()),
            (&self.search_value, SearchTargets::value_names()),
            (&self.search_data, SearchTargets::value_data()),
        ];

        for (term_opt, targets) in passes {
            let Some(needle) = term_opt.as_deref() else {
                continue;
            };
            let matcher = Matcher::new(needle, self.use_regex, self.literal);

            // Each pass needs a fresh root handle.
            let Some(root_node) = hive.root() else {
                continue;
            };
            let hits = search_subtree(hive, root_node, &matcher, *targets, self.min_size);

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

#[cfg(test)]
mod detail_stem_tests {
    use super::*;

    /// The three real collisions observed on `Collection-STDC1`, plus the
    /// cases that must keep RECmd's name unchanged.
    #[test]
    fn a_system_scope_hive_is_qualified_only_when_its_file_name_is_not_unique() {
        let system = Identity::System;
        // Canonical location: RECmd's name, unchanged.
        assert_eq!(
            detail_stem(Path::new("/c/Windows/System32/config/SYSTEM"), &system),
            "SYSTEM"
        );
        assert_eq!(
            detail_stem(Path::new("/c/Windows/System32/config/SOFTWARE"), &system),
            "SOFTWARE"
        );
        // The RegBack copy, which used to append into the live hive's file.
        assert_eq!(
            detail_stem(
                Path::new("/c/Windows/System32/config/RegBack/SYSTEM"),
                &system
            ),
            "SYSTEM_RegBack"
        );
        // The three system-scope profile hives, which used to conflate.
        for (path, expected) in [
            ("/c/Users/Default/NTUSER.DAT", "NTUSER.DAT_Default"),
            (
                "/c/Windows/ServiceProfiles/LocalService/NTUSER.DAT",
                "NTUSER.DAT_LocalService",
            ),
            (
                "/c/Windows/ServiceProfiles/NetworkService/NTUSER.DAT",
                "NTUSER.DAT_NetworkService",
            ),
        ] {
            assert_eq!(detail_stem(Path::new(path), &system), expected);
        }
    }

    /// A per-user hive keeps the bare name: the layout already carries the
    /// user, so qualifying here would only duplicate it.
    #[test]
    fn a_per_user_hive_keeps_the_bare_recmd_name() {
        assert_eq!(
            detail_stem(
                Path::new("/c/Users/alice/NTUSER.DAT"),
                &Identity::User("alice".into())
            ),
            "NTUSER.DAT"
        );
        assert_eq!(
            detail_stem(
                Path::new("/c/Users/alice/AppData/Local/Microsoft/Windows/UsrClass.dat"),
                &Identity::User("alice".into())
            ),
            "UsrClass.dat"
        );
    }

    /// Total over the paths a capture can hand it: a path with no file name
    /// and no parent must produce a value, not a panic.
    #[test]
    fn it_is_total_over_degenerate_paths() {
        for path in ["", "/", "..", "SYSTEM"] {
            let stem = detail_stem(Path::new(path), &Identity::System);
            assert!(!stem.is_empty(), "empty stem for {path:?}");
        }
    }
}
