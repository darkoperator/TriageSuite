//! Turning a run's published files into an inventory.
//!
//! Everything here is a fact the run already established -- what the router
//! published, what the Velo merge consumed, what a file's own header says --
//! never something re-derived from the shape of a path.

use crate::attribution::Identity;
use crate::output::duckdb::header::{duckdb_names, read_header};
use crate::output::duckdb::inventory::{
    Dataset, DatasetFile, DroppedOverride, EffectiveType, FileIdentity, FileRole, Inventory,
    InventoryOnly, MetadataColumns, Status,
};
use crate::output::duckdb::sql::{view_name, NameAllocator};
use crate::output::duckdb::types::{ColumnType, DatasetColumnTypes, DynamicColumnTypes};
use crate::output::published::{DatasetKey, OutputFormat, PublishedFile};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A Velo category file and the per-user slices a *successful* merge
/// consumed to build it.
#[derive(Debug, Clone)]
pub struct MergedFile {
    pub merged_path: PathBuf,
    pub source_paths: Vec<PathBuf>,
    /// The format the merge wrote, which is the format of the slices it
    /// consumed. Carried for the same reason `PublishedFile` carries one:
    /// the Velo merge post-pass runs for NDJSON output as well as CSV, and
    /// a merged NDJSON file handed to `read_csv` is a view over a file that
    /// is not a CSV. Never inferred from the extension -- the merge knows
    /// which writer it used.
    pub format: OutputFormat,
}

#[derive(Debug, Clone)]
pub struct ToolOutputs {
    pub binary_name: String,
    pub published: Vec<PublishedFile>,
    pub merged: Vec<MergedFile>,
    pub column_types: &'static [DatasetColumnTypes],
    pub dynamic_column_types: &'static [DynamicColumnTypes],
}

#[derive(Debug, Clone)]
pub struct ExternalOutputs {
    pub tool: String,
    pub output_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct HostOutputs {
    pub host: String,
    pub tools: Vec<ToolOutputs>,
    pub external: Vec<ExternalOutputs>,
}

/// Bundled because `clippy.toml` caps a function at 6 arguments.
pub struct BuildRequest<'a> {
    pub run_id: &'a str,
    pub generation: &'a str,
    pub generated_utc: &'a str,
    pub out_root: &'a Path,
    pub hosts: &'a [HostOutputs],
}

/// Column names a dataset may use to name the *evidence* path, as opposed
/// to the output path this layer injects. Recognized, never invented: a
/// dataset with none gets `None`.
const EVIDENCE_PATH_COLUMNS: &[&str] = &["SourceFile", "SourceName", "SourcePath"];

