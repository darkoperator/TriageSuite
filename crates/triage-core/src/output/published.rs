//! What an `OutputRouter` actually put on disk this run.
//!
//! The router is the only place that knows which dataset and which identity
//! produced a given path: it is what applies `dataset_filename`,
//! `velo_basename` and the identity prefix. A consumer that wants that
//! association has to be handed it, because recovering it from the filename
//! afterwards means reimplementing all three and drifting from them in
//! silence.

use crate::attribution::Identity;
use std::path::PathBuf;

/// Which writer produced a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Csv,
    Json,
}

/// Which dataset a file belongs to.
///
/// `Static` is a `DatasetSpec::id`, known at compile time. `Dynamic` is the
/// runtime basename passed to `write_dynamic_row` by the three tools whose
/// schema only exists at run time (`evtx-triage`, `re-triage`, `sqle-triage`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatasetKey {
    Static(&'static str),
    Dynamic(String),
}

impl DatasetKey {
    pub fn as_str(&self) -> &str {
        match self {
            DatasetKey::Static(id) => id,
            DatasetKey::Dynamic(name) => name,
        }
    }
}

/// One destination whose staged file `finish()` renamed into place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedFile {
    pub path: PathBuf,
    pub format: OutputFormat,
    pub dataset: DatasetKey,
    pub identity: Identity,
}
