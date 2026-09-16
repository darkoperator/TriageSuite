//! Guards every pattern in the bundled `TimelineExplorerSessions.json`
//! against filenames derived from each tool's *real* naming logic, not from
//! re-reading the pattern itself.
//!
//! This exists because of a real bug: four patterns in that config were
//! written from a plausible-looking guess at `velo_basename`'s output
//! (`Registry/*_RETriage_results.csv`, `Registry/*_AppCompatTriage_results.csv`,
//! `FileSystem/*_JLETriage_results.csv`, `Registry/*_SBETriage_results.csv`)
//! and none of them could ever match anything, because every one of those
//! tools' primary dataset carries a discriminator suffix
//! (`..._results_Batch.csv`, `..._results_AppCompatCache.csv`, etc.) that a
//! bare `_results.csv` pattern excludes. Two more
//! (`Registry/*_RETriage_results_Services.csv`/`_TaskCache.csv`) assumed a
//! shape RETriage's per-plugin detail files don't have at all -- those are
//! dynamic side-cars named `<PluginName>_<HiveStem>.csv` with no `RETriage`,
//! `results`, or run-stamp in the name (`crates/re-triage/src/lib.rs`, the
//! per-plugin detail-CSV writer). `Persistence`, the session naming all
//! three RETriage patterns, matched zero files on every real run as a
//! result -- confirmed against a real capture
//! (`Collection-DESKTOP-OA8SHHC-2026-03-12T22_54_56Z`) before this test
//! existed.
//!
//! It happened a second time, to the external tools, and this test did not
//! see it: its corpus named `EventLogs/*_Hayabusa_timeline.csv` and
//! `ThreatHunting/*_Takajo_automagic/*.csv`, which is what the config said
//! rather than what the binaries do. Hayabusa writes the exact `--output`
//! path it is handed (`EventLogs/timeline.csv`) and Takajo's `automagic -o`
//! writes its report files straight into the directory it is handed
//! (`ThreatHunting/`), not into a per-run subdirectory of its own. Restating
//! the config in the corpus makes the corpus agree with any config, so it can
//! never contradict one.
//!
//! So the corpus is built two ways, neither of them a guess:
//!
//! * Everything this tree names, from the naming code itself -- each `Tool`'s
//!   own `binary_name()`/`datasets()` run through the real `velo_basename()`.
//! * Everything a third-party binary names, from
//!   `tests/data/external_tools_observed_output.txt`, transcribed from a run
//!   of the real Hayabusa and Takajo. No in-repo function names those files,
//!   so observation is the only source there is;
//!   `observed_external_output_agrees_with_the_paths_this_crate_hands_the_binaries`
//!   ties that evidence back to the paths `plan()` actually passes them, so
//!   the fixture cannot drift from this tree unnoticed.
//!
//! The two RETriage per-plugin side-cars and the per-channel EVTX export are
//! still literals: this tree names them, but not through a `DatasetSpec`, so
//! there is no basename function to call. Both were re-observed on the
//! `Collection-DESKTOP-OA8SHHC-2026-03-12T22_54_56Z` run that produced the
//! external-tool listing.

use globset::GlobSetBuilder;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use triage_core::attribution::Identity;
use triage_core::output::layout::{OutputLayout, OutputLayoutMode};
use triage_core::output::router::{velo_basename, velo_discriminator};
use triage_core::tool::Tool;
use triage_orchestrator::capture::HostCapture;
use triage_orchestrator::external::tool::{Artifacts, ExternalTool, HostContext, OutputSpec};
use triage_orchestrator::external::tools::{hayabusa::Hayabusa, takajo};
use triage_orchestrator::external::ResolvedConfig;

const CONFIG: &str = include_str!("../../../resources/velo/TimelineExplorerSessions.json");

/// Collection-relative paths the real Hayabusa and Takajo binaries were
/// observed writing, one per line, `#` comments and blank lines ignored.
/// See the file's own header for the run it was transcribed from and how to
/// re-observe it.
const OBSERVED_EXTERNAL_OUTPUT: &str = include_str!("data/external_tools_observed_output.txt");

/// Stand-in collection directory for the `--layout velo` paths `plan()`
/// builds. Absolute so `HostContext::velo_dir` is shaped like a real one;
/// nothing is created on disk, only joined and stripped back off.
const COLLECTION: &str = "/collection/Processed-HOSTX-2026-01-01T000000Z";

