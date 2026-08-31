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
