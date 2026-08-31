//! The `ExternalTool` abstraction: what an external binary must tell the
//! orchestrator so the driver can run it without knowing which tool it is.
//!
//! This mirrors `crate::registry`'s in-process `Tool` registry in shape — a fixed
//! compile-time table, one stable key per entry — but it deliberately generalizes
//! *execution* only, not *configuration*. Each tool keeps a typed, named field on
//! `ResolvedConfig`; see the design doc for why routing tables through
//! `toml::Value` would cost `deny_unknown_fields` its spans and its
//! unknown-table check.

use super::config::{ConfigError, ResolvedConfig};
use crate::capture::HostCapture;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Who creates an invocation's output directory, and under what precondition.
/// External tools genuinely disagree about this, so it can't be one blanket
/// `create_dir_all` in the driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputDirPolicy {
    /// `create_dir_all(work_dir)` before spawning; pre-existing content is fine.
    /// Hayabusa writes exactly-named files and owns `--clobber`.
    CreateIfMissing,
    /// Create only `work_dir.parent()`, never the leaf. Takajo's `automagic -o`
    /// creates its own leaf directory and refuses to run if it already exists
    /// ("Please specify a new folder name").
    ToolCreatesLeaf,
}

/// How the driver discovers what an invocation actually produced, once it exits
/// successfully. A zero exit code alone never counts as output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputSpec {
    /// A single known path, reported iff it exists.
    Path(PathBuf),
    /// Files directly under `dir` whose basename starts with `prefix`, sorted.
    /// Hayabusa's `logon-summary` writes a variable number of `<prefix>-*.csv`
    /// files — none at all if it found nothing to summarize — and the exact
    /// suffixes are deliberately not hardcoded anywhere in this crate.
    PrefixedIn { dir: PathBuf, prefix: String },
}

/// An output this invocation makes available to tools later in registry order.
/// Published only if the path exists as a file afterwards, checked independently
/// of the exit status: a tool can report success and still write nothing.
#[derive(Debug, Clone)]
pub struct Publish {
    /// Fully-qualified slot name, by convention `"<tool-key>.<what>"`,
    /// e.g. `"hayabusa.jsonl"`.
    pub slot: &'static str,
    pub path: PathBuf,
}

/// A precondition on an earlier tool's published artifact.
///
/// Checked *before* the binary is resolved, which is load-bearing: a tool whose
/// prerequisite is missing reports `found: true` and an explanatory skip, rather
/// than "not found on PATH", even when its binary is also absent.
#[derive(Debug, Clone)]
pub struct Requirement {
    pub slot: &'static str,
    pub report_name: &'static str,
    pub skipped_message: &'static str,
}

/// One subprocess launch, fully declarative — the driver executes it.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// Name used in the run manifest and on the console, e.g. `"hayabusa-csv"`.
    pub report_name: &'static str,
    pub args: Vec<OsString>,
    /// The directory this invocation writes into.
    pub work_dir: PathBuf,
    pub dir_policy: OutputDirPolicy,
    pub outputs: OutputSpec,
    pub publishes: Option<Publish>,
}

/// Per-host paths handed to `plan()`.
///
/// `host_dir` is computed once, by the driver, as `out_root.join(&host.output_id)`
/// — the single place the output_id-not-hostname invariant is enforced. A repeated
/// hostname gets a stable per-collection directory, so no tool module may build
/// this path from `host.host` itself.
pub struct HostContext<'a> {
    pub host: &'a HostCapture,
    pub host_dir: PathBuf,
}

/// Artifacts published by tools earlier in registry order.
#[derive(Debug, Default)]
pub struct Artifacts(HashMap<&'static str, PathBuf>);

impl Artifacts {
    pub fn get(&self, slot: &str) -> Option<&Path> {
        self.0.get(slot).map(PathBuf::as_path)
    }

    pub(super) fn publish(&mut self, slot: &'static str, path: PathBuf) {
        self.0.insert(slot, path);
    }
}

/// One external subprocess tool.
///
/// Implementors are unit structs: they hold no per-run state, so the registry can
/// be a `const` table. All state lives in `ResolvedConfig` and `Artifacts`.
pub trait ExternalTool: Sync {
    /// Stable short key: the `[<key>]` TOML table, the `--skip <key>` name, and
    /// the report name used when the binary can't be found. Must be unique in
    /// this registry and disjoint from `crate::registry`'s in-process keys —
    /// both enforced by a unit test.
    fn key(&self) -> &'static str;

    fn enabled(&self, cfg: &ResolvedConfig) -> bool;

    /// Force-disable for this run, used by `--skip <key>`.
    fn disable(&self, cfg: &mut ResolvedConfig);

    /// The configured binary name or path, handed to `resolve_bin`.
    fn bin<'a>(&self, cfg: &'a ResolvedConfig) -> &'a str;

    /// Validate this tool's config against the fully-merged, post-profile
    /// config. Runs once at `ExternalConfig::resolve` time, so a combination
    /// that can never be satisfied fails before any evidence is touched.
    fn validate(&self, _cfg: &ResolvedConfig) -> Result<(), ConfigError> {
        Ok(())
    }

    /// What this tool needs from an earlier tool's output.
    fn requires(&self) -> Option<Requirement> {
        None
    }

    /// Slots this tool's `plan()` may publish. Declarative, and consulted only by
    /// the registry's ordering test — `plan()` remains the source of truth at
    /// runtime.
    fn publishable_slots(&self) -> &'static [&'static str] {
        &[]
    }

    /// The invocations to attempt for one host, in order. Called only when the
    /// tool is enabled, its `requires()` slot (if any) is satisfied, and its
    /// binary resolved.
    fn plan(
        &self,
        cfg: &ResolvedConfig,
        ctx: &HostContext<'_>,
        prior: &Artifacts,
    ) -> Vec<Invocation>;
}