/// The observed external-tool paths, comments and blanks stripped.
fn observed_external_paths() -> Vec<&'static str> {
    OBSERVED_EXTERNAL_OUTPUT
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// `<COLLECTION>/x` back to `x`, forward-slashed, for comparing a path
/// `plan()` built against a line of the observed listing.
fn collection_relative(path: &Path) -> String {
    path.strip_prefix(COLLECTION)
        .unwrap_or_else(|_| panic!("{} is not under {COLLECTION}", path.display()))
        .to_string_lossy()
        .replace('\\', "/")
}

/// A `HostContext` shaped the way the driver builds one under `--layout
/// velo`: `velo_dir` set, which is what makes each external tool join its
/// forensic category rather than its `--layout native` subdirectory.
fn velo_host_context() -> (HostCapture, PathBuf) {
    let host = HostCapture {
        host: "HOSTX".to_string(),
        output_id: "HOSTX".to_string(),
        os: "Windows".to_string(),
        collection_dir: PathBuf::from("/capture/Collection-HOSTX"),
        artifact_root: PathBuf::from("/capture/Collection-HOSTX/uploads"),
        source_archive: None,
    };
    (host, PathBuf::from(COLLECTION))
}

#[derive(serde::Deserialize)]
struct SessionConfig {
    #[serde(rename = "Sessions")]
    sessions: Vec<SessionDef>,
}

#[derive(serde::Deserialize)]
struct SessionDef {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "FilePatterns")]
    file_patterns: Vec<String>,
}

/// Fixed stamp standing in for the real `<yyyy-MM-ddTHHmmssZ>` run stamp:
/// only its presence (not its value) matters to a pattern's leading `*`.
const STAMP: &str = "2026-01-01T000000Z";