/// Build the inventory for one run.
pub fn build(request: BuildRequest<'_>) -> Inventory {
    let mut inv = Inventory::empty(
        request.run_id,
        request.generation,
        request.generated_utc,
        request.out_root,
        Status::Ok,
    );

    // (tool, dataset id) -> the files that belong to it, in host order.
    let mut grouped: BTreeMap<(String, String), Vec<StagedFile>> = BTreeMap::new();
    let mut types_for: BTreeMap<(String, String), Vec<&'static [ColumnType]>> = BTreeMap::new();

    for host in request.hosts {
        for tool in &host.tools {
            // Sources of a successful merge are folded into the merged file
            // and must not be scanned alongside it.
            let mut consumed: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
            for merge in &tool.merged {
                for source in &merge.source_paths {
                    consumed.insert(source.clone(), merge.merged_path.clone());
                }
            }
            // A merge output is also a published file for the tools that
            // publish the category file itself. It is staged once, below,
            // as the merged file; staging it here as well would put two
            // scans of one path in the same view and double every row.
            //
            // Only the CSV merges: a non-CSV merge output is inventoried
            // below rather than scanned, so it must not suppress anything
            // here either.
            let merged_paths: BTreeSet<PathBuf> = tool
                .merged
                .iter()
                .filter(|m| m.format == OutputFormat::Csv)
                .map(|m| m.merged_path.clone())
                .collect();

            for file in &tool.published {
                let dataset_id = file.dataset.as_str().to_string();
                if file.format == OutputFormat::Json {
                    inv.inventory_only.push(InventoryOnly {
                        tool: tool.binary_name.clone(),
                        dataset_id,
                        format: "json".into(),
                        path: relative(request.out_root, &file.path),
                        reason: "json-not-viewed".into(),
                    });
                    continue;
                }
                // The declared types below are still registered for a merge
                // output: the merged file is the one that gets the view, and
                // dropping its dataset's overrides here would leave it
                // untyped.
                if !merged_paths.contains(&file.path) {
                    let role = match consumed.get(&file.path) {
                        Some(_) => FileRole::Slice,
                        None => FileRole::Primary,
                    };
                    grouped
                        .entry((tool.binary_name.clone(), dataset_id.clone()))
                        .or_default()
                        .push(StagedFile {
                            path: file.path.clone(),
                            host: host.host.clone(),
                            identity: Some(identity_of(&file.identity)),
                            role,
                            derived_into: consumed.get(&file.path).cloned(),
                        });
                }
                let declared: Vec<&'static [ColumnType]> = match &file.dataset {
                    DatasetKey::Static(id) => tool
                        .column_types
                        .iter()
                        .filter(|d| d.dataset_id == *id)
                        .map(|d| d.columns)
                        .collect(),
                    // A dynamic id carries the evidence in its name
                    // (`Services_SYSTEM`, `Individual/Security`), so it is
                    // matched by prefix. Longest wins and nothing else is
                    // applied: two declarations that both match are a
                    // specific one and a general one, not two halves of a
                    // schema, and merging them would let the general one
                    // contradict the specific one it exists to refine.
                    DatasetKey::Dynamic(name) => {
                        dynamic_columns_for(tool.dynamic_column_types, name)
                            .into_iter()
                            .collect()
                    }
                };
                {
                    if !declared.is_empty() {
                        // Recorded once per (tool, dataset), not once per
                        // file: the declaration is a compile-time constant
                        // and every host repeats the same one. Extending
                        // would replay it, dropping the same override twice
                        // and allocating a second `__text` companion that
                        // shadows the first.
                        types_for
                            .entry((tool.binary_name.clone(), dataset_id.clone()))
                            .or_insert(declared);
                    }
                }
            }

            // The merged file itself, attributed to no single identity. One
            // entry per path: two merges naming the same output are two
            // records of one file, not two files.
            let mut staged_merged: BTreeSet<PathBuf> = BTreeSet::new();
            for merge in &tool.merged {
                if !staged_merged.insert(merge.merged_path.clone()) {
                    continue;
                }
                let dataset_id = dataset_id_for_merged(tool, &merge.merged_path);
                // A `--json` run merges per-user NDJSON slices exactly as a
                // `--csv` run merges CSV ones. The result is inventoried and
                // not viewed, the same answer a published JSON file gets
                // above; scanning it with `read_csv` would build a view over
                // a file that is not a CSV.
                if merge.format != OutputFormat::Csv {
                    inv.inventory_only.push(InventoryOnly {
                        tool: tool.binary_name.clone(),
                        dataset_id,
                        format: "json".into(),
                        path: relative(request.out_root, &merge.merged_path),
                        reason: "json-not-viewed".into(),
                    });
                    continue;
                }
                grouped
                    .entry((tool.binary_name.clone(), dataset_id))
                    .or_default()
                    .push(StagedFile {
                        path: merge.merged_path.clone(),
                        host: host.host.clone(),
                        identity: None,
                        role: FileRole::Merged,
                        derived_into: None,
                    });
            }
        }

        for external in &host.external {
            for path in &external.output_paths {
                if path.extension().and_then(|e| e.to_str()) != Some("csv") {
                    inv.inventory_only.push(InventoryOnly {
                        tool: external.tool.clone(),
                        dataset_id: stem_of(path),
                        format: extension_of(path),
                        path: relative(request.out_root, path),
                        reason: "external-not-csv".into(),
                    });
                    continue;
                }
                grouped
                    .entry((format!("ext_{}", external.tool), stem_of(path)))
                    .or_default()
                    .push(StagedFile {
                        path: path.clone(),
                        host: host.host.clone(),
                        identity: None,
                        role: FileRole::Primary,
                        derived_into: None,
                    });
            }
        }
    }

    let mut view_names = NameAllocator::new(Vec::<String>::new());
    let mut datasets = Vec::new();
    let mut dropped = Vec::new();
    for ((tool, dataset_id), staged) in grouped {
        let declared = types_for
            .get(&(tool.clone(), dataset_id.clone()))
            .cloned()
            .unwrap_or_default();
        let mut sinks = Sinks {
            view_names: &mut view_names,
            dropped: &mut dropped,
        };
        if let Some(dataset) = assemble(
            request.out_root,
            &tool,
            &dataset_id,
            staged,
            &declared,
            &mut sinks,
        ) {
            datasets.push(dataset);
        }
    }
    // `status` stays whatever the run was: it describes the run, not one
    // dataset that ended up with nothing to scan.
    inv.warnings
        .extend(datasets.iter().filter_map(missing_evidence_warning));
    inv.warnings
        .extend(datasets.iter().flat_map(out_of_root_warnings));
    // `InventoryOnly` records (a JSON publish, an external tool's non-CSV
    // output) route through the identical `relative()` fallback but never
    // enter `dataset.files`, so `out_of_root_warnings` above cannot see them.
    // Nothing upstream guarantees those paths stay under `out_root` either.
    let inventory_only_warnings: Vec<String> = inv
        .inventory_only
        .iter()
        .filter_map(out_of_root_warning_for_inventory_only)
        .collect();
    inv.warnings.extend(inventory_only_warnings);
    inv.datasets = datasets;
    inv.dropped_overrides = dropped;

    if inv.datasets.is_empty() {
        inv.status = Status::NoCsvOutput;
    }
    inv
}

