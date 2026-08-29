//! EvtxTriage: EvtxECmd-compatible Windows .evtx event-log parser.

pub mod cli;
pub mod maps_embed;
pub mod sync;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};
use triage_evtx::{MapIndex, ParseOptions};

pub const DATASETS: &[DatasetSpec] = &[DatasetSpec {
    id: "events",
    default_basename: "EvtxTriage_Output",
    framing: JsonFraming::Ndjson,
    csv_only: false,
    override_suffix: None,
}];

pub struct EvtxTool {
    pub maps: MapIndex,
    pub opts: ParseOptions,
    /// Additive: when true, every event is also written to a per-source-file dataset,
    /// alongside (not instead of) the combined "events" dataset.
    pub split: bool,
    /// Used output stems for `--split` collision handling across per-file parse calls.
    pub used_stems: Mutex<HashSet<String>>,
}

impl Default for EvtxTool {
    fn default() -> Self {
        EvtxTool {
            maps: crate::maps_embed::load_bundled(),
            opts: ParseOptions::new(),
            split: false,
            used_stems: Mutex::new(HashSet::new()),
        }
    }
}

impl EvtxTool {
    /// Allocate an unused output stem for a source path, suffixing `_1`, `_2`, …
    /// on collision across the runner's per-file parse calls.
    fn allocate_stem(&self, path: &Path) -> String {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output")
            .to_string();
        let mut used = self.used_stems.lock().unwrap();
        if used.insert(stem.clone()) {
            return stem;
        }
        let mut n = 1u32;
        loop {
            let candidate = format!("{stem}_{n}");
            if used.insert(candidate.clone()) {
                return candidate;
            }
            n += 1;
        }
    }
}

impl Tool for EvtxTool {
    fn binary_name(&self) -> &'static str {
        "EvtxTriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &["*.evtx"]
    }

    fn validate_legacy(&self, path: &Path) -> bool {
        use std::io::Read;
        let mut buf = [0u8; 8];
        match std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut buf)) {
            // .evtx files begin with the ASCII signature "ElfFile\0".
            Ok(()) => &buf == b"ElfFile\0",
            Err(_) => false,
        }
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

    fn resource_class(&self) -> triage_core::tool::ResourceClass {
        triage_core::tool::ResourceClass::Heavy
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        let to_err = |e: triage_evtx::EvtxTriageError| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        };

        // --split is additive, not exclusive: every event always goes to the aggregate
        // "events" dataset, and also to a per-source-file dataset when --split is set.
        let split_stem = self.split.then(|| self.allocate_stem(path));
        let mut count = 0u64;
        match triage_evtx::visit_evtx_file(path, &self.opts, &self.maps, &mut |rec| {
            out.write("events", &rec)?;
            if let Some(stem) = &split_stem {
                out.write_dynamic_record(stem, &rec)?;
            }
            count += 1;
            Ok::<(), TriageError>(())
        }) {
            Ok(()) => {}
            Err(triage_evtx::VisitError::Parse(error)) => return Err(to_err(error)),
            Err(triage_evtx::VisitError::Visitor(error)) => return Err(error),
        }
        Ok(count)
    }
}
