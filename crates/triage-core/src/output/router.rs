use crate::attribution::Identity;
use crate::error::TriageError;
use crate::output::dataset::{CsvSink, DatasetSpec, JsonSink};
use crate::output::layout::{OutputLayout, OutputLayoutMode};
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

/// Output filename for a dataset: the default basename, or the user override
/// (primary dataset gets it verbatim; suffixed datasets get `{stem}{suffix}{ext}`).
fn dataset_filename(
    spec: &DatasetSpec,
    override_name: &Option<String>,
    ext: &str,
    run_stamp: &Option<String>,
) -> String {
    match (override_name, spec.override_suffix) {
        // Default name: prepend the run stamp (Zimmerman convention), e.g.
        // `20260616184308_EvtxTriage_Output.csv`.
        (None, _) => match run_stamp {
            Some(stamp) => format!("{stamp}_{}.{ext}", spec.default_basename),
            None => format!("{}.{ext}", spec.default_basename),
        },
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

    /// Final destinations currently opened by this run.
    pub fn output_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for files in self.open.values() {
            if let Some((_, _, path)) = &files.csv {
                paths.push(path.clone());
            }
            if let Some((_, _, path)) = &files.json {
                paths.push(path.clone());
            }
        }
        for files in self.dynamic.values() {
            if let Some((_, _, path)) = &files.csv {
                paths.push(path.clone());
            }
            if let Some((_, _, path)) = &files.json {
                paths.push(path.clone());
            }
        }
        paths.sort();
        paths.dedup();
        paths
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
                    let name = dataset_filename(spec, &self.csvf, "csv", &self.run_stamp);
                    let staged = layout.create_staged(&self.identity, &name)?;
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
                        let name = dataset_filename(spec, &self.jsonf, "json", &self.run_stamp);
                        layout.create_staged(&self.identity, &name).map(|staged| {
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

    /// Return the current output identity (set by the runner via set_identity).
    /// Used by tools that write side-car files co-located with the main output.
    pub fn current_identity(&self) -> &Identity {
        &self.identity
    }

    /// Flush and close every open file; returns total records written.
    /// Attempts all sinks even if an earlier one fails (best-effort flush).
    pub fn finish(mut self) -> Result<u64, TriageError> {
        if self.failed {
            self.cleanup_temporaries();
            return Err(TriageError::Output {
                path: PathBuf::from("<output-router>"),
                message: "output transaction aborted after an earlier write failure".into(),
            });
        }
        let mut first_err: Option<TriageError> = None;

        for (_, files) in self.open.drain() {
            if let Some((csv, temporary, path)) = files.csv {
                if let Err(e) = csv.finish() {
                    let _ = std::fs::remove_file(&temporary);
                    first_err.get_or_insert(TriageError::Output {
                        path: path.clone(),
                        message: e.to_string(),
                    });
                } else if let Err(e) = publish(&temporary, &path, self.overwrite) {
                    first_err.get_or_insert(e);
                }
            }
            if let Some((json, temporary, path)) = files.json {
                if let Err(e) = json.finish() {
                    let _ = std::fs::remove_file(&temporary);
                    first_err.get_or_insert(TriageError::Output {
                        path: path.clone(),
                        message: e.to_string(),
                    });
                } else if let Err(e) = publish(&temporary, &path, self.overwrite) {
                    first_err.get_or_insert(e);
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
                } else if let Err(e) = publish(&temporary, &path, self.overwrite) {
                    first_err.get_or_insert(e);
                }
            }
            if let Some((json, temporary, path)) = files.json {
                if let Err(e) = json.finish() {
                    let _ = std::fs::remove_file(&temporary);
                    first_err.get_or_insert(TriageError::Output {
                        path: path.clone(),
                        message: e.to_string(),
                    });
                } else if let Err(e) = publish(&temporary, &path, self.overwrite) {
                    first_err.get_or_insert(e);
                }
            }
        }

        if let Some(err) = first_err {
            return Err(err);
        }
        Ok(self.records)
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
    fn routes_csv_and_json_per_identity_from_same_records() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = OutputRouter::new("StubTriage", DATASETS, opts(tmp.path())).unwrap();

        r.set_identity(Identity::User("alice".into()));
        r.write("stub", &Row { name: "a".into() }).unwrap();
        r.set_identity(Identity::System);
        r.write("stub", &Row { name: "b".into() }).unwrap();
        let total = r.finish().unwrap();
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
        r.finish().unwrap();
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
        let total = r.finish().unwrap();
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
        assert_eq!(r.finish().unwrap(), 2);
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
        r.finish().unwrap();
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
        r.finish().unwrap();
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
        r.finish().unwrap();
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
        assert!(r.finish().is_err());
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
        r.finish().unwrap();
        assert!(tmp.path().join("csv/Detail_system.csv").exists());
        assert!(!tmp.path().join("json/Detail_system.json").exists());
    }
}
