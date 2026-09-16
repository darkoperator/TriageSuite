//! Velo basenames are derived from a naming convention rather than declared
//! per dataset. This test is what makes that safe: if a tool's dataset
//! basenames ever stop following the convention, two datasets collide on one
//! Velo filename and this fails, instead of one silently overwriting the other.
//!
//! Coverage is not limited to `registry::all_keys()` (the orchestrator's
//! `--only`/`--skip` set): `LolTriage`, `AnyDeskTriage`, and `SrumNetTriage`
//! are standalone binaries with a `Tool` impl that never appears in
//! `ALL_KEYS`, so they are constructed directly below. `SrumNetTriage` in
//! particular has three datasets and is a real collision surface, not just a
//! completeness formality.

use std::collections::{BTreeSet, HashMap};
use triage_core::output::router::velo_basename;
use triage_core::tool::Tool;

/// Every tool with a `Tool` impl in this tree, registered or not. Kept next
/// to `registry::all_keys()` rather than folded into it: these three are not
/// selectable via `--only`/`--skip` and constructing them needs crate-specific
/// setup the registry's `build()` doesn't (and shouldn't) know about.
fn standalone_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(lol_triage::LolTool {
            refs: lol_triage::refdata::LolRefs::new(Vec::new(), Vec::new()),
        }),
        Box::new(anydesk_triage::AnyDeskTool),
        Box::new(srum_net_triage::SrumNetTool {
            tz: srum_net_triage::aggregate::TzOffset(0),
            business_hours: "08:00-18:00".parse().expect("valid business-hours literal"),
        }),
    ]
}

#[test]
fn velo_basenames_are_unique_per_tool() {
    let keys = triage_orchestrator::registry::all_keys();
    // A future refactor that empties `ALL_KEYS` (or a tool whose `datasets()`
    // regresses to `&[]`) must not make the loop below silently check
    // nothing while still reporting green.
    assert!(!keys.is_empty(), "registry::all_keys() must not be empty");

    let mut checked_datasets = 0usize;

    for key in keys {
        let tool = triage_orchestrator::registry::tool_for_key(key)
            .unwrap_or_else(|| panic!("registry key {key} builds no tool"));
        checked_datasets += check_tool(tool.as_ref());
    }
    for tool in standalone_tools() {
        checked_datasets += check_tool(tool.as_ref());
    }

    assert!(
        checked_datasets > 0,
        "no dataset was checked; every registered tool has an empty datasets() list"
    );
}

/// Checks one tool's datasets for Velo-name collisions and shape, returning
/// how many datasets were checked (so the caller can assert the total isn't
/// zero, catching a tool that vacuously passes by declaring no datasets).
fn check_tool(tool: &dyn Tool) -> usize {
    let binary = tool.binary_name();
    let mut seen: HashMap<String, &str> = HashMap::new();
    let mut checked = 0usize;
    for spec in tool.datasets() {
        let name = velo_basename(binary, spec);
        if let Some(previous) = seen.insert(name.clone(), spec.id) {
            panic!(
                "{binary}: datasets {previous} and {} both map to Velo name {name}",
                spec.id
            );
        }
        assert!(
            name.starts_with(&format!("{binary}_results")),
            "{binary}: dataset {} produced {name}, which is not a Velo name",
            spec.id
        );
        checked += 1;
    }
    checked
}