struct StagedFile {
    path: PathBuf,
    host: String,
    identity: Option<FileIdentity>,
    role: FileRole,
    derived_into: Option<PathBuf>,
}

/// The two things `assemble` writes into that outlive one dataset: the
/// run-wide view-name allocator and the dropped-override log. Bundled
/// because `clippy.toml` caps a function at 6 arguments and these are the
/// two that are not about the dataset being assembled.
struct Sinks<'a> {
    view_names: &'a mut NameAllocator,
    dropped: &'a mut Vec<DroppedOverride>,
}

/// The declared columns for a dynamic dataset id, or `None`.
///
/// A dynamic id carries the evidence in its name -- `Services_SYSTEM`,
/// `BamDam_SYSTEM_RegBack`, `Individual/Security` -- so it is matched by
/// prefix rather than by equality. Two rules make that safe to declare
/// against:
///
/// * **Longest prefix wins.** Two declarations that both match are a general
///   one and a specific one refining it, so the specific one is the answer.
/// * **Only the winner applies.** The matches are not merged. Merging would
///   let the general declaration contribute a column type the specific one
///   deliberately left out, or contradict one it deliberately changed.
///
/// The prefix carries its own separator (`"Services_"`, not `"Services"`),
/// which is what stops a declaration from claiming a longer plugin name that
/// merely starts the same way.
fn dynamic_columns_for(
    declared: &'static [DynamicColumnTypes],
    dataset_id: &str,
) -> Option<&'static [ColumnType]> {
    declared
        .iter()
        .filter(|d| dataset_id.starts_with(d.prefix))
        .max_by_key(|d| d.prefix.len())
        .map(|d| d.columns)
}

