use crate::attribution::Identity;
use crate::error::TriageError;
use crate::output::dataset::{CsvSink, DatasetSpec, JsonSink};
use crate::output::layout::{OutputLayout, OutputLayoutMode, SideCarReference};
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;

pub struct RouterOptions {
    pub csv_root: Option<std::path::PathBuf>,
    pub json_root: Option<std::path::PathBuf>,
    pub csvf: Option<String>,
    pub jsonf: Option<String>,
    pub pretty: bool,
    pub overwrite: bool,
    /// `yyyyMMddHHmmss` run stamp prepended to default output filenames (the
    /// Zimmerman convention). `None` disables stamping (used by unit tests for
    /// deterministic names). Ignored when the user supplies `--csvf`/`--jsonf`.
    pub run_stamp: Option<String>,
    /// Output directory layout: `Flat` = `<root>/<identity>_<file>`, `Nested` =
    /// the legacy `<root>/<ToolName>/<identity>/` tree. The CLI sets this
    /// (Flat unless `--nested-output`).
    pub layout_mode: OutputLayoutMode,
}

/// Environment variable that pins [`run_stamp`] to a fixed value.
///
/// Intended for tests and for reproducible output filenames, not for ordinary
/// runs: every run sharing a stamp is exactly the overwrite hazard the stamp
/// exists to prevent.
pub const RUN_STAMP_ENV: &str = "TRIAGE_RUN_STAMP";

