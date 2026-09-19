//! VeloProcessor-shaped output layout: a per-collection `Processed-<HOST>-<stamp>`
//! directory containing forensic-category subdirectories.
//!
//! `triage-core` knows nothing about forensic categories; it receives a root
//! that already points at `Processed-<HOST>-<stamp>/<Category>` and applies
//! `OutputLayoutMode::Velo` within it.

use std::path::{Path, PathBuf};

pub mod hashes;
pub mod merge;
pub mod proclog;
pub mod sessions;
pub mod sysinfo;
pub mod veloresults;

/// Which output tree `TriageSuite run` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Layout {
    /// VeloProcessor-shaped category tree (default).
    #[default]
    Velo,
    /// The per-tool, per-identity tree (`<out>/<HOST>/<Tool>/<identity>/`).
    Native,
}

/// The forensic category directory a tool's output belongs in.
///
/// `RemoteAccess` and `SQLiteArtifacts` have no VeloProcessor equivalent:
/// VeloProcessor runs no tool that produces that output, so there is no name
/// to match.
pub fn category_for_key(key: &str) -> &'static str {
    match key {
        "mft" | "pe" | "le" | "jle" | "rb" => "FileSystem",
        "re" | "sbe" | "amc" | "acc" => "Registry",
        "evtx" => "EventLogs",
        "srum" | "sum" | "wxt" | "srumnet" => "SystemActivity",
        "browser" => "BrowserActivity",
        "lol" => "ThreatHunting",
        "anydesk" => "RemoteAccess",
        "sqle" => "SQLiteArtifacts",
        // A new tool must be classified here. Defaulting to a catch-all would
        // hide the omission until an analyst went looking for the output.
        other => panic!("no Velo category for tool key {other}"),
    }
}

/// One dataset's per-user output in a category: the filename stem its slices
/// carry, and the `PerUser/` directory they are written into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryStem {
    /// `<stamp>_<velo_basename>` -- the prefix every one of this dataset's
    /// per-user filenames starts with, and the name of its merged file.
    pub stem: String,
    /// The dataset's Velo discriminator, which is the level below `PerUser/`
    /// its slices go in, or `None` for a dataset that has none and writes
    /// straight into `PerUser/`
    /// (`triage_core::output::layout::OutputLayout::for_velo_dataset`).
    pub dataset_dir: Option<String>,
}

/// Every Velo dataset stem in `category`, with the `PerUser/` directory each
/// one's slices are written into.
///
/// Production does not need this: the merge is told its own dataset's
/// directory by the caller that ran the tool, and within one directory a
/// per-user filename decodes by stripping a `<stem>_` prefix. What needs it
/// is the guard that keeps that true --
/// `every_per_user_directory_decodes_to_exactly_one_stem`
/// (`tests/velo_names.rs`) -- which has to see every stem a *category* can
/// produce, not just one tool's, because `PerUser/` is one directory tree per
/// category: two different tools' stems can share a directory, and that is
/// the one way a per-user name could still be claimable by two datasets.
/// Deriving the set from the registry rather than from a calling tool is what
/// makes a dataset -- or a whole tool -- added later covered without anyone
/// having to remember this.
///
/// A registry key whose tool cannot be built with default options
/// contributes nothing; the guard asserts the total is non-zero so that a
/// registry which stopped producing stems fails rather than passing
/// vacuously.
pub fn category_stems(category: &str, stamp: &str) -> Vec<CategoryStem> {
    let mut stems = Vec::new();
    for key in crate::registry::all_keys() {
        if category_for_key(key) != category {
            continue;
        }
        let Some(tool) = crate::registry::tool_for_key(key) else {
            continue;
        };
        for spec in tool.datasets() {
            stems.push(CategoryStem {
                stem: format!(
                    "{stamp}_{}",
                    triage_core::output::router::velo_basename(tool.binary_name(), spec)
                ),
                dataset_dir: triage_core::output::router::velo_discriminator(
                    tool.binary_name(),
                    spec,
                ),
            });
        }
    }
    stems
}

/// `Processed-<output_id>-<stamp>`.
pub fn collection_dir(output_id: &str, stamp: &str) -> String {
    format!("Processed-{output_id}-{stamp}")
}

/// `<out>/Processed-<output_id>-<stamp>/<category>`.
pub fn velo_root(out_root: &Path, output_id: &str, stamp: &str, category: &str) -> PathBuf {
    out_root
        .join(collection_dir(output_id, stamp))
        .join(category)
}

/// `Some(<root>/Processed-<output_id>-<stamp>)` under `Layout::Velo`, `None`
/// under `Layout::Native`.
///
/// Shared by `execute.rs` (process logs) and `main.rs` (process logs' shared
/// directory, plus the existing chain-of-custody hash log) so the
/// "process logs are a Velo-layout concept" gate lives in exactly one place
/// — `execute.rs` and `main.rs` each computing this independently was a
/// drift risk: they only agreed because `csv_root`/`json_root` both happen
/// to equal `args.out` today (`main.rs`'s `OutputOpts` construction).
pub fn collection_dir_for(
    layout: Layout,
    root: &Path,
    output_id: &str,
    stamp: &str,
) -> Option<PathBuf> {
    (layout == Layout::Velo).then(|| root.join(collection_dir(output_id, stamp)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registry_key_has_a_category() {
        for key in crate::registry::all_keys() {
            let category = category_for_key(key);
            assert!(!category.is_empty(), "no category for tool key {key}");
        }
        // Second-pass tools are not in ALL_KEYS but still produce output.
        assert_eq!(category_for_key("lol"), "ThreatHunting");
        assert_eq!(category_for_key("srumnet"), "SystemActivity");
        assert_eq!(category_for_key("anydesk"), "RemoteAccess");
    }

    #[test]
    fn categories_match_the_spec() {
        assert_eq!(category_for_key("mft"), "FileSystem");
        assert_eq!(category_for_key("pe"), "FileSystem");
        assert_eq!(category_for_key("le"), "FileSystem");
        assert_eq!(category_for_key("jle"), "FileSystem");
        assert_eq!(category_for_key("rb"), "FileSystem");
        assert_eq!(category_for_key("re"), "Registry");
        assert_eq!(category_for_key("sbe"), "Registry");
        assert_eq!(category_for_key("amc"), "Registry");
        assert_eq!(category_for_key("acc"), "Registry");
        assert_eq!(category_for_key("evtx"), "EventLogs");
        assert_eq!(category_for_key("srum"), "SystemActivity");
        assert_eq!(category_for_key("sum"), "SystemActivity");
        assert_eq!(category_for_key("wxt"), "SystemActivity");
        assert_eq!(category_for_key("browser"), "BrowserActivity");
        assert_eq!(category_for_key("sqle"), "SQLiteArtifacts");
    }

    #[test]
    fn velo_root_is_the_collection_dir_then_the_category() {
        let root = velo_root(
            std::path::Path::new("/out"),
            "WS01",
            "2026-03-13T192553Z",
            "FileSystem",
        );
        assert_eq!(
            root,
            std::path::Path::new("/out/Processed-WS01-2026-03-13T192553Z/FileSystem")
        );
    }
}
