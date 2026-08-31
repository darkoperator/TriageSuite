use crate::error::TriageError;
use crate::output::dataset::DatasetSpec;
use crate::output::router::OutputRouter;
use std::path::Path;

/// Whether a tool's artifacts belong to a user or to the system when the
/// path carries no user information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Unattributable artifacts land in users/unknown.
    UserSpecific,
    /// All artifacts land in system (e.g. Prefetch).
    SystemWide,
    /// Attribute to the in-capture user when derivable; otherwise system
    /// (e.g. LNK files: user profile -> users/<u>, else system-wide).
    UserElseSystem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validation {
    Supported,
    Unsupported { reason: String },
    Corrupt { reason: String },
    Unreadable { error: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceClass {
    Light,
    Heavy,
}

/// One forensic parser. The shared runner drives discovery, validation,
/// attribution, output routing, the summary, and exit codes; the tool
/// supplies only what is artifact-specific.
pub trait Tool {
    /// Binary name as shown in output paths and the banner, e.g. "PETriage".
    fn binary_name(&self) -> &'static str;

    /// Case-insensitive filename globs used in directory mode.
    /// Patterns match the filename component only.
    fn patterns(&self) -> &[&'static str];

    /// Content validation (magic/structure), never extension-only (spec 3.2).
    fn validate_legacy(&self, path: &Path) -> bool;

    fn validate(&self, path: &Path) -> Validation {
        if let Err(error) = std::fs::File::open(path) {
            return Validation::Unreadable {
                error: error.to_string(),
            };
        }
        if self.validate_legacy(path) {
            Validation::Supported
        } else if self.invalid_content_is_corrupt() {
            Validation::Corrupt {
                reason: "matching artifact failed content or structure validation".into(),
            }
        } else {
            Validation::Unsupported {
                reason: "content or structure is not supported by this parser".into(),
            }
        }
    }

    /// True when a filename match denotes an artifact strongly enough that a
    /// failed content check should be audited as corruption, not a skip.
    fn invalid_content_is_corrupt(&self) -> bool {
        false
    }

    /// Whether the orchestrator may skip an artifact whose content is
    /// byte-identical to one already parsed for this tool on this host.
    ///
    /// True for almost everything: two identical copies of the same `.pf` or
    /// hive are the same evidence collected twice, and parsing both would
    /// duplicate every row. This is the Zimmerman-compatible default.
    ///
    /// Return false when the artifact's *location* is itself part of what the
    /// tool reports, so that two identical files at different paths are two
    /// distinct findings rather than one. BrowserTriage is the case: it emits
    /// a `Profile` column, and a browser update leaves `Snapshots/<version>`
    /// copies byte-identical to the live profile's. Deduplicating those keeps
    /// the content but silently drops the second profile's attribution, which
    /// makes "which profiles held this extension" unanswerable.
    fn dedupe_by_content(&self) -> bool {
        true
    }

    /// The datasets this tool can emit.
    fn datasets(&self) -> &'static [DatasetSpec];

    fn scope(&self) -> Scope;

    fn resource_class(&self) -> ResourceClass {
        ResourceClass::Light
    }

    /// Parse one validated artifact, writing records through the router.
    /// Returns the number of records emitted for this artifact.
    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError>;
}