fn assemble(
    out_root: &Path,
    tool: &str,
    dataset_id: &str,
    staged: Vec<StagedFile>,
    declared: &[&'static [ColumnType]],
    sinks: &mut Sinks<'_>,
) -> Option<Dataset> {
    let view = sinks.view_names.allocate(&view_name(tool, dataset_id));
    let raw_view = format!("raw_{view}");

    let staged = collapse_duplicate_paths(staged);
    let mut files = Vec::with_capacity(staged.len());
    let mut all_columns: BTreeSet<String> = BTreeSet::new();
    let mut ordered_columns: Vec<String> = Vec::new();

    for item in staged {
        let mut warnings = Vec::new();
        let header = match read_header(&item.path) {
            Ok(raw) if raw.is_empty() => {
                warnings.push("file has no header record".into());
                None
            }
            Ok(raw) => Some(duckdb_names(&raw)),
            Err(error) => {
                warnings.push(format!("header unreadable: {error}"));
                None
            }
        };
        let included = header.is_some() && item.role != FileRole::Slice;
        if let (Some(names), true) = (header.as_ref(), included) {
            for name in names {
                if all_columns.insert(name.clone()) {
                    ordered_columns.push(name.clone());
                }
            }
        }
        files.push(DatasetFile {
            path: relative(out_root, &item.path),
            host: item.host,
            identity: item.identity,
            role: item.role,
            included_in_view: included,
            derived_into: item.derived_into.map(|p| relative(out_root, &p)),
            header,
            warnings,
        });
    }

    // Only a dataset with no files at all disappears. A dataset whose files
    // all failed to qualify -- an unreadable header, or nothing but consumed
    // per-user slices -- is still returned, so the inventory records each
    // file with `included_in_view: false` and its own warning. Dropping it
    // here would delete the evidence that the file was published and then
    // left out of the view, which is precisely what an analyst needs told.
    if files.is_empty() {
        return None;
    }

    let mut allocator = NameAllocator::new(ordered_columns.clone());
    let mut effective_types = BTreeMap::new();
    for set in declared {
        for column in *set {
            if !all_columns.contains(column.column) {
                sinks.dropped.push(DroppedOverride {
                    view: view.clone(),
                    dataset_id: dataset_id.to_string(),
                    column: column.column.to_string(),
                    declared_type: column.sql_type.sql().to_string(),
                });
                continue;
            }
            let text_column = allocator.allocate(&format!("{}__text", column.column));
            effective_types.insert(
                column.column.to_string(),
                EffectiveType::new(column.sql_type, text_column, column.time_semantics),
            );
        }
    }

    let metadata_columns = MetadataColumns {
        run_id: allocator.allocate("_triage_run_id"),
        host: allocator.allocate("_triage_host"),
        identity: allocator.allocate("_triage_identity"),
        output_file: allocator.allocate("_triage_output_file"),
    };

    let evidence_path_column = EVIDENCE_PATH_COLUMNS
        .iter()
        .find(|candidate| all_columns.contains(**candidate))
        .map(|c| c.to_string());

    Some(Dataset {
        view,
        raw_view,
        tool: tool.to_string(),
        dataset_id: dataset_id.to_string(),
        source: if tool.starts_with("ext_") {
            "external".into()
        } else {
            "internal".into()
        },
        metadata_columns,
        evidence_path_column,
        columns: ordered_columns,
        effective_types,
        files,
    })
}

/// One entry per path, whatever the caller handed us.
///
/// A dataset that scans one path twice counts every evidence row in it
/// twice, which is the failure this whole module exists to prevent, so the
/// invariant is enforced here rather than left to a caller to maintain.
/// The first record of a path wins, because it carries the host the run
/// recorded first; a later `Merged` record still promotes the role, because
/// a merged file's rows span users and must not be attributed to the single
/// identity a `Primary` record named. The loser's own identity and
/// `derived_into` are discarded with it -- a path cannot simultaneously be
/// one user's slice and the file that folded it in.
fn collapse_duplicate_paths(staged: Vec<StagedFile>) -> Vec<StagedFile> {
    let mut first_at: BTreeMap<PathBuf, usize> = BTreeMap::new();
    let mut out: Vec<StagedFile> = Vec::with_capacity(staged.len());
    for item in staged {
        match first_at.get(&item.path) {
            Some(index) => {
                if item.role == FileRole::Merged {
                    if let Some(kept) = out.get_mut(*index) {
                        kept.role = FileRole::Merged;
                        kept.identity = None;
                        kept.derived_into = None;
                    }
                }
            }
            None => {
                first_at.insert(item.path.clone(), out.len());
                out.push(item);
            }
        }
    }
    out
}

/// A dataset that has files but includes none of them contributes nothing to
/// the view. The slices are deliberately *not* restored as a fallback --
/// scanning them beside a merged file that is merely unreadable today would
/// double every row -- so the absence is recorded instead of being silently
/// correct-but-empty.
fn missing_evidence_warning(dataset: &Dataset) -> Option<String> {
    if dataset.files.is_empty() || dataset.files.iter().any(|f| f.included_in_view) {
        return None;
    }
    let excluded: Vec<String> = dataset
        .files
        .iter()
        .map(|file| {
            let reason = if file.warnings.is_empty() {
                match file.role {
                    FileRole::Slice => "folded into a merged file".to_string(),
                    _ => "not included in the view".to_string(),
                }
            } else {
                file.warnings.join("; ")
            };
            format!("{} ({reason})", file.path.display())
        })
        .collect();
    Some(format!(
        "{} / {}: no file is included in the view, so this dataset's \
         evidence does not appear in it. Excluded: {}. Slices are not \
         scanned as a fallback, because undercounting is recoverable and \
         double counting is not.",
        dataset.tool,
        dataset.dataset_id,
        excluded.join(", ")
    ))
}

/// A file whose absolute path could not be made relative to `out_root` at
/// all -- it lies entirely outside the collection root -- is still recorded
/// and still included in the view: it is real evidence the run genuinely
/// published, and dropping it would be worse than any path-hygiene concern.
/// What is not acceptable is doing this silently: `relative()` falls back to
/// the absolute path on such a file, which would otherwise leak the
/// operator's directory layout into a document whose entire stated purpose
/// is to be relocatable. So every such file gets a warning naming it.
///
/// `file.path.is_absolute()` is exactly the signal that `relative()`'s
/// `strip_prefix` fell through to its fallback, because every path that
/// stripped successfully is relative by construction.
fn out_of_root_warnings(dataset: &Dataset) -> Vec<String> {
    dataset
        .files
        .iter()
        .filter(|file| file.path.is_absolute())
        .map(|file| {
            format!(
                "{} / {}: {} lies outside out_root and could not be stored as \
                 a relative path. It is still included in the view, but this \
                 inventory is not relocatable for it.",
                dataset.tool,
                dataset.dataset_id,
                file.path.display()
            )
        })
        .collect()
}

/// The `InventoryOnly` analogue of `out_of_root_warnings`: a JSON publish or
/// a non-CSV external output whose path could not be made relative to
/// `out_root` either. These records never reach `dataset.files`, so they
/// need their own check rather than being covered by the one above -- the
/// underlying defect (`relative()`'s silent absolute-path fallback) is
/// identical, just reached through a different field.
fn out_of_root_warning_for_inventory_only(entry: &InventoryOnly) -> Option<String> {
    if !entry.path.is_absolute() {
        return None;
    }
    Some(format!(
        "{} / {}: {} lies outside out_root and could not be stored as a \
         relative path. It is still recorded (as inventory-only), but this \
         inventory is not relocatable for it.",
        entry.tool,
        entry.dataset_id,
        entry.path.display()
    ))
}

fn identity_of(identity: &Identity) -> FileIdentity {
    match identity {
        Identity::User(name) => FileIdentity::User { name: name.clone() },
        Identity::System => FileIdentity::System,
        Identity::Unknown => FileIdentity::Unknown,
    }
}

/// The dataset a merged file belongs to: the dataset of any published file
/// the merge consumed. A merge with no identifiable source falls back to the
/// file stem, which is the same string the router derived the name from.
fn dataset_id_for_merged(tool: &ToolOutputs, merged_path: &Path) -> String {
    for merge in &tool.merged {
        if merge.merged_path != merged_path {
            continue;
        }
        for source in &merge.source_paths {
            if let Some(file) = tool.published.iter().find(|p| &p.path == source) {
                return file.dataset.as_str().to_string();
            }
        }
    }
    stem_of(merged_path)
}

fn relative(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unnamed")
        .to_string()
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod dynamic_match_tests {
    use super::dynamic_columns_for;
    use crate::output::duckdb::types::{ColumnType, DynamicColumnTypes, SqlType};

    const A: &[ColumnType] = &[ColumnType {
        column: "a",
        sql_type: SqlType::BigInt,
        time_semantics: None,
    }];
    const B: &[ColumnType] = &[ColumnType {
        column: "b",
        sql_type: SqlType::Boolean,
        time_semantics: None,
    }];

    const DECL: &[DynamicColumnTypes] = &[
        DynamicColumnTypes {
            prefix: "Services_",
            columns: A,
        },
        DynamicColumnTypes {
            prefix: "Services_SYSTEM_Reg",
            columns: B,
        },
    ];

    /// Compared by column name: `ColumnType` is a public data struct and does
    /// not need a `PartialEq` it has no other caller for.
    fn names(got: Option<&'static [ColumnType]>) -> Option<Vec<&'static str>> {
        got.map(|cs| cs.iter().map(|c| c.column).collect())
    }

    #[test]
    fn matches_a_prefix_and_ignores_the_rest_of_the_id() {
        // The hive stem varies per capture; the plugin name does not.
        assert_eq!(
            names(dynamic_columns_for(DECL, "Services_SYSTEM")),
            Some(vec!["a"])
        );
    }

    #[test]
    fn the_longest_prefix_wins_and_does_not_merge() {
        // Both prefixes match this id. The specific one answers alone: if the
        // two were merged, `a` would come back alongside `b`.
        assert_eq!(
            names(dynamic_columns_for(DECL, "Services_SYSTEM_RegBack")),
            Some(vec!["b"]),
            "the more specific declaration must win outright"
        );
    }

    #[test]
    fn a_separator_in_the_prefix_stops_a_longer_plugin_name_matching() {
        // This is why prefixes are written `Services_` and not `Services`.
        assert_eq!(names(dynamic_columns_for(DECL, "ServicesHub_SYSTEM")), None);
    }

    #[test]
    fn the_prefix_must_start_the_id_not_merely_appear_in_it() {
        // A substring match would claim this: the id contains "Services_"
        // but names a different plugin. Written because a `contains` bug
        // passed every other test in this module.
        assert_eq!(
            names(dynamic_columns_for(DECL, "Legacy_Services_SYSTEM")),
            None
        );
    }

    #[test]
    fn an_unknown_dataset_gets_nothing_rather_than_a_guess() {
        assert_eq!(names(dynamic_columns_for(DECL, "TaskCache_SOFTWARE")), None);
        assert_eq!(names(dynamic_columns_for(&[], "Services_SYSTEM")), None);
    }
}