/// The property the merge post-pass now depends on, which
/// `velo_basenames_are_unique_per_tool` above does **not** cover: uniqueness
/// is about *equality*, and a per-user filename is decoded by stripping a
/// `<stem>_` prefix, so it is the **prefix** relation between stems sharing a
/// directory that decides whether a file can be claimed by the wrong dataset.
///
/// A prefix pair is not forbidden -- forbidding it would ban a tool from
/// having both a bare `_results` dataset and a discriminated one, which four
/// tools in the registry do today. It is made *harmless* instead: a dataset's
/// discriminator is a `PerUser/<Disc>/` directory rather than part of the
/// filename (`OutputLayout::for_velo_dataset`), so a stem and its
/// discriminated sibling never share a directory and no profile name can
/// reach across from one to the other.
///
/// What that construction cannot rule out on its own is two *tools* whose
/// stems land in the same directory and are prefix-related anyway -- a tool
/// named so that its `<Tool>_results` extends another's. This is the guard
/// over that residue, and it asserts the decode itself rather than the naming
/// rule behind it: for every pair of stems that share a `PerUser/` directory,
/// a file of one must decode for that one and for nobody else -- including
/// when the profile name is chosen to look exactly like the ambiguity.
#[test]
fn every_per_user_directory_decodes_to_exactly_one_stem() {
    use triage_orchestrator::velo::merge::user_from_per_user_filename;
    use triage_orchestrator::velo::{category_for_key, category_stems};

    let stamp = "2026-03-13T192553Z";
    let categories: BTreeSet<&str> = triage_orchestrator::registry::all_keys()
        .iter()
        .map(|key| category_for_key(key))
        .collect();

    let mut checked_stems = 0usize;
    let mut shared_directory_pairs = 0usize;
    for category in categories {
        // `SQLiteArtifacts` legitimately yields none: SQLETriage's output is
        // entirely dynamic (per-map, named from the source database), so it
        // declares no `DatasetSpec` and the merge post-pass never runs for
        // it. An empty list is therefore not a defect here -- the total
        // asserted below is what catches a registry that stopped producing
        // stems at all.
        let stems = category_stems(category, stamp);
        checked_stems += stems.len();
        for mine in &stems {
            // Every discriminator in the category, plus two ordinary names:
            // a profile may be called anything, and the interesting "anything"
            // is a name that begins with a sibling dataset's discriminator,
            // which is exactly what a `<stem>_<label>` filename cannot tell
            // apart from that sibling's own output.
            let mut labels: Vec<String> = vec!["alice".into(), "a_b".into()];
            for other in &stems {
                if let Some(dir) = &other.dataset_dir {
                    labels.push(format!("{dir}_alice"));
                }
            }
            for label in &labels {
                let filename = format!("{}_{label}.csv", mine.stem);
                assert_eq!(
                    user_from_per_user_filename(&filename, &mine.stem).as_deref(),
                    Some(label.as_str()),
                    "{category}: {} must claim its own per-user file",
                    mine.stem
                );
                for other in &stems {
                    if other.stem == mine.stem {
                        continue;
                    }
                    if other.dataset_dir != mine.dataset_dir {
                        // Different `PerUser/` directories: `other` never
                        // reads this file, so the decode is not even asked.
                        continue;
                    }
                    assert_eq!(
                        user_from_per_user_filename(&filename, &other.stem),
                        None,
                        "{category}: {} and {} both write into {}, and {} \
                         absorbs {}'s file for profile {label}",
                        mine.stem,
                        other.stem,
                        per_user_dir_display(mine.dataset_dir.as_deref()),
                        other.stem,
                        mine.stem
                    );
                }
            }
            shared_directory_pairs += stems
                .iter()
                .filter(|other| other.stem != mine.stem && other.dataset_dir == mine.dataset_dir)
                .count();
        }
    }

    // The registry has several tools sharing each category's bare `PerUser/`
    // directory (BrowserActivity's and FileSystem's tools, for instance). A
    // registry that stopped producing any shared directory would make the
    // cross-stem half of the loop above vacuous while still reporting green.
    assert!(checked_stems > 0, "no Velo stem was checked");
    assert!(
        shared_directory_pairs > 0,
        "no two stems share a PerUser/ directory: this guard would be vacuous"
    );
}

/// A tripwire, not a proof: today `velo_basename` formats the very
/// `velo_discriminator` result that becomes `dataset_dir`, so
/// `<stem>` ending in `_results_<dataset_dir>` is a tautology over current
/// code and this test cannot fail. It is here for the refactor that derives
/// those two independently -- the directory a dataset writes under and the
/// discriminator its filenames carry must stay the same string -- and it
/// would fail the moment they drifted.
///
/// The property itself, *end to end over the real binary*, is pinned by
/// `a_discriminated_datasets_per_user_slice_lands_under_its_own_directory_and_still_merges`
/// (`tests/velo_layout.rs`): that one fails -- verifiably, it was the RED for
/// this change -- when the router and the merge choose different directories,
/// because the merge then scans an empty directory and silently produces no
/// merged file at all.
#[test]
fn a_stems_directory_is_the_discriminator_its_filenames_carry() {
    use triage_orchestrator::velo::{category_for_key, category_stems};

    let stamp = "2026-03-13T192553Z";
    let categories: BTreeSet<&str> = triage_orchestrator::registry::all_keys()
        .iter()
        .map(|key| category_for_key(key))
        .collect();
    let mut checked = 0usize;
    for category in categories {
        for entry in category_stems(category, stamp) {
            match &entry.dataset_dir {
                Some(dir) => assert!(
                    entry.stem.ends_with(&format!("_results_{dir}")),
                    "{category}: {} is written under PerUser/{dir}/ but its \
                     name does not end in _results_{dir}",
                    entry.stem
                ),
                None => assert!(
                    entry.stem.ends_with("_results"),
                    "{category}: {} writes straight into PerUser/ but is not \
                     a bare _results stem",
                    entry.stem
                ),
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no Velo stem was checked");
}

/// `PerUser/` or `PerUser/<discriminator>/`, for an assertion message that
/// names a real directory rather than an `Option`.
fn per_user_dir_display(dataset_dir: Option<&str>) -> String {
    match dataset_dir {
        Some(dir) => format!("PerUser/{dir}/"),
        None => "PerUser/".to_string(),
    }
}
