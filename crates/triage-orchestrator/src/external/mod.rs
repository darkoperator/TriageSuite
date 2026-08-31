//! External third-party forensic binaries the orchestrator shells out to, per
//! host, after that host's in-process tools finish.
//!
//! Unlike `crate::registry`'s in-process `Tool` parsers, these are separately
//! maintained programs TriageSuite only orchestrates: it resolves the binary,
//! builds its argv, runs it, and folds the exit status and output paths into the
//! run manifest. It never reimplements or reinterprets their output.

pub mod config;
pub mod registry;
pub mod tool;
pub mod tools;

mod args;
mod driver;
mod invoke;
mod report;

pub use config::{ConfigError, ExternalConfig, ResolvedConfig};
pub use driver::run_external_tools_for_host;
pub use report::ExternalToolReport;