/// Is this a value we are willing to put in a filename?
///
/// The stamp becomes part of an output path, so an unconstrained environment
/// variable would be a path-traversal primitive. Only a short alphanumeric
/// token is accepted; anything else falls back to the clock.
fn valid_stamp(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The run stamp (`yyyyMMddHHmmss`, UTC) prepended to default output filenames
/// so successive runs don't overwrite each other.
///
/// `TRIAGE_RUN_STAMP` overrides it. Without that, two runs that need to land on
/// the same filename — a test asserting the overwrite guard, say — have to
/// start inside the same wall-clock second, which is a race that fails under
/// load.
pub fn run_stamp() -> String {
    match std::env::var(RUN_STAMP_ENV) {
        Ok(pinned) if valid_stamp(&pinned) => pinned,
        _ => chrono::Utc::now().format("%Y%m%d%H%M%S").to_string(),
    }
}

/// The Velo-layout run stamp (`yyyy-MM-ddTHHmmssZ`, UTC), matching
/// VeloProcessor's `Get-ForensicTimestamp`. `TRIAGE_RUN_STAMP` overrides it,
/// same as [`run_stamp`].
pub fn velo_run_stamp() -> String {
    match std::env::var(RUN_STAMP_ENV) {
        Ok(pinned) if valid_stamp(&pinned) => pinned,
        _ => chrono::Utc::now().format("%Y-%m-%dT%H%M%SZ").to_string(),
    }
}

#[cfg(test)]
mod run_stamp_tests {
    use super::*;

    /// The stamp lands in a filename, so an unconstrained value would let an
    /// environment variable steer where output is written.
    #[test]
    fn only_a_filename_safe_token_is_accepted() {
        assert!(valid_stamp("20260101000000"));
        assert!(valid_stamp("case-1_run2"));
        assert!(!valid_stamp(""));
        assert!(!valid_stamp("../../etc/passwd"));
        assert!(!valid_stamp("a/b"));
        assert!(!valid_stamp("a\\b"));
        assert!(!valid_stamp("with space"));
        assert!(!valid_stamp(&"x".repeat(33)));
    }

    /// The default is still the clock, in the documented shape.
    #[test]
    fn the_unpinned_stamp_is_a_utc_timestamp() {
        let stamp = run_stamp();
        assert_eq!(stamp.len(), 14, "got {stamp}");
        assert!(stamp.chars().all(|c| c.is_ascii_digit()), "got {stamp}");
    }

    /// The Velo stamp format (`yyyy-MM-ddTHHmmssZ`) is used for Velo-layout
    /// output trees and must be filename-safe and parseable.
    #[test]
    fn the_velo_stamp_is_velos_documented_format() {
        let stamp = velo_run_stamp();
        assert_eq!(stamp.len(), 18, "got {stamp}");
        assert!(stamp.ends_with('Z'), "got {stamp}");
        assert_eq!(&stamp[4..5], "-", "got {stamp}");
        assert_eq!(&stamp[10..11], "T", "got {stamp}");
        assert!(
            valid_stamp(&stamp),
            "the Velo stamp must be filename-safe: {stamp}"
        );
    }
}

struct DatasetFiles {
    csv: Option<(CsvSink<File>, PathBuf, PathBuf)>,
    json: Option<(JsonSink<File>, PathBuf, PathBuf)>,
}

struct DynamicFiles {
    headers: Vec<String>,
    csv_only: bool,
    csv: Option<(csv::Writer<File>, PathBuf, PathBuf)>,
    json: Option<(JsonSink<File>, PathBuf, PathBuf)>,
}

/// Velo-layout basename for a dataset: `<Tool>_results[_<Discriminator>]`.
/// The discriminator comes from [`velo_discriminator`], which documents how
/// it is derived; `velo_basenames_are_unique_per_tool` in triage-orchestrator
/// fails the build if two of a tool's datasets ever produce the same name.
pub fn velo_basename(binary_name: &str, spec: &DatasetSpec) -> String {
    match velo_discriminator(binary_name, spec) {
        Some(discriminator) => format!("{binary_name}_results_{discriminator}"),
        None => format!("{binary_name}_results"),
    }
}

/// The `<Discriminator>` half of [`velo_basename`], or `None` for a dataset
/// that has none (a tool's single or primary dataset, whose Velo name is a
/// bare `<Tool>_results`).
///
/// The discriminator is recovered from `default_basename`, which follows one
/// of three shapes across the tree: `<Tool>_Output[_<Disc>]` (PETriage),
/// `<Tool>_<Disc>_Output` (SrumETriage), or `<Tool>_<Disc>_Output_<Disc2>`
/// with the marker *inside* the name (MFTriage's `MFTriage_$MFT_Output_FileListing`,
/// where the marker separates a source identifier from a sub-dataset name and
/// neither half is the whole discriminator on its own). A basename matching
/// none of these is used whole rather than mangled.
///
/// Returned separately because it is not only part of a filename: under
/// `OutputLayoutMode::Velo` it is also the directory a dataset's per-user
/// slices are written into (`OutputLayout::for_velo_dataset`, which explains
/// why they need one). Derived here from the `DatasetSpec` itself rather than
/// recovered by splitting an assembled name apart, so the directory and the
/// filename cannot disagree about which dataset a file belongs to.
pub fn velo_discriminator(binary_name: &str, spec: &DatasetSpec) -> Option<String> {
    let stem = spec
        .default_basename
        .strip_prefix(binary_name)
        .and_then(|rest| rest.strip_prefix('_'))
        .unwrap_or(spec.default_basename);

    let discriminator = if stem == "Output" {
        String::new()
    } else if let Some(rest) = stem.strip_prefix("Output_") {
        rest.to_string()
    } else if let Some(rest) = stem.strip_suffix("_Output") {
        rest.to_string()
    } else if let Some((before, after)) = stem.split_once("_Output_") {
        // An inner marker (e.g. `$MFT_Output_FileListing`): both halves are
        // real discriminator content, so join them rather than picking one.
        format!("{before}_{after}")
    } else {
        stem.to_string()
    };

    if discriminator.is_empty() {
        None
    } else {
        Some(discriminator)
    }
}

/// Output filename for a dataset: the default basename, or the user override
/// (primary dataset gets it verbatim; suffixed datasets get `{stem}{suffix}{ext}`).
fn dataset_filename(
    spec: &DatasetSpec,
    override_name: &Option<String>,
    ext: &str,
    run_stamp: &Option<String>,
    layout_mode: OutputLayoutMode,
    binary_name: &str,
) -> String {
    match (override_name, spec.override_suffix) {
        // Default name: prepend the run stamp (Zimmerman convention), e.g.
        // `20260616184308_EvtxTriage_Output.csv`.
        (None, _) => {
            let basename = match layout_mode {
                OutputLayoutMode::Velo => velo_basename(binary_name, spec),
                _ => spec.default_basename.to_string(),
            };
            match run_stamp {
                Some(stamp) => format!("{stamp}_{basename}.{ext}"),
                None => format!("{basename}.{ext}"),
            }
        }
        // User-supplied name wins verbatim; no stamp.
        (Some(name), None) => name.clone(),
        (Some(name), Some(suffix)) => {
            let p = std::path::Path::new(name);
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
            match p.extension().and_then(|e| e.to_str()) {
                Some(e) => format!("{stem}{suffix}.{e}"),
                None => format!("{stem}{suffix}"),
            }
        }
    }
}

/// What [`OutputRouter::finish`] did: the destinations it actually published,
/// and the record count or the first failure.
///
/// The two are reported together because a caller needs both on the failing
/// path: `finish` publishes each sink in turn and returns the *first* error,
/// so a failure can still leave earlier destinations published, and an
/// abort after a write failure publishes nothing at all. Neither is
/// recoverable from a `Result` alone, and neither is recoverable from the
/// filesystem: a destination this run never published can still hold a
/// *previous* run's file, so existence says nothing about ownership. That
/// mistake is what this type exists to make unavailable -- see
/// `triage_orchestrator::velo::merge::merge_per_user`, which may only
/// reclaim a category-root file this run published itself.
#[derive(Debug)]
#[must_use]
pub struct FinishReport {
    /// Every final destination whose staged file was successfully renamed
    /// into place by this `finish()`, sorted and deduplicated. Empty when
    /// the router aborted after an earlier write failure, because that path
    /// deletes every staged file and publishes none of them.
    pub published: Vec<PathBuf>,
    /// Total records written, or the first error `finish()` hit.
    pub outcome: Result<u64, TriageError>,
}

impl FinishReport {
    /// The record count or the first error, for a caller that does not care
    /// which destinations were published.
    pub fn into_outcome(self) -> Result<u64, TriageError> {
        self.outcome
    }
}

/// Routes records to lazily-created CSV/JSON files keyed by
/// (identity, dataset id). CSV and JSON are written from the same record
/// in one call (spec section 3.3: both formats from one parse).
pub struct OutputRouter {
    csv_layout: Option<OutputLayout>,
    json_layout: Option<OutputLayout>,
    datasets: &'static [DatasetSpec],
    csvf: Option<String>,
    jsonf: Option<String>,
    pretty: bool,
    run_stamp: Option<String>,
    layout_mode: OutputLayoutMode,
    binary_name: String,
    identity: Identity,
    open: HashMap<(Identity, &'static str), DatasetFiles>,
    dynamic: HashMap<(Identity, String), DynamicFiles>,
    records: u64,
    overwrite: bool,
    failed: bool,
}

impl OutputRouter {
    pub fn new(
        binary_name: &str,
        datasets: &'static [DatasetSpec],
        opts: RouterOptions,
    ) -> Result<Self, TriageError> {
        for (flag, value) in [
            ("--csvf", opts.csvf.as_deref()),
            ("--jsonf", opts.jsonf.as_deref()),
        ] {
            if let Some(value) = value {
                let path = std::path::Path::new(value);
                if value.is_empty()
                    || value.contains(['/', '\\'])
                    || path.is_absolute()
                    || path.components().count() != 1
                    || matches!(value, "." | "..")
                {
                    return Err(TriageError::Usage(format!(
                        "{flag} must be a filename, not a path"
                    )));
                }
            }
        }
        let primaries = datasets
            .iter()
            .filter(|d| d.override_suffix.is_none())
            .count();
        if primaries > 1 && (opts.csvf.is_some() || opts.jsonf.is_some()) {
            return Err(TriageError::Usage(
                "--csvf/--jsonf is ambiguous for tools with multiple primary datasets".into(),
            ));
        }

        Ok(Self {
            csv_layout: opts
                .csv_root
                .map(|r| OutputLayout::new(&r, binary_name, opts.overwrite, opts.layout_mode)),
            json_layout: opts
                .json_root
                .map(|r| OutputLayout::new(&r, binary_name, opts.overwrite, opts.layout_mode)),
            datasets,
            csvf: opts.csvf,
            jsonf: opts.jsonf,
            pretty: opts.pretty,
            run_stamp: opts.run_stamp,
            layout_mode: opts.layout_mode,
            binary_name: binary_name.to_string(),
            identity: Identity::Unknown,
            open: HashMap::new(),
            dynamic: HashMap::new(),
            records: 0,
            overwrite: opts.overwrite,
            failed: false,
        })
    }

    /// Output roots, used by discovery to exclude them from the input walk.
    pub fn roots(&self) -> Vec<std::path::PathBuf> {
        [self.csv_layout.as_ref(), self.json_layout.as_ref()]
            .into_iter()
            .flatten()
            .map(|l| l.root().to_path_buf())
            .collect()
    }

    /// Set the identity for subsequent writes (the runner calls this before
    /// handing an artifact to the tool's parser).
    pub fn set_identity(&mut self, identity: Identity) {
        self.identity = identity;
    }

    pub fn write<T: serde::Serialize>(
        &mut self,
        dataset_id: &'static str,
        record: &T,
    ) -> Result<(), TriageError> {
        let result = self.write_static(dataset_id, record);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn write_static<T: serde::Serialize>(
        &mut self,
        dataset_id: &'static str,
        record: &T,
    ) -> Result<(), TriageError> {
        let spec = self
            .datasets
            .iter()
            .find(|d| d.id == dataset_id)
            .ok_or_else(|| TriageError::Fatal(format!("unknown dataset id: {dataset_id}")))?;

        let key = (self.identity.clone(), spec.id);
        if !self.open.contains_key(&key) {
            let csv = match &self.csv_layout {
                Some(layout) => {
                    let name = dataset_filename(
                        spec,
                        &self.csvf,
                        "csv",
                        &self.run_stamp,
                        self.layout_mode,
                        &self.binary_name,
                    );
                    // Per-dataset, not per-router: the discriminator decides
                    // which `PerUser/` directory this dataset's per-user
                    // slices go in (`OutputLayout::for_velo_dataset`), and a
                    // router serves every dataset of one tool. Inert under
                    // `Nested` and `Flat`, which have no `PerUser/` tree.
                    let staged = layout
                        .for_velo_dataset(velo_discriminator(&self.binary_name, spec).as_deref())
                        .create_staged(&self.identity, &name)?;
                    Some((
                        CsvSink::new(staged.file),
                        staged.temporary,
                        staged.destination,
                    ))
                }
                None => None,
            };
            let json_result = if spec.csv_only {
                Ok(None)
            } else {
                match &self.json_layout {
                    Some(layout) => {
                        let name = dataset_filename(
                            spec,
                            &self.jsonf,
                            "json",
                            &self.run_stamp,
                            self.layout_mode,
                            &self.binary_name,
                        );
                        layout
                            .for_velo_dataset(
                                velo_discriminator(&self.binary_name, spec).as_deref(),
                            )
                            .create_staged(&self.identity, &name)
                            .map(|staged| {
                                Some((
                                    JsonSink::new(staged.file, spec.framing, self.pretty),
                                    staged.temporary,
                                    staged.destination,
                                ))
                            })
                    }
                    None => Ok(None),
                }
            };
            let json = match json_result {
                Ok(json) => json,
                Err(error) => {
                    if let Some((writer, temporary, _)) = csv {
                        drop(writer);
                        let _ = std::fs::remove_file(temporary);
                    }
                    return Err(error);
                }
            };
            self.open.insert(key.clone(), DatasetFiles { csv, json });
        }

        let files = self
            .open
            .get_mut(&key)
            .ok_or_else(|| TriageError::Fatal("output sink disappeared".into()))?;
        if let Some((csv, _, path)) = files.csv.as_mut() {
            csv.write(record).map_err(|e| TriageError::Output {
                path: path.clone(),
                message: e.to_string(),
            })?;
        }
        if let Some((json, _, path)) = files.json.as_mut() {
            json.write(record).map_err(|e| TriageError::Output {
                path: path.clone(),
                message: e.to_string(),
            })?;
        }
        self.records += 1;
        Ok(())
    }

    /// Write a row whose CSV schema and basename are known only at runtime.
    /// The same logical row is also emitted as NDJSON when JSON output is
    /// enabled. Reusing a destination with different headers is rejected.
    pub fn write_dynamic_row(
        &mut self,
        basename: &str,
        headers: &[String],
        row: &[String],
    ) -> Result<(), TriageError> {
        let result = self.write_dynamic_values(basename, headers, row, false, None);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// CSV-only form of [`Self::write_dynamic_row`].
    pub fn write_dynamic_csv_row(
        &mut self,
        basename: &str,
        headers: &[String],
        row: &[String],
    ) -> Result<(), TriageError> {
        let result = self.write_dynamic_values(basename, headers, row, true, None);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// Write any serializable object under a runtime basename.
    pub fn write_dynamic_record<T: serde::Serialize>(
        &mut self,
        basename: &str,
        record: &T,
    ) -> Result<(), TriageError> {
        let value = serde_json::to_value(record).map_err(|e| TriageError::Fatal(e.to_string()))?;
        let object = value.as_object().ok_or_else(|| {
            TriageError::Fatal("dynamic record must serialize as an object".into())
        })?;
        let headers: Vec<String> = object.keys().cloned().collect();
        let row: Vec<String> = object.values().map(render_csv_value).collect();
        let result = self.write_dynamic_values(basename, &headers, &row, false, Some(&value));
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// Emit an NDJSON object to a runtime basename. CSV, when configured, is
    /// derived from the object's keys and scalar values.
    pub fn write_dynamic_object(
        &mut self,
        basename: &str,
        object: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), TriageError> {
        self.write_dynamic_record(basename, &serde_json::Value::Object(object.clone()))
    }

    /// Redirect an already-open dynamic dataset's eventual publish
    /// destination from `old_basename` to `new_basename`, without moving any
    /// data: writes go to a temporary file until `finish()` renames it to
    /// its destination (`create_staged`/`publish`), so retargeting that
    /// destination here is free of filesystem work and cannot collide with
    /// another writer's open temporary file.
    ///
    /// Exists for callers that must fold two logically-equivalent runtime
    /// basenames (e.g. two `Channel` spellings that only disagree in case)
    /// into one on-disk file under a name chosen by a rule that isn't known
    /// until after the first one is written — `EvtxTool`'s `Individual/`
    /// export canonicalizes on the lexicographically smallest spelling,
    /// which a later write can reveal only after an earlier, larger-spelled
    /// one already opened the file.
    ///
    /// # Contract
    ///
    /// Both basenames are looked up under the router's *current* `identity`,
    /// same as every other dynamic write — a caller that rekeys across an
    /// identity change must `set_identity` first, same as it would for any
    /// other call in this module.
    ///
    /// Returns `Ok(false)` (a no-op) if `old_basename` has no open dynamic
    /// entry, or if the two basenames are equal. Returns `Err` — leaving
    /// both entries exactly as they were, nothing removed or moved — in two
    /// cases this deliberately never tries to paper over:
    /// - `new_basename` already has its own open dynamic entry. Inserting
    ///   over it would silently drop that entry's unflushed writer and
    ///   orphan its temporary file — reachable by neither `finish()`,
    ///   `Drop`, nor `cleanup_temporaries` afterward — so this is refused
    ///   rather than risked.
    /// - `old_basename` and `new_basename` resolve to different parent
    ///   directories. Unlike `create_staged` (which `create_dir_all`s a
    ///   *new* file's parent), a rekey never creates directories, so a
    ///   cross-directory move would only surface as a rename failure inside
    ///   `finish()`, far from this call site; refusing it here fails fast
    ///   with a specific reason instead.
    ///
    /// `EvtxTool::resolve_individual_stem` is the one existing caller, and
    /// it upholds both invariants by construction: it moves the registered
    /// canonical spelling strictly downward (lexicographically smaller,
    /// never sideways or back up), one step at a time, so a basename it has
    /// already abandoned as an `old_basename` can never simultaneously be
    /// some *other* group's still-live `new_basename` target; and every
    /// candidate basename is sanitized into the same flat `Individual/`
    /// directory, so a rekey between them is always same-directory. A
    /// future caller must uphold the same two properties, or handle the
    /// `Err` this returns when it doesn't.
    pub fn rekey_dynamic(
        &mut self,
        old_basename: &str,
        new_basename: &str,
    ) -> Result<bool, TriageError> {
        if old_basename == new_basename {
            return Ok(false);
        }
        let old_key = (self.identity.clone(), old_basename.to_string());
        if !self.dynamic.contains_key(&old_key) {
            return Ok(false);
        }
        let new_key = (self.identity.clone(), new_basename.to_string());
        if self.dynamic.contains_key(&new_key) {
            return Err(TriageError::Fatal(format!(
                "rekey_dynamic: refusing to move {old_basename:?} onto {new_basename:?}: \
                 {new_basename:?} already has its own open dynamic entry, and clobbering it \
                 would orphan its writer and temporary file"
            )));
        }

        // Compute both new destinations, and reject a cross-directory move,
        // before touching the map at all -- a rejected rekey must leave the
        // old entry exactly as it was, not removed and lost.
        let files_ref = self
            .dynamic
            .get(&old_key)
            .expect("checked contains_key above");
        let new_csv_destination = match (&files_ref.csv, &self.csv_layout) {
            (Some((_, _, old_destination)), Some(layout)) => {
                let new_destination =
                    layout.file_path(&self.identity, &format!("{new_basename}.csv"));
                if new_destination.parent() != old_destination.parent() {
                    return Err(TriageError::Fatal(format!(
                        "rekey_dynamic: {old_basename:?} -> {new_basename:?} would move the CSV \
                         output across directories ({:?} -> {:?}), which is not supported",
                        old_destination.parent(),
                        new_destination.parent()
                    )));
                }
                Some(new_destination)
            }
            _ => None,
        };
        let new_json_destination = match (&files_ref.json, &self.json_layout) {
            (Some((_, _, old_destination)), Some(layout)) => {
                let new_destination =
                    layout.file_path(&self.identity, &format!("{new_basename}.json"));
                if new_destination.parent() != old_destination.parent() {
                    return Err(TriageError::Fatal(format!(
                        "rekey_dynamic: {old_basename:?} -> {new_basename:?} would move the JSON \
                         output across directories ({:?} -> {:?}), which is not supported",
                        old_destination.parent(),
                        new_destination.parent()
                    )));
                }
                Some(new_destination)
            }
            _ => None,
        };

        let mut files = self
            .dynamic
            .remove(&old_key)
            .expect("checked contains_key above, and nothing since has touched `dynamic`");
        if let (Some((_, _, destination)), Some(new_destination)) =
            (files.csv.as_mut(), new_csv_destination)
        {
            *destination = new_destination;
        }
        if let (Some((_, _, destination)), Some(new_destination)) =
            (files.json.as_mut(), new_json_destination)
        {
            *destination = new_destination;
        }
        self.dynamic.insert(new_key, files);
        Ok(true)
    }

    fn write_dynamic_values(
        &mut self,
        basename: &str,
        headers: &[String],
        row: &[String],
        csv_only: bool,
        json_value: Option<&serde_json::Value>,
    ) -> Result<(), TriageError> {
        if headers.len() != row.len() {
            return Err(TriageError::Fatal(format!(
                "dynamic dataset {basename} row has {} values for {} headers",
                row.len(),
                headers.len()
            )));
        }
        let key = (self.identity.clone(), basename.to_string());
        if !self.dynamic.contains_key(&key) {
            let csv = match &self.csv_layout {
                Some(layout) => {
                    let staged =
                        layout.create_staged(&self.identity, &format!("{basename}.csv"))?;
                    let mut writer = csv::Writer::from_writer(staged.file);
                    if let Err(error) = writer.write_record(headers) {
                        drop(writer);
                        let _ = std::fs::remove_file(&staged.temporary);
                        return Err(TriageError::Output {
                            path: staged.destination,
                            message: error.to_string(),
                        });
                    }
                    Some((writer, staged.temporary, staged.destination))
                }
                None => None,
            };
            let json_result = if csv_only {
                Ok(None)
            } else {
                match &self.json_layout {
                    Some(layout) => layout
                        .create_staged(&self.identity, &format!("{basename}.json"))
                        .map(|staged| {
                            Some((
                                JsonSink::new(
                                    staged.file,
                                    crate::output::dataset::JsonFraming::Ndjson,
                                    false,
                                ),
                                staged.temporary,
                                staged.destination,
                            ))
                        }),
                    None => Ok(None),
                }
            };
            let json = match json_result {
                Ok(json) => json,
                Err(error) => {
                    if let Some((writer, temporary, _)) = csv {
                        drop(writer);
                        let _ = std::fs::remove_file(temporary);
                    }
                    return Err(error);
                }
            };
            self.dynamic.insert(
                key.clone(),
                DynamicFiles {
                    headers: headers.to_vec(),
                    csv_only,
                    csv,
                    json,
                },
            );
        }
        let files = self
            .dynamic
            .get_mut(&key)
            .ok_or_else(|| TriageError::Fatal("dynamic output sink disappeared".into()))?;
        if files.headers != headers || files.csv_only != csv_only {
            return Err(TriageError::Output {
                path: PathBuf::from(basename),
                message: "conflicting schema or format for dynamic dataset".into(),
            });
        }
        if let Some((writer, _, path)) = files.csv.as_mut() {
            writer.write_record(row).map_err(|e| TriageError::Output {
                path: path.clone(),
                message: e.to_string(),
            })?;
        }
        if let Some((writer, _, path)) = files.json.as_mut() {
            let owned;
            let value = if let Some(value) = json_value {
                value
            } else {
                owned = serde_json::Value::Object(
                    headers
                        .iter()
                        .cloned()
                        .zip(row.iter().cloned().map(serde_json::Value::String))
                        .collect(),
                );
                &owned
            };
            writer.write(value).map_err(|e| TriageError::Output {
                path: path.clone(),
                message: e.to_string(),
            })?;
        }
        self.records += 1;
        Ok(())
    }

    /// Return the directory where the current identity's CSV files are written,
    /// or `None` if no CSV output root was configured. Callers (e.g. RETriage's
    /// per-plugin detail-CSV writer) use this to co-locate side-car files in the
    /// same identity directory as the batch CSV.
    pub fn current_csv_dir(&self) -> Option<std::path::PathBuf> {
        let layout = self.csv_layout.as_ref()?;
        // layout.file_path returns the full path including filename; strip the
        // file component to get the directory.
        let dummy = layout.file_path(&self.identity, "_dummy_");
        dummy.parent().map(|p| p.to_path_buf())
    }

    /// Return the directory where the current identity's JSON files are written,
    /// or `None` if no JSON output root was configured. Mirror of
    /// `current_csv_dir`; used by tools that write dynamic NDJSON side-car files
    /// (e.g. SQLETriage's per-map-query output) co-located with the CSV.
    pub fn current_json_dir(&self) -> Option<std::path::PathBuf> {
        let layout = self.json_layout.as_ref()?;
        let dummy = layout.file_path(&self.identity, "_dummy_");
        dummy.parent().map(|p| p.to_path_buf())
    }

    /// Layout-correct filename for a dynamic side-car in the current identity's
    /// output dir (pairs with current_csv_dir/current_json_dir). In Flat mode the
    /// identity is folded into the name so side-cars for different identities
    /// don't collide at the shared root; in Nested mode the bare name is kept
    /// (the per-identity directory already carries the identity).
    pub fn dynamic_filename(&self, name: &str) -> String {
        self.csv_layout
            .as_ref()
            .or(self.json_layout.as_ref())
            .map(|l| l.dynamic_filename(&self.identity, name))
            .unwrap_or_else(|| name.to_string())
    }

    /// A detached calculator for the references naming this tool's dynamic
    /// side-cars, fixed to the current identity and layout
    /// (`OutputLayout::side_car_reference`).
    ///
    /// Detached because a caller writing a cross-reference into a record
    /// already holds `&mut self` for the write. Snapshot it before the write
    /// loop; it stops being correct if the identity changes, which is the
    /// same boundary a tool's `parse` call already has.
    ///
    /// The CSV layout is preferred over the JSON one because a side-car
    /// reference names a CSV file, and falls back to the JSON layout -- which
    /// arranges paths by the same rules, under its own root -- so the string
    /// is still well-formed for the identity and directory in a JSON-only
    /// run. Well-formed, not resolvable: `write_dynamic_csv_row` opens no
    /// sink at all without a CSV layout, so in that mode there is no detail
    /// CSV for the reference to name. Same precedence, and same reason, as
    /// `dynamic_filename` above.
    pub fn side_car_reference(&self) -> SideCarReference {
        SideCarReference::new(
            self.csv_layout
                .as_ref()
                .or(self.json_layout.as_ref())
                .cloned(),
            self.identity.clone(),
        )
    }

    /// Return the current output identity (set by the runner via set_identity).
    /// Used by tools that write side-car files co-located with the main output.
    pub fn current_identity(&self) -> &Identity {
        &self.identity
    }

    /// Flush and close every open file, publishing each staged file onto its
    /// final destination, and report what was published along with the record
    /// count or the first error ([`FinishReport`]).
    ///
    /// Attempts all sinks even if an earlier one fails (best-effort flush).
    pub fn finish(mut self) -> FinishReport {
        if self.failed {
            self.cleanup_temporaries();
            // Nothing was renamed onto a destination, so nothing was
            // published -- whatever sits at those paths belongs to some
            // earlier run.
            return FinishReport {
                published: Vec::new(),
                outcome: Err(TriageError::Output {
                    path: PathBuf::from("<output-router>"),
                    message: "output transaction aborted after an earlier write failure".into(),
                }),
            };
        }
        let mut first_err: Option<TriageError> = None;
        let mut published: Vec<PathBuf> = Vec::new();

        for (_, files) in self.open.drain() {
            if let Some((csv, temporary, path)) = files.csv {
                if let Err(e) = csv.finish() {
                    let _ = std::fs::remove_file(&temporary);
                    first_err.get_or_insert(TriageError::Output {
                        path: path.clone(),
                        message: e.to_string(),
                    });
                } else {
                    match publish(&temporary, &path, self.overwrite) {
                        Ok(()) => published.push(path),
                        Err(e) => {
                            first_err.get_or_insert(e);
                        }
                    }
                }
            }
            if let Some((json, temporary, path)) = files.json {
                if let Err(e) = json.finish() {
                    let _ = std::fs::remove_file(&temporary);
                    first_err.get_or_insert(TriageError::Output {
                        path: path.clone(),
                        message: e.to_string(),
                    });
                } else {
                    match publish(&temporary, &path, self.overwrite) {
                        Ok(()) => published.push(path),
                        Err(e) => {
                            first_err.get_or_insert(e);
                        }
                    }
                }
            }
        }

        for (_, files) in self.dynamic.drain() {
            if let Some((mut csv, temporary, path)) = files.csv {
                if let Err(e) = csv.flush() {
                    let _ = std::fs::remove_file(&temporary);
                    first_err.get_or_insert(TriageError::Output {
                        path: path.clone(),
                        message: e.to_string(),
                    });
                } else {
                    match publish(&temporary, &path, self.overwrite) {
                        Ok(()) => published.push(path),
                        Err(e) => {
                            first_err.get_or_insert(e);
                        }
                    }
                }
            }
            if let Some((json, temporary, path)) = files.json {
                if let Err(e) = json.finish() {
                    let _ = std::fs::remove_file(&temporary);
                    first_err.get_or_insert(TriageError::Output {
                        path: path.clone(),
                        message: e.to_string(),
                    });
                } else {
                    match publish(&temporary, &path, self.overwrite) {
                        Ok(()) => published.push(path),
                        Err(e) => {
                            first_err.get_or_insert(e);
                        }
                    }
                }
            }
        }

        published.sort();
        published.dedup();
        FinishReport {
            published,
            outcome: match first_err {
                Some(err) => Err(err),
                None => Ok(self.records),
            },
        }
    }

    fn cleanup_temporaries(&mut self) {
        let open = std::mem::take(&mut self.open);
        for (_, files) in open {
            if let Some((writer, temporary, _)) = files.csv {
                drop(writer);
                let _ = std::fs::remove_file(temporary);
            }
            if let Some((writer, temporary, _)) = files.json {
                drop(writer);
                let _ = std::fs::remove_file(temporary);
            }
        }
        let dynamic = std::mem::take(&mut self.dynamic);
        for (_, files) in dynamic {
            if let Some((writer, temporary, _)) = files.csv {
                drop(writer);
                let _ = std::fs::remove_file(temporary);
            }
            if let Some((writer, temporary, _)) = files.json {
                drop(writer);
                let _ = std::fs::remove_file(temporary);
            }
        }
    }
}

fn publish(
    temporary: &std::path::Path,
    destination: &std::path::Path,
    overwrite: bool,
) -> Result<(), TriageError> {
    if !overwrite && destination.exists() {
        let _ = std::fs::remove_file(temporary);
        return Err(TriageError::Output {
            path: destination.to_path_buf(),
            message: "output file exists; pass --overwrite to replace it".into(),
        });
    }
    if let Err(e) = std::fs::rename(temporary, destination) {
        let _ = std::fs::remove_file(temporary);
        return Err(TriageError::Output {
            path: destination.to_path_buf(),
            message: e.to_string(),
        });
    }
    Ok(())
}

fn render_csv_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(v) => v.to_string(),
        serde_json::Value::Number(v) => v.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

impl Drop for OutputRouter {
    fn drop(&mut self) {
        for files in self.open.values() {
            if let Some((_, temporary, _)) = &files.csv {
                let _ = std::fs::remove_file(temporary);
            }
            if let Some((_, temporary, _)) = &files.json {
                let _ = std::fs::remove_file(temporary);
            }
        }
        for files in self.dynamic.values() {
            if let Some((_, temporary, _)) = &files.csv {
                let _ = std::fs::remove_file(temporary);
            }
            if let Some((_, temporary, _)) = &files.json {
                let _ = std::fs::remove_file(temporary);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribution::Identity;
    use crate::output::dataset::{DatasetSpec, JsonFraming};
    use serde::Serialize;

    #[derive(Serialize)]
    struct Row {
        #[serde(rename = "Name")]
        name: String,
    }

    const DATASETS: &[DatasetSpec] = &[DatasetSpec {
        id: "stub",
        default_basename: "StubTriage_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    }];

    fn opts(tmp: &std::path::Path) -> RouterOptions {
        RouterOptions {
            csv_root: Some(tmp.join("csv")),
            json_root: Some(tmp.join("json")),
            csvf: None,
            jsonf: None,
            pretty: false,
            overwrite: false,
            run_stamp: None,
            layout_mode: OutputLayoutMode::Flat,
        }
    }

    #[test]
    fn velo_basenames_strip_the_tool_prefix_and_the_output_marker() {
        fn spec(basename: &'static str, suffix: Option<&'static str>) -> DatasetSpec {
            DatasetSpec {
                id: "x",
                default_basename: basename,
                framing: JsonFraming::Ndjson,
                csv_only: false,
                override_suffix: suffix,
            }
        }

        // Primary dataset, no discriminator.
        assert_eq!(
            velo_basename("PETriage", &spec("PETriage_Output", None)),
            "PETriage_results"
        );
        // Discriminator after the Output marker.
        assert_eq!(
            velo_basename(
                "PETriage",
                &spec("PETriage_Output_Timeline", Some("_Timeline"))
            ),
            "PETriage_results_Timeline"
        );
        // Discriminator before the Output marker, primary dataset.
        assert_eq!(
            velo_basename(
                "SrumETriage",
                &spec("SrumETriage_NetworkUsages_Output", None)
            ),
            "SrumETriage_results_NetworkUsages"
        );
        // Discriminator before the Output marker, non-primary dataset.
        assert_eq!(
            velo_basename(
                "SrumETriage",
                &spec(
                    "SrumETriage_AppResourceUseInfo_Output",
                    Some("_AppResourceUseInfo")
                )
            ),
            "SrumETriage_results_AppResourceUseInfo"
        );
        // A basename that follows no convention falls back to itself, unmangled.
        assert_eq!(
            velo_basename("T", &spec("Something_Odd", None)),
            "T_results_Something_Odd"
        );
        // Inner `_Output_` marker (MFTriage's file-listing dataset): both
        // halves are real discriminator content, so they must be joined
        // rather than the literal `_Output_` leaking into the filename.
        assert_eq!(
            velo_basename(
                "MFTriage",
                &spec("MFTriage_$MFT_Output_FileListing", Some("_FileListing"))
            ),
            "MFTriage_results_$MFT_FileListing"
        );
    }

    #[test]
    fn velo_mode_uses_the_velo_basename_and_native_modes_do_not() {
        let s = DatasetSpec {
            id: "x",
            default_basename: "PETriage_Output",
            framing: JsonFraming::Ndjson,
            csv_only: false,
            override_suffix: None,
        };
        let stamp = Some("2026-03-13T192553Z".to_string());
        assert_eq!(
            dataset_filename(&s, &None, "csv", &stamp, OutputLayoutMode::Velo, "PETriage"),
            "2026-03-13T192553Z_PETriage_results.csv"
        );
        assert_eq!(
            dataset_filename(
                &s,
                &None,
                "csv",
                &stamp,
                OutputLayoutMode::Nested,
                "PETriage"
            ),
            "2026-03-13T192553Z_PETriage_Output.csv"
        );
    }

    /// A user-supplied --csvf still wins verbatim in every mode.
    #[test]
    fn velo_mode_does_not_override_an_explicit_csvf() {
        let s = DatasetSpec {
            id: "x",
            default_basename: "PETriage_Output",
            framing: JsonFraming::Ndjson,
            csv_only: false,
            override_suffix: None,
        };
        assert_eq!(
            dataset_filename(
                &s,
                &Some("custom.csv".into()),
                "csv",
                &Some("2026-03-13T192553Z".into()),
                OutputLayoutMode::Velo,
                "PETriage"
            ),
            "custom.csv"
        );
    }

    #[test]
    fn routes_csv_and_json_per_identity_from_same_records() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("StubTriage", DATASETS, opts(tmp.path())).unwrap();

        r.set_identity(Identity::User("alice".into()));
        r.write("stub", &Row { name: "a".into() }).unwrap();
        r.set_identity(Identity::System);
        r.write("stub", &Row { name: "b".into() }).unwrap();
        let total = r.finish().into_outcome().unwrap();
        assert_eq!(total, 2);

        let alice_csv = tmp.path().join("csv/StubTriage_Output_alice.csv");
        let sys_json = tmp.path().join("json/StubTriage_Output_system.json");
        assert_eq!(std::fs::read_to_string(alice_csv).unwrap(), "Name\na\n");
        assert_eq!(
            std::fs::read_to_string(sys_json).unwrap(),
            "{\"Name\":\"b\"}\n"
        );
    }

    #[test]
    fn filename_overrides_apply_within_each_identity_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let mut o = opts(tmp.path());
        o.csvf = Some("custom.csv".into());
        let mut r = OutputRouter::new("StubTriage", DATASETS, o).unwrap();
        r.set_identity(Identity::Unknown);
        r.write("stub", &Row { name: "x".into() }).unwrap();
        r.finish().into_outcome().unwrap();
        assert!(tmp.path().join("csv/custom_unknown.csv").exists());
    }

    #[test]
    fn unknown_dataset_id_is_a_fatal_error() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("StubTriage", DATASETS, opts(tmp.path())).unwrap();
        r.set_identity(Identity::System);
        assert!(r.write("nope", &Row { name: "x".into() }).is_err());
    }

    #[test]
    fn filename_overrides_reject_paths_and_dot_segments() {
        let tmp = tempfile::tempdir().unwrap();
        for bad in ["../x.csv", "/tmp/x.csv", r"C:\x.csv", ".", ""] {
            let mut options = opts(tmp.path());
            options.csvf = Some(bad.into());
            assert!(
                matches!(
                    OutputRouter::new("T", DATASETS, options),
                    Err(TriageError::Usage(_))
                ),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn same_identity_and_dataset_reuses_one_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("StubTriage", DATASETS, opts(tmp.path())).unwrap();
        r.set_identity(Identity::User("alice".into()));
        r.write("stub", &Row { name: "a".into() }).unwrap();
        r.write("stub", &Row { name: "b".into() }).unwrap();
        let total = r.finish().into_outcome().unwrap();
        assert_eq!(total, 2);
        let csv = tmp.path().join("csv/StubTriage_Output_alice.csv");
        assert_eq!(std::fs::read_to_string(csv).unwrap(), "Name\na\nb\n");
    }

    #[test]
    fn two_datasets_without_override_coexist() {
        const TWO: &[DatasetSpec] = &[
            DatasetSpec {
                id: "a",
                default_basename: "A_Out",
                framing: JsonFraming::Ndjson,
                csv_only: false,
                override_suffix: None,
            },
            DatasetSpec {
                id: "b",
                default_basename: "B_Out",
                framing: JsonFraming::Ndjson,
                csv_only: false,
                override_suffix: None,
            },
        ];
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("T", TWO, opts(tmp.path())).unwrap();
        r.set_identity(Identity::System);
        r.write("a", &Row { name: "x".into() }).unwrap();
        r.write("b", &Row { name: "y".into() }).unwrap();
        assert_eq!(r.finish().into_outcome().unwrap(), 2);
        assert!(tmp.path().join("csv/A_Out_system.csv").exists());
        assert!(tmp.path().join("csv/B_Out_system.csv").exists());
    }

    const MAIN_AND_TL: &[DatasetSpec] = &[
        DatasetSpec {
            id: "main",
            default_basename: "PETriage_Output",
            framing: JsonFraming::Ndjson,
            csv_only: false,
            override_suffix: None,
        },
        DatasetSpec {
            id: "timeline",
            default_basename: "PETriage_Output_Timeline",
            framing: JsonFraming::Ndjson,
            csv_only: true,
            override_suffix: Some("_Timeline"),
        },
    ];

    #[test]
    fn override_names_primary_verbatim_and_suffixes_secondary() {
        let tmp = tempfile::tempdir().unwrap();
        let mut o = opts(tmp.path());
        o.csvf = Some("custom.csv".into());
        let mut r = OutputRouter::new("PETriage", MAIN_AND_TL, o).unwrap();
        r.set_identity(Identity::System);
        r.write("main", &Row { name: "a".into() }).unwrap();
        r.write("timeline", &Row { name: "b".into() }).unwrap();
        r.finish().into_outcome().unwrap();
        assert!(tmp.path().join("csv/custom_system.csv").exists());
        assert!(tmp.path().join("csv/custom_Timeline_system.csv").exists());
    }

    #[test]
    fn csv_only_dataset_creates_no_json_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("PETriage", MAIN_AND_TL, opts(tmp.path())).unwrap();
        r.set_identity(Identity::System);
        r.write("main", &Row { name: "a".into() }).unwrap();
        r.write("timeline", &Row { name: "b".into() }).unwrap();
        r.finish().into_outcome().unwrap();
        assert!(tmp.path().join("json/PETriage_Output_system.json").exists());
        assert!(!tmp
            .path()
            .join("json/PETriage_Output_Timeline_system.json")
            .exists());
    }

    #[test]
    fn current_json_dir_returns_identity_dir_when_json_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let opts = RouterOptions {
            csv_root: None,
            json_root: Some(tmp.path().to_path_buf()),
            csvf: None,
            jsonf: None,
            pretty: false,
            overwrite: false,
            run_stamp: None,
            // current_json_dir derives the dir from the identity subdir, which
            // only exists in Nested mode; Flat would return <root> for every
            // identity, making this assertion meaningless.
            layout_mode: OutputLayoutMode::Nested,
        };
        let mut r = OutputRouter::new("SQLETriage", &[], opts).unwrap();
        r.set_identity(crate::attribution::Identity::User("alice".into()));
        let dir = r.current_json_dir().expect("json dir");
        assert!(dir.to_string_lossy().contains("alice"));
        // No CSV root configured -> current_csv_dir is None.
        assert!(r.current_csv_dir().is_none());
    }

    #[test]
    fn dynamic_filename_distinguishes_identities_in_flat_mode() {
        // Two identities with the same bare side-car name must resolve to
        // distinct filenames in Flat mode so they don't overwrite each other
        // at the shared root (the data-loss bug this fix targets).
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("RETriage", DATASETS, opts(tmp.path())).unwrap();

        r.set_identity(Identity::User("alice".into()));
        let alice = r.dynamic_filename("RecentDocs_NTUSER.DAT.csv");
        r.set_identity(Identity::User("bob".into()));
        let bob = r.dynamic_filename("RecentDocs_NTUSER.DAT.csv");

        assert_eq!(alice, "RecentDocs_NTUSER.DAT_alice.csv");
        assert_eq!(bob, "RecentDocs_NTUSER.DAT_bob.csv");
        assert_ne!(alice, bob);
    }

    #[test]
    fn dynamic_filename_keeps_bare_name_in_nested_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let mut o = opts(tmp.path());
        o.layout_mode = OutputLayoutMode::Nested;
        let mut r = OutputRouter::new("RETriage", DATASETS, o).unwrap();
        r.set_identity(Identity::User("alice".into()));
        assert_eq!(
            r.dynamic_filename("RecentDocs_NTUSER.DAT.csv"),
            "RecentDocs_NTUSER.DAT.csv"
        );
    }

    #[test]
    fn override_with_two_primary_datasets_is_still_usage_error() {
        const TWO_PRIMARY: &[DatasetSpec] = &[
            DatasetSpec {
                id: "a",
                default_basename: "A_Out",
                framing: JsonFraming::Ndjson,
                csv_only: false,
                override_suffix: None,
            },
            DatasetSpec {
                id: "b",
                default_basename: "B_Out",
                framing: JsonFraming::Ndjson,
                csv_only: false,
                override_suffix: None,
            },
        ];
        let tmp = tempfile::tempdir().unwrap();
        let mut o = opts(tmp.path());
        o.csvf = Some("x.csv".into());
        let err = OutputRouter::new("T", TWO_PRIMARY, o)
            .err()
            .expect("expected a Usage error");
        assert!(
            matches!(err, crate::error::TriageError::Usage(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn nested_layout_mode_preserves_legacy_router_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let mut o = opts(tmp.path());
        o.layout_mode = OutputLayoutMode::Nested;
        let mut r = OutputRouter::new("StubTriage", DATASETS, o).unwrap();
        r.set_identity(Identity::System);
        r.write("stub", &Row { name: "x".into() }).unwrap();
        r.finish().into_outcome().unwrap();
        assert!(tmp
            .path()
            .join("csv/StubTriage/system/StubTriage_Output.csv")
            .exists());
    }

    #[test]
    fn dynamic_rows_reuse_sink_and_reject_schema_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("T", &[], opts(tmp.path())).unwrap();
        r.set_identity(Identity::System);
        let headers = vec!["A".to_string(), "B".to_string()];
        r.write_dynamic_row("Runtime", &headers, &["1".into(), "2".into()])
            .unwrap();
        r.write_dynamic_row("Runtime", &headers, &["3".into(), "4".into()])
            .unwrap();
        let conflict = vec!["A".to_string(), "C".to_string()];
        assert!(r
            .write_dynamic_row("Runtime", &conflict, &["5".into(), "6".into()])
            .is_err());
        assert!(r.finish().into_outcome().is_err());
        assert!(!tmp.path().join("csv/Runtime_system.csv").exists());
    }

    #[test]
    fn outputs_are_invisible_until_finish_and_drop_cleans_temps() {
        let tmp = tempfile::tempdir().unwrap();
        let final_path = tmp.path().join("csv/StubTriage_Output_system.csv");
        {
            let mut r = OutputRouter::new("T", DATASETS, opts(tmp.path())).unwrap();
            r.set_identity(Identity::System);
            r.write("stub", &Row { name: "x".into() }).unwrap();
            assert!(!final_path.exists());
        }
        assert!(!final_path.exists());
        let leftovers = std::fs::read_dir(tmp.path().join("csv")).unwrap().count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn dynamic_csv_only_creates_no_json() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("T", &[], opts(tmp.path())).unwrap();
        r.set_identity(Identity::System);
        r.write_dynamic_csv_row("Detail", &["A".into()], &["x".into()])
            .unwrap();
        r.finish().into_outcome().unwrap();
        assert!(tmp.path().join("csv/Detail_system.csv").exists());
        assert!(!tmp.path().join("json/Detail_system.json").exists());
    }

    /// Rows written before and after `rekey_dynamic` all land in the file
    /// named after the *new* basename, in both CSV and NDJSON, with no
    /// leftover `.tmp-*` files -- the underlying writer is never reopened,
    /// only its eventual publish destination changes.
    #[test]
    fn rekey_dynamic_moves_rows_written_before_and_after_it_to_the_new_name() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("T", &[], opts(tmp.path())).unwrap();
        r.set_identity(Identity::System);
        let headers = vec!["A".to_string()];
        r.write_dynamic_row("Old", &headers, &["1".into()]).unwrap();
        assert!(r.rekey_dynamic("Old", "New").unwrap());
        r.write_dynamic_row("New", &headers, &["2".into()]).unwrap();
        r.finish().into_outcome().unwrap();

        let csv = std::fs::read_to_string(tmp.path().join("csv/New_system.csv")).unwrap();
        assert_eq!(csv.lines().count(), 3, "header + two rows: {csv}");
        assert!(!tmp.path().join("csv/Old_system.csv").exists());
        let json = std::fs::read_to_string(tmp.path().join("json/New_system.json")).unwrap();
        assert_eq!(json.lines().count(), 2, "two NDJSON rows: {json}");
        assert!(!tmp.path().join("json/Old_system.json").exists());

        let leftovers: Vec<_> = std::fs::read_dir(tmp.path().join("csv"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            leftovers,
            vec![std::ffi::OsString::from("New_system.csv")],
            "no orphaned temporary file: {leftovers:?}"
        );
    }

    /// A no-op basename (equal to itself, or naming no open entry) does not
    /// error and touches nothing.
    #[test]
    fn rekey_dynamic_is_a_no_op_for_an_absent_or_identical_basename() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("T", &[], opts(tmp.path())).unwrap();
        r.set_identity(Identity::System);
        assert!(!r.rekey_dynamic("Missing", "AlsoMissing").unwrap());

        r.write_dynamic_row("Existing", &["A".to_string()], &["1".into()])
            .unwrap();
        assert!(!r.rekey_dynamic("Existing", "Existing").unwrap());
        r.finish().into_outcome().unwrap();
        assert!(tmp.path().join("csv/Existing_system.csv").exists());
    }

    /// Rekeying onto a basename that already has its own live entry must be
    /// refused, not silently clobbered -- an insert over it would drop that
    /// entry's unflushed writer and orphan its temporary file, reachable by
    /// neither `finish()` nor `Drop` afterward. Both original entries must
    /// survive the refused rekey attempt untouched.
    #[test]
    fn rekey_dynamic_refuses_to_clobber_an_existing_target_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("T", &[], opts(tmp.path())).unwrap();
        r.set_identity(Identity::System);
        let headers = vec!["A".to_string()];
        r.write_dynamic_row("Alpha", &headers, &["1".into()])
            .unwrap();
        r.write_dynamic_row("Beta", &headers, &["2".into()])
            .unwrap();

        assert!(
            r.rekey_dynamic("Alpha", "Beta").is_err(),
            "must refuse to move onto an already-occupied basename"
        );

        r.finish().into_outcome().unwrap();
        assert!(tmp.path().join("csv/Alpha_system.csv").exists());
        assert!(tmp.path().join("csv/Beta_system.csv").exists());
        let alpha = std::fs::read_to_string(tmp.path().join("csv/Alpha_system.csv")).unwrap();
        assert!(alpha.contains('1'), "Alpha's own row must survive: {alpha}");
        let beta = std::fs::read_to_string(tmp.path().join("csv/Beta_system.csv")).unwrap();
        assert!(beta.contains('2'), "Beta's own row must survive: {beta}");
    }

    /// A rekey whose new basename resolves to a different parent directory
    /// must be refused rather than left to fail obscurely inside
    /// `finish()`'s rename. The original entry must survive untouched.
    #[test]
    fn rekey_dynamic_refuses_a_cross_directory_move() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("T", &[], opts(tmp.path())).unwrap();
        r.set_identity(Identity::System);
        r.write_dynamic_row("Dir1/Name", &["A".to_string()], &["1".into()])
            .unwrap();

        assert!(
            r.rekey_dynamic("Dir1/Name", "Dir2/Name").is_err(),
            "must refuse a move across directories"
        );

        r.finish().into_outcome().unwrap();
        assert!(tmp.path().join("csv/Dir1/Name_system.csv").exists());
        assert!(!tmp.path().join("csv/Dir2").exists());
    }
}
