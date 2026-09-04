//! TriageSuite orchestrator: detect a capture, run every parser over it,
//! fan out across hosts, and emit a chain-of-custody manifest.

pub mod archive;
pub mod capture;
pub mod execute;
pub mod external;
pub mod input;
pub mod manifest;
pub mod progress_ui;
pub mod registry;

/// How many per-file skip reasons a report keeps. Enough to diagnose a
/// pattern, few enough that a run over thousands of unsupported files still
/// produces a readable manifest.
pub(crate) const MAX_REASON_SAMPLES: usize = 10;

/// The final path component, lossily decoded. Empty when the path has none
/// (`/`, `..`).
pub fn file_name_lossy(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