/// Every `DatasetSpec`-routed tool referenced by the bundled config, keyed
/// by its `registry::all_keys()` key (or a standalone binary's own key for
/// the second-pass tools `lol`/`srumnet`, which never appear in `ALL_KEYS`
/// but are constructed directly here the same way
/// `velo_names.rs::standalone_tools` does).
fn dataset_backed_tools() -> Vec<(&'static str, Box<dyn Tool>)> {
    let mut tools: Vec<(&'static str, Box<dyn Tool>)> = Vec::new();
    for key in [
        "pe", "jle", "le", "rb", "re", "sbe", "srum", "amc", "acc", "browser",
    ] {
        let tool = triage_orchestrator::registry::tool_for_key(key)
            .unwrap_or_else(|| panic!("registry key {key} builds no tool"));
        tools.push((key, tool));
    }
    tools.push((
        "lol",
        Box::new(lol_triage::LolTool {
            refs: lol_triage::refdata::LolRefs::new(Vec::new(), Vec::new()),
        }),
    ));
    tools.push((
        "srumnet",
        Box::new(srum_net_triage::SrumNetTool {
            tz: srum_net_triage::aggregate::TzOffset(0),
            business_hours: "08:00-18:00".parse().expect("valid business-hours literal"),
        }),
    ));
    tools
}

/// Every relative path (collection-directory-relative, forward-slashed) a
/// real run could plausibly produce, built from real naming logic rather
/// than from the config under test.
fn realistic_corpus() -> Vec<String> {
    let mut corpus = Vec::new();

    for (key, tool) in dataset_backed_tools() {
        let category = triage_orchestrator::velo::category_for_key(key);
        let binary = tool.binary_name();
        for spec in tool.datasets() {
            let basename = velo_basename(binary, spec);
            corpus.push(format!("{category}/{STAMP}_{basename}.csv"));
        }
    }

    // RETriage's per-plugin detail CSVs are dynamic side-cars, not
    // `DatasetSpec`s (see the module doc above): `<PluginName>_<HiveStem>`,
    // no tool name, no `results`, no stamp. Confirmed on a real capture as
    // `Registry/Services_SYSTEM.csv` and `Registry/TaskCache_SOFTWARE.csv`
    // (TaskCache reads the SOFTWARE hive, not SYSTEM).
    corpus.push("Registry/Services_SYSTEM.csv".to_string());
    corpus.push("Registry/TaskCache_SOFTWARE.csv".to_string());

    // `evtx-triage`'s per-channel export (`--only evtx` without
    // `--no-individual`) is named by this tree but not through a
    // `DatasetSpec` either: `EventLogs/Individual/<Channel>.csv`, the
    // channel being whatever the host logged. Confirmed on a real capture.
    corpus.push("EventLogs/Individual/Security.csv".to_string());

    // Everything the external tools put on disk, from a transcript of a real
    // run. This tree names two of those paths itself -- Hayabusa's
    // `EventLogs/timeline.{csv,jsonl}`, joined in `tools/hayabusa.rs` and
    // passed as `--output` -- and the transcript is cross-checked against
    // them by
    // `observed_external_output_agrees_with_the_paths_this_crate_hands_the_binaries`.
    // The rest this tree could not reconstruct at all: `logon-summary` is
    // given a filename *prefix* and Takajo's `automagic -o` a *directory*,
    // and each invents what goes below it. Either way the corpus now comes
    // from what a writer was seen doing, so a pattern written from a
    // plausible-sounding external filename has nothing to match.
    corpus.extend(observed_external_paths().into_iter().map(str::to_string));

    corpus
}

/// Every pattern in the bundled config must match at least one filename a
/// real tool run could actually produce. A pattern that matches nothing in
/// this corpus is unreachable in production -- exactly the defect class
/// that left `Persistence` silently unwritten on every real run.
#[test]
fn every_pattern_matches_a_realistically_named_file() {
    let config: SessionConfig = serde_json::from_str(CONFIG).unwrap();
    assert!(!config.sessions.is_empty(), "the bundled config is empty");

    let corpus = realistic_corpus();
    assert!(!corpus.is_empty(), "the realistic-name corpus is empty");

    let mut unreachable: Vec<String> = Vec::new();
    for session in &config.sessions {
        for pattern in &session.file_patterns {
            let set = GlobSetBuilder::new()
                .add(
                    triage_orchestrator::velo::sessions::compile_pattern(pattern).unwrap_or_else(
                        |e| panic!("session {}: invalid pattern {pattern}: {e}", session.name),
                    ),
                )
                .build()
                .unwrap();
            let matched = corpus.iter().any(|path| set.is_match(path));
            if !matched {
                unreachable.push(format!("{}: {pattern}", session.name));
            }
        }
    }

    assert!(
        unreachable.is_empty(),
        "these patterns cannot match any realistically-named file, so their session silently \
         drops the intended data on every real run:\n{}\n\ncorpus checked against:\n{}",
        unreachable.join("\n"),
        corpus.join("\n")
    );
}

/// Every dataset-backed tool this test relies on must actually be reachable
/// through `registry::tool_for_key`/the standalone constructors, so a typo
/// in the key list above fails loudly instead of silently shrinking the
/// corpus.
#[test]
fn every_dataset_backed_tool_is_constructible() {
    let tools = dataset_backed_tools();
    assert_eq!(
        tools.len(),
        12,
        "expected exactly the 12 tools listed above"
    );
    let mut seen_datasets: HashMap<&str, usize> = HashMap::new();
    for (key, tool) in &tools {
        seen_datasets.insert(key, tool.datasets().len());
    }
    for (key, count) in seen_datasets {
        assert!(count > 0, "{key}: no datasets found");
    }
}

/// Ties the observed listing back to this tree, so it cannot quietly become
/// a second hardcoded guess.
///
/// Every path `Hayabusa::plan()` hands the binary as `--output` must appear
/// in the listing verbatim -- that is the half of the naming this tree owns,
/// and a change to the forensic category or the filename breaks here rather
/// than silently leaving the corpus describing a tree that no longer exists.
/// Takajo's `automagic -o` is handed a directory and names its own files, so
/// what is checked there is the directory: every path in the listing must sit
/// *directly* inside it. That is an assertion about the listing's shape, not
/// a claim that Takajo writes nothing deeper -- it does create
/// `scriptblock-logs/`, which the fixture's header describes and deliberately
/// does not transcribe. What the check rules out is the listing growing an
/// intermediate per-run directory, which is precisely the shape the config
/// used to require (`ThreatHunting/*_Takajo_automagic/...`) and no run has
/// ever produced.
#[test]
fn observed_external_output_agrees_with_the_paths_this_crate_hands_the_binaries() {
    let observed = observed_external_paths();
    assert!(!observed.is_empty(), "the observed listing is empty");

    let cfg = ResolvedConfig::default();
    assert!(
        cfg.hayabusa.csv && cfg.hayabusa.json && cfg.hayabusa.logon_summary,
        "this test needs every Hayabusa invocation planned; defaults changed"
    );
    let (host, velo_dir) = velo_host_context();
    let ctx = HostContext {
        host: &host,
        host_dir: PathBuf::from("/out/HOSTX"),
        velo_dir: Some(velo_dir.clone()),
    };

    let plan = Hayabusa.plan(&cfg, &ctx, &Artifacts::default());
    assert_eq!(plan.len(), 3, "expected csv, jsonl and logon-summary");
    for invocation in &plan {
        match &invocation.outputs {
            OutputSpec::Path(path) => {
                let relative = collection_relative(path);
                assert!(
                    observed.contains(&relative.as_str()),
                    "{} writes {relative}, which no real run was ever observed producing; \
                     re-observe tests/data/external_tools_observed_output.txt",
                    invocation.report_name
                );
            }
            OutputSpec::PrefixedIn { dir, prefix } => {
                let dir = collection_relative(dir);
                assert!(
                    observed.iter().any(|path| {
                        path.strip_prefix(&format!("{dir}/"))
                            .is_some_and(|name| !name.contains('/') && name.starts_with(prefix))
                    }),
                    "{} writes {dir}/{prefix}-*, which no real run was ever observed producing",
                    invocation.report_name
                );
            }
        }
    }

    let takajo_dir = collection_relative(&takajo::velo_output_dir(&velo_dir));
    let under_takajo: Vec<&str> = observed
        .iter()
        .copied()
        .filter_map(|path| path.strip_prefix(&format!("{takajo_dir}/")))
        .collect();
    assert!(
        !under_takajo.is_empty(),
        "nothing was observed under {takajo_dir}, the directory takajo automagic -o is given"
    );
    let nested: Vec<&&str> = under_takajo.iter().filter(|n| n.contains('/')).collect();
    assert!(
        nested.is_empty(),
        "the observed Takajo listing is depth-1 report files only (see the fixture header on \
         `scriptblock-logs/`, its one subdirectory, which is excluded on purpose); these sit \
         deeper, so either the binary changed or the listing was re-observed recursively: \
         {nested:?}"
    );
}

/// One dataset's published output, as the collection-relative paths a real
/// run puts on disk: the merged file at the category root, and the per-user
/// slices under it.
struct DatasetPaths {
    /// `<Category>/<stamp>_<Tool>_results[_<Disc>].csv` -- the merged file,
    /// which carries every per-user row plus a `TriageUser` column
    /// (`velo::merge`).
    merged: String,
    /// `<Category>/PerUser[/<Disc>]/<stem>_<label>.csv` -- the slices that
    /// merged file was built from, including the reclaimed system-scope one.
    slices: Vec<String>,
}

/// Every dataset's merged path and its own per-user slice paths, derived
/// through the *routing* code that writes them -- `OutputLayout` under
/// `OutputLayoutMode::Velo`, given the dataset's real Velo discriminator --
/// rather than by writing the `PerUser/` convention out a second time here.
///
/// The labels are the three shapes a slice filename can carry: a profile
/// name (`Identity::User`), the `unknown` bucket, and the merge's reclaimed
/// system-scope slice (`velo::merge::reclaimed_slice_filename`).
fn dataset_paths() -> Vec<DatasetPaths> {
    let collection = Path::new(COLLECTION);
    let mut out = Vec::new();
    for (key, tool) in dataset_backed_tools() {
        let category = triage_orchestrator::velo::category_for_key(key);
        let binary = tool.binary_name();
        let category_root = collection.join(category);
        for spec in tool.datasets() {
            let basename = velo_basename(binary, spec);
            let stem = format!("{STAMP}_{basename}");
            let filename = format!("{stem}.csv");
            let layout = OutputLayout::new(&category_root, binary, false, OutputLayoutMode::Velo)
                .for_velo_dataset(velo_discriminator(binary, spec).as_deref());

            let mut slices = Vec::new();
            for identity in [Identity::User("alice".to_string()), Identity::Unknown] {
                slices.push(collection_relative(&layout.file_path(&identity, &filename)));
            }
            // The reclaimed system-scope slice lands in the same per-user
            // directory, under the internal label.
            let per_user_dir = layout
                .file_path(&Identity::Unknown, &filename)
                .parent()
                .expect("a per-user slice always has a parent directory")
                .to_path_buf();
            slices.push(collection_relative(&per_user_dir.join(
                triage_orchestrator::velo::merge::reclaimed_slice_filename(&stem, "csv"),
            )));

            out.push(DatasetPaths {
                merged: format!("{category}/{filename}"),
                slices,
            });
        }
    }
    out
}

/// No session pattern may match a per-user slice.
///
/// A session is "the files an analyst opens together for this theme". The
/// merged file at the category root is built from exactly these slices and
/// carries every one of their rows plus a `TriageUser` column
/// (`velo::merge`), so loading both puts every row into Timeline Explorer
/// twice -- every sort, filter and count double-counts. The slices are
/// redundant by construction, not merely overlapping.
///
/// This was real: under `globset`'s default semantics `*` crosses `/`, so
/// `Registry/*_RETriage_results*.csv` matched
/// `Registry/PerUser/Batch/<stem>_alice.csv` as well as the merge, and five
/// of the ten session files two real runs wrote contained both.
/// `sessions::compile_pattern` sets `literal_separator(true)`, which is what
/// this asserts the effect of.
///
/// The corpus is the routing code's own output, not a restatement of the
/// `PerUser/` convention, so a change to where slices are written reaches
/// this test.
#[test]
fn no_pattern_matches_a_per_user_slice() {
    let config: SessionConfig = serde_json::from_str(CONFIG).unwrap();
    let datasets = dataset_paths();
    assert!(!datasets.is_empty(), "no dataset paths were derived");

    let mut unwanted: Vec<String> = Vec::new();
    // Non-vacuity: this test is only meaningful for datasets whose *merged*
    // file some session actually names -- otherwise "no pattern matches the
    // slice" would hold trivially for a category no session mentions at all.
    let mut overlapping_datasets = 0usize;

    for dataset in &datasets {
        let mut merged_is_named = false;
        for session in &config.sessions {
            for pattern in &session.file_patterns {
                let set = GlobSetBuilder::new()
                    .add(triage_orchestrator::velo::sessions::compile_pattern(pattern).unwrap())
                    .build()
                    .unwrap();
                if set.is_match(&dataset.merged) {
                    merged_is_named = true;
                }
                for slice in &dataset.slices {
                    if set.is_match(slice) {
                        unwanted.push(format!("{}: {pattern} matches {slice}", session.name));
                    }
                }
            }
        }
        if merged_is_named {
            overlapping_datasets += 1;
        }
    }

    assert!(
        overlapping_datasets > 0,
        "no session names any merged file, so this test asserts nothing"
    );
    assert!(
        unwanted.is_empty(),
        "these patterns also match a per-user slice of a file they already \
         select in merged form, so the session loads every one of those rows \
         twice:\n{}",
        unwanted.join("\n")
    );
}

/// No session pattern may match the reclaimed system-scope slice, under any
/// dataset.
///
/// Stronger than the general slice rule and worth stating on its own: that
/// file is named with `RECLAIM_LABEL`, a 162-character internal bookkeeping
/// string that exists only to be unreachable by any real account name. It is
/// not a filename an analyst should ever be shown, whatever is decided about
/// ordinary per-user slices. Both real runs' `Persistence` session listed it.
#[test]
fn no_pattern_matches_the_reclaimed_system_scope_slice() {
    let config: SessionConfig = serde_json::from_str(CONFIG).unwrap();
    // The label itself, recovered from the merge's own name builder rather
    // than restated here, so renaming it cannot silently empty this corpus.
    let marker = triage_orchestrator::velo::merge::reclaimed_slice_filename("stem", "csv")
        .strip_prefix("stem_")
        .and_then(|rest| rest.strip_suffix(".csv"))
        .expect("reclaimed_slice_filename builds <stem>_<label>.<ext>")
        .to_string();
    let reclaimed: Vec<String> = dataset_paths()
        .into_iter()
        .filter_map(|dataset| {
            dataset
                .slices
                .into_iter()
                .find(|slice| slice.contains(&marker))
        })
        .collect();
    assert!(
        !reclaimed.is_empty(),
        "no reclaimed-slice path was derived; the label changed shape"
    );

    for session in &config.sessions {
        for pattern in &session.file_patterns {
            let set = GlobSetBuilder::new()
                .add(triage_orchestrator::velo::sessions::compile_pattern(pattern).unwrap())
                .build()
                .unwrap();
            for path in &reclaimed {
                assert!(
                    !set.is_match(path),
                    "session {} pattern {pattern} matches the reclaimed \
                     system-scope slice {path}; RECLAIM_LABEL is an internal \
                     name and must never reach an analyst's session list",
                    session.name
                );
            }
        }
    }
}
