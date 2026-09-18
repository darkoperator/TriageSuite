//! Post-pass that merges one dataset's per-user CSVs (`PerUser/`, or
//! `PerUser/<Discriminator>/` -- see `OutputLayout::for_velo_dataset`) into
//! one category-level file carrying a `TriageUser` column.
//!
//! This is a post-pass rather than a router feature because `OutputRouter`
//! serializes a generic `T: Serialize` through the `csv` crate, which cannot
//! append a column to such a record (`#[serde(flatten)]` is unsupported
//! there). Merging afterwards also makes the `PerUser/` files the source of
//! truth: nothing rewrites them, so they stay byte-for-byte Zimmerman-exact.

use std::path::{Path, PathBuf};
use triage_core::attribution::{Identity, MAX_COMPONENT_CHARS};
use triage_core::error::TriageError;
use triage_core::output::layout::{OutputLayout, OutputLayoutMode};

#[derive(Debug)]
pub struct MergeReport {
    pub merged_path: PathBuf,
    pub sources: usize,
    /// The per-user slices this merge actually consumed, in the order it
    /// read them.
    ///
    /// `sources` alone cannot support the exclusion decision a view layer
    /// has to make: knowing that three slices were folded in says nothing
    /// about *which* three, and scanning a merged file alongside a slice it
    /// already contains doubles every one of that slice's rows.
    ///
    /// Note that the reclaimed system-scope slice appears here under its
    /// post-rename `_<RECLAIM_LABEL>` name, not the name the router
    /// published it under.
    pub source_paths: Vec<PathBuf>,
    pub rows: u64,
}

/// On-disk label for the reclaimed system-scope slice's filename
/// (`<per-user dir>/<stem>_<RECLAIM_LABEL>.csv`) -- deliberately not
/// `"system"`.
///
/// `triage_core::attribution::sanitize_component` is the only place a real
/// profile name becomes a filesystem component, and it passes almost every
/// character straight through (only `< > : " / \ | ? *` and controls are
/// mapped to `_`, and a handful of exact reserved DOS device names are
/// prefixed) -- so a label chosen for its *characters* can always be
/// produced by some real account name, on some filesystem, in some case.
/// `Users/System` and `Users/system` are both real, distinct accounts on a
/// case-sensitive filesystem, so case-folding the label doesn't close this
/// either: it just breaks that pair the other way.
///
/// `sanitize_component` is **not** the last shaper of a user-derived label,
/// though: `Attributor::identity_for` (`crates/triage-core/src/attribution.rs`)
/// appends *after* sanitizing, unchecked, on its collision paths --
/// `{base}-{short_suffix}` (an 8-hex-char SHA-256 prefix) and, on a rarer
/// second collision, `{base}-{full_suffix}` (the full 64-hex-char digest) --
/// and `velo_filename` uses that result verbatim. The real ceiling on any
/// label the router can write is therefore `max_identity_label_chars()`
/// below (`MAX_COMPONENT_CHARS` + `-` + a full hex-encoded SHA-256 digest),
/// not `MAX_COMPONENT_CHARS` alone. `RECLAIM_LABEL` is longer than that
/// computed ceiling, which makes it provably outside every label
/// `Attributor` can produce -- collision-proof with any real account, on
/// any filesystem, case-sensitive or not, by construction rather than by
/// comparison. (A label chosen merely to be *longer than 80* would have
/// been unsound: `{base}-{short_suffix}` alone already reaches 89.)
///
/// The `TriageUser` column value written for these rows is still the plain
/// `"system"` (see the merge loops below) -- the on-disk filename and the
/// column analysts read are separate concerns.
///
/// 162 characters: `<stem>_<RECLAIM_LABEL>.csv` stays comfortably under the
/// common 255-byte filename-component limit even for the longest realistic
/// stem (a 32-character `TRIAGE_RUN_STAMP` plus the longest
/// `velo_basename` output in the registry is well under 80 characters),
/// see `reclaim_label_leaves_headroom_under_the_255_byte_filename_limit`.
const RECLAIM_LABEL: &str = "__triagesuite_internal_reclaimed_system_scope_slice_never_a_real_account_name_longer_than_attribution_rs_max_identity_label_incl_full_sha256_suffix_see_merge_rs__";

/// Filename of the reclaimed system-scope slice for `stem`:
/// `<stem>_<RECLAIM_LABEL>.<ext>`.
///
/// Both merge passes build this name, and so does the session-pattern guard
/// (`tests/velo_sessions_patterns.rs`), which asserts no Timeline Explorer
/// session pattern ever matches it -- `RECLAIM_LABEL` is an internal
/// bookkeeping name and an analyst must never be shown a file called that.
/// One spelling, so the guard cannot drift off the name the merge writes.
pub fn reclaimed_slice_filename(stem: &str, ext: &str) -> String {
    format!("{stem}_{RECLAIM_LABEL}.{ext}")
}

/// The true upper bound on any per-user label `Attributor::identity_for` can
/// emit, computed from the real symbols involved rather than hardcoded --
/// see `RECLAIM_LABEL`'s doc comment for the chain this reflects
/// (`sanitize_component` truncates to `MAX_COMPONENT_CHARS`, then
/// `identity_for` may append `-` plus a full hex-encoded digest,
/// unchecked). Raising `MAX_COMPONENT_CHARS`, or widening the hash
/// `Attributor` uses, changes this bound automatically, so the `debug_assert!`
/// and test below that compare `RECLAIM_LABEL` against it stay meaningful
/// rather than silently inverting the guarantee.
fn max_identity_label_chars() -> usize {
    use sha2::Digest;
    // "-" plus two hex characters per digest byte, matching
    // `Attributor::identity_for`'s `format!("{base}-{full_suffix}")`.
    MAX_COMPONENT_CHARS + 1 + sha2::Sha256::output_size() * 2
}

/// The user encoded in a per-user filename, or `None` when `filename` is not
/// per-user output for `stem`.
///
/// `filename` must come from the directory `stem`'s per-user slices are
/// written into -- `PerUser/`, plus the dataset's discriminator level when it
/// has one (`OutputLayout::for_velo_dataset`, which explains why). That is
/// what makes this decode unambiguous rather than merely plausible. A Velo
/// stem is `<stamp>_<Tool>_results[_<Discriminator>]`, so a tool with both a
/// bare dataset and a discriminated one produces stems where one is a literal
/// `<other>_` prefix of the other, and a profile named after the
/// discriminator (`PackageId_alice`, `Downloads_alice`) then produces exactly
/// the filename the discriminated dataset produces for the profile the rest
/// of the label names. Reproduced on a real capture: the bare dataset's slice
/// for `Downloads_alice` and the `Downloads` dataset's slice for `alice` were
/// one path, so the run either failed on the collision or clobbered one of
/// them, and the survivor was published under the other dataset's schema and
/// the other dataset's `TriageUser`. Hoisting the discriminator into a
/// directory is what separates them; no rule applied to the *name* can,
/// longest-match included, because the two names are one string.
///
/// Total by construction: every input returns a value or an explicit absence.
/// A user label may itself contain `_` (profile names are sanitized, not
/// stripped), so the label is everything between `<stem>_` and the extension.
pub fn user_from_per_user_filename(filename: &str, stem: &str) -> Option<String> {
    Some(label_after_stem(filename, stem)?.to_string())
}

/// The label between `<stem>_` and the extension, or `None` when `filename`
/// is not `<stem>_<label>[.<ext>]`.
fn label_after_stem<'a>(filename: &'a str, stem: &str) -> Option<&'a str> {
    let rest = filename.strip_prefix(stem)?.strip_prefix('_')?;
    let label = match rest.rsplit_once('.') {
        Some((label, _ext)) => label,
        None => rest,
    };
    if label.is_empty() {
        return None;
    }
    Some(label)
}

/// The directory one dataset's per-user slices live in:
/// `<category>/PerUser[/<discriminator>]`. The mirror of
/// `OutputLayout::for_velo_dataset` on the reading side -- the merge must
/// look exactly where the router wrote, and both halves of that are one rule
/// stated in two crates, so each names the other.
fn per_user_dir(category_dir: &Path, dataset_dir: Option<&str>) -> PathBuf {
    let shared = category_dir.join("PerUser");
    match dataset_dir {
        Some(dir) => shared.join(dir),
        None => shared,
    }
}

/// Whether `path` is one of the destinations this run's `OutputRouter`
/// published (`router_wrote`, see [`merge_per_user`]).
///
/// Both merge functions ask this of two different paths -- the reclaim's
/// source at the category root, and the reclaim's destination under
/// the dataset's per-user directory -- so the comparison lives in one place
/// rather than in four hand-written copies.
///
/// The comparison is exact, no normalization, because both sides are built
/// from the same value: `execute.rs` clones one `csv_root`/`json_root` into
/// `RouterOptions` and passes the other copy here as `category_dir`, and
/// `router_wrote` holds the `StagedFile::destination` paths `OutputLayout`
/// derived from that root. A relative or unnormalized root reaches both
/// sides identically, so it cannot desynchronize them --
/// `a_relative_out_path_still_reclaims_the_system_scope_slice`
/// (`tests/velo_layout.rs`) pins that end to end, because a false negative
/// here would let `--overwrite` replace a genuine system-scope slice with a
/// merged file built from the per-user slices alone.
fn router_published(router_wrote: &[PathBuf], path: &Path) -> bool {
    router_wrote.iter().any(|written| written == path)
}

/// This run's published per-user slices for `stem` in `per_user`, as
/// `(user label, path)` pairs -- the merge's sources.
///
/// Derived from `router_wrote` rather than from a listing of `per_user`,
/// because that directory accumulates across runs. `--overwrite` replaces
/// the slices a rerun writes *again*, but a slice for a profile that is no
/// longer on the host, or for an artifact class that stopped being
/// collected, is simply left behind: nothing this run does touches it. A
/// directory scan cannot tell such a leftover from this run's own output --
/// they are the same filename, because the same `TRIAGE_RUN_STAMP` produces
/// the same stem -- so it merged the leftover in under its old identity, and
/// `write_output_hashes` then attested to a merged file carrying evidence
/// this capture does not contain. Publication is the only signal that
/// distinguishes them, and `OutputRouter::finish` now reports it (see
/// [`merge_per_user`]'s `router_wrote`).
///
/// The reclaimed system-scope slice is deliberately *not* found here: it is
/// renamed into `per_user` during the merge below, long after the router
/// finished, so it is by construction absent from `router_wrote`. The
/// reclaim appends it to the sources itself, which is what admits that one
/// file -- and only that one -- on top of this run's publications.
///
/// `extension` includes the dot (`".csv"`, `".json"`): one per-user
/// directory holds both formats for the same stem, and each merge must take
/// only its own. Both merges call this, so their source selection cannot
/// drift apart the way two hand-written scans could.
///
/// `path.parent()` is compared to `per_user` for the same reason
/// [`router_published`] compares paths exactly: both sides are built from
/// one `category_dir` by the same rule (`per_user_dir` here,
/// `OutputLayout::velo_per_user_dir` on the write side), so no
/// normalization is needed and none is done. It also keeps a discriminated
/// dataset's slices out of the bare dataset's merge, exactly as a
/// non-recursive `read_dir` of `PerUser/` did.
fn published_per_user_sources(
    per_user: &Path,
    stem: &str,
    extension: &str,
    router_wrote: &[PathBuf],
) -> Vec<(String, PathBuf)> {
    router_wrote
        .iter()
        .filter(|path| path.parent() == Some(per_user))
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
            if !name.ends_with(extension) {
                return None;
            }
            Some((user_from_per_user_filename(name, stem)?, path.clone()))
        })
        .collect()
}

/// Merge this dataset's `<per-user dir>/<stem>_<user>.csv` slices into
/// `<category_dir>/<stem>.csv`, appending a `TriageUser` column.
///
/// Returns `Ok(None)` when the dataset's per-user directory does not exist or
/// this run published no per-user slices for `stem` — a system-scope tool, or
/// a tool that produced nothing.
///
/// `router_wrote` is the set of destinations `OutputRouter` actually
/// **published** this run: `OutputRouter::finish`'s `FinishReport::published`,
/// one entry per staged file whose rename onto its destination succeeded.
///
/// It is the merge's source of truth twice over. It *selects the sources*:
/// the per-user slices merged are this run's publications, never whatever
/// happens to be sitting in the per-user directory
/// ([`published_per_user_sources`], which explains what a directory listing
/// let through). And it answers the reclaim's one real question: was the
/// category-root file this run's own system-scope output, or a leftover from
/// a previous run? Only the former may be reclaimed -- see the reclaim
/// below, and note that the reclaimed slice itself is the one source that
/// does not come from `router_wrote`, because it is created after the router
/// finished.
///
/// Published, not merely opened, and not merely present: `finish()` returns
/// the first error it hits but keeps going, so a failed run can still have
/// published some destinations and not others, and a run that aborted after
/// an earlier write failure deletes every staged file and publishes none.
/// In both cases a destination this run opened can be sitting there holding
/// a *previous* run's file -- with `--overwrite`, a previous run's own
/// merged file, at exactly the path a `Scope::UserElseSystem` tool's
/// system-scope slice is written to. Reclaiming that carried the only copy
/// of those rows off the category root; the caller therefore reports
/// publication and nothing weaker
/// (`a_failed_finish_publishes_nothing_so_the_previous_merged_csv_survives`
/// and its NDJSON twin, `tests/velo_layout.rs`).
///
/// It also feeds a second, defensive check. Because the reclaim writes its
/// slice under `RECLAIM_LABEL` rather than `"system"`, no real account can
/// ever produce a path that collides with it, so `router_wrote` should never
/// actually contain the reclaim's *destination* -- that check is a backstop
/// against the invariant ever being violated (e.g. a future change to
/// `RECLAIM_LABEL` or to the sanitizer), not the primary mechanism, and its
/// message says so if it ever fires.
///
/// `dataset_dir` is the dataset's Velo discriminator
/// (`triage_core::output::router::velo_discriminator`), or `None` for a
/// dataset that has none -- the directory level its per-user slices were
/// written into, and the reason a filename there decodes to exactly one
/// dataset (`OutputLayout::for_velo_dataset`,
/// `user_from_per_user_filename`).
pub fn merge_per_user(
    category_dir: &Path,
    stem: &str,
    overwrite: bool,
    router_wrote: &[PathBuf],
    dataset_dir: Option<&str>,
) -> Result<Option<MergeReport>, TriageError> {
    // Debug-only (compiled out in release, like every `debug_assert!`): a
    // cheap early warning in dev/test builds, not a guarantee this binary
    // enforces at runtime. `reclaim_label_exceeds_max_identity_label_chars`
    // below is what actually proves the invariant, and it runs regardless
    // of build profile.
    debug_assert!(
        RECLAIM_LABEL.chars().count() > max_identity_label_chars(),
        "RECLAIM_LABEL must stay outside the real ceiling on an \
         Attributor-derived label (max_identity_label_chars()), not just \
         MAX_COMPONENT_CHARS, to remain collision-proof with any real \
         account name"
    );
    let per_user = per_user_dir(category_dir, dataset_dir);
    if !per_user.is_dir() {
        return Ok(None);
    }

    let mut sources = published_per_user_sources(&per_user, stem, ".csv", router_wrote);
    // A per-user directory is shared between tools (e.g. `FileSystem/PerUser`
    // holds both `le`'s and `rb`'s slices -- both bare-dataset tools in that
    // category), so its existence alone says nothing about *this* stem -- a
    // `SystemWide` sibling's own category-root file must never be swept into a
    // reclaim just because some other tool created the directory. Gating on
    // "at least one per-user slice this run published for this stem" rather
    // than on the tool's `Scope` also gets the opposite edge case right: a
    // `UserElseSystem` tool that produced system-only output this run has no
    // per-user slices either, and must not gain a pointless single-row
    // `TriageUser` column.
    if sources.is_empty() {
        return Ok(None);
    }

    // Only now -- knowing a genuine per-user slice exists for this stem, and
    // that the router published the category-root path itself this run -- is
    // the file sitting there safe to treat as this run's system-scope slice
    // (`Identity::System` under `OutputLayoutMode::Velo` routes there, the
    // same path the merged file must end up at). Reclaim it before the merge
    // below, so it merges in with `TriageUser = "system"` instead of
    // permanently blocking the merged file from ever existing.
    let merged_path = category_dir.join(format!("{stem}.csv"));
    if router_published(router_wrote, &merged_path) && merged_path.is_file() {
        let system_slice = per_user.join(reclaimed_slice_filename(stem, "csv"));
        // Asked unconditionally, *not* only when the file is on disk. See
        // `RECLAIM_LABEL`'s doc comment: no real account can ever produce
        // this exact path, so `router_wrote` naming it means the invariant
        // was somehow violated and something is wrong enough not to guess
        // about. It also has to be asked before `exists()` can be consulted,
        // because `published_per_user_sources` does no I/O: a destination
        // named as published but absent from disk still yields a
        // `RECLAIM_LABEL` source, and the rename below would then make the
        // push a *second* entry for that one path -- the duplicated
        // system-scope row the case-folding round fixed
        // (`the_invariant_backstop_refuses_a_claimed_reclaim_path_that_is_not_on_disk`).
        if router_published(router_wrote, &system_slice) {
            return Err(TriageError::Output {
                path: system_slice.clone(),
                message: format!(
                    "internal invariant violated: {} was written by the \
                     router this run, but RECLAIM_LABEL is supposed to be \
                     unreachable by any real account name -- refusing to \
                     fold {} into the merge rather than guessing",
                    system_slice.display(),
                    merged_path.display()
                ),
            });
        }
        if system_slice.exists() && !overwrite {
            // A stale leftover from a previous run's own reclaim -- derived
            // output of this pipeline, which `--overwrite` legitimately
            // replaces via the rename below, same as any other output
            // collision. Without it, refused like any other output
            // collision.
            return Err(TriageError::Output {
                path: system_slice.clone(),
                message: format!(
                    "cannot fold the system-scope output at {} into the \
                     merge: {} already exists; pass --overwrite to \
                     replace it (a leftover reclaim from a previous run, \
                     not this run's output)",
                    merged_path.display(),
                    system_slice.display()
                ),
            });
        }
        std::fs::rename(&merged_path, &system_slice).map_err(|e| TriageError::Output {
            path: system_slice.clone(),
            message: e.to_string(),
        })?;
        // No duplicate to guard against: `sources` holds only destinations
        // the router published, and the backstop above has -- now
        // unconditionally -- refused the one case where `router_wrote` could
        // name this path. A stale leftover the rename just replaced was
        // therefore never a source.
        sources.push((RECLAIM_LABEL.to_string(), system_slice));
    }
    // Deterministic output ordering: the filesystem's is not.
    sources.sort_by(|a, b| a.0.cmp(&b.0));

    // A previous run's own merged file can still be sitting at `merged_path`
    // here -- the reclaim above declines it, because it is not this run's
    // system-scope output -- and so can a directory occupying the path. Both
    // are ordinary output collisions from this point on: refused here without
    // `--overwrite`; with it, `publish`'s rename replaces a stale file and
    // still fails on a directory, like every other output in this tool.
    if !overwrite && merged_path.exists() {
        return Err(TriageError::Output {
            path: merged_path,
            message: "output file exists; pass --overwrite to replace it".into(),
        });
    }

    // Staged, then renamed into place -- never written in situ. A merge can
    // fail *after* the destination is open (a per-user header that differs
    // from the first source's is a hard error below), and truncating the
    // destination up front left a partial file carrying the wrong dataset's
    // schema at the category root. That file is absent from the manifest but
    // `write_output_hashes` walks the directory, so the chain-of-custody
    // record attested to a file the pipeline already knew was wrong. Nothing
    // may appear at `merged_path` unless the whole merge succeeded.
    let layout = staging_layout(category_dir, overwrite);
    let staged = layout.create_staged(&Identity::System, &format!("{stem}.csv"))?;
    let temporary = staged.temporary.clone();
    let rows = match write_merged_csv(staged.file, &sources, &merged_path) {
        Ok(rows) => rows,
        Err(e) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(e);
        }
    };
    if let Err(e) = layout.publish(&temporary, &merged_path) {
        // `publish` removes the temporary itself only on its
        // already-exists arm; a failed rename leaves it behind, and a
        // stray `.tmp-` file at the category root would be walked into
        // `CaseInfo/<stamp>_OutputHashes.txt` like any other file.
        let _ = std::fs::remove_file(&temporary);
        return Err(e);
    }

    Ok(Some(MergeReport {
        merged_path,
        sources: sources.len(),
        source_paths: sources.iter().map(|(_, path)| path.clone()).collect(),
        rows,
    }))
}

/// The `OutputLayout` used to stage and publish a merged file.
///
/// `binary_name` is `""` on purpose: it is read only by
/// `OutputLayout::base()`, which serves `OutputLayoutMode::Nested`. Under
/// `Velo` an `Identity::System` path is `<root>/<filename>`, which is exactly
/// where the merged file belongs, so there is no tool name for this call to
/// supply and none is consulted.
fn staging_layout(category_dir: &Path, overwrite: bool) -> OutputLayout {
    OutputLayout::new(category_dir, "", overwrite, OutputLayoutMode::Velo)
}

/// Write every source's rows, plus the `TriageUser` column, into the staged
/// file. `destination` is used only to name the eventual output in error
/// messages: nothing here touches it on disk.
fn write_merged_csv(
    file: std::fs::File,
    sources: &[(String, PathBuf)],
    destination: &Path,
) -> Result<u64, TriageError> {
    let merged_path = destination;
    let mut writer = csv::Writer::from_writer(file);
    let mut header: Option<Vec<String>> = None;
    let mut rows = 0u64;

    for (user, path) in sources {
        let mut reader = csv::Reader::from_path(path).map_err(|e| TriageError::Output {
            path: path.clone(),
            message: e.to_string(),
        })?;
        let this: Vec<String> = reader
            .headers()
            .map_err(|e| TriageError::Output {
                path: path.clone(),
                message: e.to_string(),
            })?
            .iter()
            .map(str::to_string)
            .collect();

        match &header {
            None => {
                let mut out = this.clone();
                out.push("TriageUser".to_string());
                writer.write_record(&out).map_err(|e| TriageError::Output {
                    path: merged_path.to_path_buf(),
                    message: e.to_string(),
                })?;
                header = Some(this);
            }
            Some(first) if *first != this => {
                // Interleaved columns would produce a file that looks correct
                // and is not. Fail instead.
                return Err(TriageError::Output {
                    path: path.clone(),
                    message: format!(
                        "per-user header differs from {}: cannot merge",
                        sources[0].1.display()
                    ),
                });
            }
            Some(_) => {}
        }

        // The on-disk reclaim label and the column value are separate
        // concerns (`RECLAIM_LABEL`'s doc comment): analysts read "system"
        // in `TriageUser`, never the collision-proof internal label.
        let triage_user = if user == RECLAIM_LABEL {
            "system"
        } else {
            user
        };
        for record in reader.records() {
            let record = record.map_err(|e| TriageError::Output {
                path: path.clone(),
                message: e.to_string(),
            })?;
            let mut out: Vec<String> = record.iter().map(str::to_string).collect();
            out.push(triage_user.to_string());
            writer.write_record(&out).map_err(|e| TriageError::Output {
                path: merged_path.to_path_buf(),
                message: e.to_string(),
            })?;
            rows += 1;
        }
    }

    writer.flush().map_err(|e| TriageError::Output {
        path: merged_path.to_path_buf(),
        message: e.to_string(),
    })?;

    Ok(rows)
}

/// NDJSON counterpart of [`merge_per_user`]. Each line is parsed as an object
/// and gains a `TriageUser` field; a line that is not a JSON object fails the
/// merge rather than being skipped, because a dropped record is evidence not
/// shown. See `merge_per_user`'s doc comment for what `router_wrote` is and
/// why it is the exact signal, not a heuristic, and what `dataset_dir` is.
pub fn merge_per_user_ndjson(
    category_dir: &Path,
    stem: &str,
    overwrite: bool,
    router_wrote: &[PathBuf],
    dataset_dir: Option<&str>,
) -> Result<Option<MergeReport>, TriageError> {
    // Debug-only (compiled out in release, like every `debug_assert!`): a
    // cheap early warning in dev/test builds, not a guarantee this binary
    // enforces at runtime. `reclaim_label_exceeds_max_identity_label_chars`
    // below is what actually proves the invariant, and it runs regardless
    // of build profile.
    debug_assert!(
        RECLAIM_LABEL.chars().count() > max_identity_label_chars(),
        "RECLAIM_LABEL must stay outside the real ceiling on an \
         Attributor-derived label (max_identity_label_chars()), not just \
         MAX_COMPONENT_CHARS, to remain collision-proof with any real \
         account name"
    );
    let per_user = per_user_dir(category_dir, dataset_dir);
    if !per_user.is_dir() {
        return Ok(None);
    }

    let mut sources = published_per_user_sources(&per_user, stem, ".json", router_wrote);
    // See the identical gate in `merge_per_user`: a per-user directory is
    // shared between tools, so its existence alone says nothing about this
    // stem -- only a per-user slice this run published for this stem makes a
    // pre-existing category-root file safe to treat as a system-scope slice
    // below.
    if sources.is_empty() {
        return Ok(None);
    }

    // See the identical reclaim in `merge_per_user`: the router writes a
    // `UserElseSystem` tool's system-scope slice straight to the category
    // root, the same path the merged NDJSON file will occupy -- and only a
    // file the router published there *this run* may be reclaimed.
    let merged_path = category_dir.join(format!("{stem}.json"));
    if router_published(router_wrote, &merged_path) && merged_path.is_file() {
        let system_slice = per_user.join(reclaimed_slice_filename(stem, "json"));
        // See the identical backstop and reasoning in `merge_per_user`,
        // including why it is asked before -- and independently of --
        // `exists()`.
        if router_published(router_wrote, &system_slice) {
            return Err(TriageError::Output {
                path: system_slice.clone(),
                message: format!(
                    "internal invariant violated: {} was written by the \
                     router this run, but RECLAIM_LABEL is supposed to be \
                     unreachable by any real account name -- refusing to \
                     fold {} into the merge rather than guessing",
                    system_slice.display(),
                    merged_path.display()
                ),
            });
        }
        if system_slice.exists() && !overwrite {
            // See the identical collision arm in `merge_per_user`.
            return Err(TriageError::Output {
                path: system_slice.clone(),
                message: format!(
                    "cannot fold the system-scope output at {} into the \
                     merge: {} already exists; pass --overwrite to \
                     replace it (a leftover reclaim from a previous run, \
                     not this run's output)",
                    merged_path.display(),
                    system_slice.display()
                ),
            });
        }
        std::fs::rename(&merged_path, &system_slice).map_err(|e| TriageError::Output {
            path: system_slice.clone(),
            message: e.to_string(),
        })?;
        // See the identical note in `merge_per_user`: a source set built from
        // this run's publications cannot already hold the reclaim's label,
        // because the backstop above refuses that case unconditionally.
        sources.push((RECLAIM_LABEL.to_string(), system_slice));
    }
    sources.sort_by(|a, b| a.0.cmp(&b.0));

    // See `merge_per_user`'s identical guard: a previous run's own merged
    // file, or a directory, can still occupy this path, and both are
    // ordinary output collisions from here on.
    if !overwrite && merged_path.exists() {
        return Err(TriageError::Output {
            path: merged_path,
            message: "output file exists; pass --overwrite to replace it".into(),
        });
    }
    // Staged, then renamed into place: see the identical reasoning in
    // `merge_per_user`. This path can fail mid-loop too -- a source line that
    // is not a JSON object is a hard error below -- and a partial NDJSON file
    // left at the category root would be hashed into
    // `CaseInfo/<stamp>_OutputHashes.txt` exactly like the CSV case.
    let layout = staging_layout(category_dir, overwrite);
    let staged = layout.create_staged(&Identity::System, &format!("{stem}.json"))?;
    let temporary = staged.temporary.clone();
    let rows = match write_merged_ndjson(staged.file, &sources, &merged_path) {
        Ok(rows) => rows,
        Err(e) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(e);
        }
    };
    if let Err(e) = layout.publish(&temporary, &merged_path) {
        // See the identical cleanup in `merge_per_user`.
        let _ = std::fs::remove_file(&temporary);
        return Err(e);
    }

    Ok(Some(MergeReport {
        merged_path,
        sources: sources.len(),
        source_paths: sources.iter().map(|(_, path)| path.clone()).collect(),
        rows,
    }))
}

/// NDJSON counterpart of [`write_merged_csv`], with the same contract:
/// `destination` names the eventual output in error messages only, and
/// nothing here touches it on disk.
fn write_merged_ndjson(
    file: std::fs::File,
    sources: &[(String, PathBuf)],
    destination: &Path,
) -> Result<u64, TriageError> {
    use std::io::{BufRead, BufReader, Write};

    let merged_path = destination;
    let mut out = file;
    let mut rows = 0u64;

    for (user, path) in sources {
        // See the identical translation in `merge_per_user`: analysts read
        // "system" in `TriageUser`, never the collision-proof internal label.
        let triage_user = if user == RECLAIM_LABEL {
            "system"
        } else {
            user
        };
        let file = std::fs::File::open(path).map_err(|e| TriageError::Output {
            path: path.clone(),
            message: e.to_string(),
        })?;
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|e| TriageError::Output {
                path: path.clone(),
                message: e.to_string(),
            })?;
            if line.trim().is_empty() {
                continue;
            }
            // `preserve_order` (Cargo.toml) is enabled workspace-wide so that
            // JSON property order matches struct declaration order matches
            // CSV column order; inserting `TriageUser` last here keeps NDJSON
            // consistent with the merged CSV's trailing column.
            let mut value: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&line)
                .map_err(|e| TriageError::Output {
                    path: path.clone(),
                    message: format!("line is not a JSON object: {e}"),
                })?;
            value.insert(
                "TriageUser".to_string(),
                serde_json::Value::String(triage_user.to_string()),
            );
            let encoded = serde_json::to_string(&value).map_err(|e| TriageError::Output {
                path: merged_path.to_path_buf(),
                message: e.to_string(),
            })?;
            writeln!(out, "{encoded}").map_err(|e| TriageError::Output {
                path: merged_path.to_path_buf(),
                message: e.to_string(),
            })?;
            rows += 1;
        }
    }
    out.flush().map_err(|e| TriageError::Output {
        path: merged_path.to_path_buf(),
        message: e.to_string(),
    })?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The router's system-scope write at the category root, as the reclaim
    /// sees it: the file itself *and* the `router_wrote` entry saying this
    /// run published it. Both halves are the precondition -- a category-root
    /// file that `router_wrote` does not name is a previous run's leftover
    /// (its merged output, most often), which the reclaim must never move.
    fn write_router_system_slice(
        category: &std::path::Path,
        stem: &str,
        ext: &str,
        body: &str,
    ) -> Vec<PathBuf> {
        vec![write(category, &format!("{stem}.{ext}"), body)]
    }

    /// Writes a file and returns its path, so a test can hand that path
    /// straight to `router_wrote`. A per-user slice the test does *not* put
    /// in `router_wrote` is a leftover from a previous run, which is exactly
    /// what `published_per_user_sources` refuses to merge -- so "write it and
    /// publish it" and "write it only" are the two things these tests need to
    /// be able to say, and the return value is how they say the first.
    fn write(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    /// The proof `RECLAIM_LABEL`'s doc comment refers to:
    /// `sanitize_component` never emits a string longer than
    /// `MAX_COMPONENT_CHARS`, no matter the input, so a label longer than
    /// that is provably outside its output image -- not just for the inputs
    /// this suite happens to try, but for every input, because the function
    /// truncates unconditionally.
    #[test]
    fn reclaim_label_exceeds_max_identity_label_chars() {
        let ceiling = max_identity_label_chars();
        assert!(
            RECLAIM_LABEL.chars().count() > ceiling,
            "RECLAIM_LABEL ({} chars) must exceed the computed ceiling ({} \
             chars = MAX_COMPONENT_CHARS + '-' + a full hex-encoded SHA-256 \
             digest) to stay outside every label Attributor::identity_for \
             can produce -- not just MAX_COMPONENT_CHARS ({}) alone",
            RECLAIM_LABEL.chars().count(),
            ceiling,
            MAX_COMPONENT_CHARS
        );
        // A handful of adversarial inputs to `sanitize_component` alone,
        // including one already longer than RECLAIM_LABEL itself, to make
        // the "sanitize_component never exceeds MAX_COMPONENT_CHARS" half of
        // the claim concrete rather than merely asserted.
        for input in [
            "System",
            "system",
            &"a".repeat(500),
            RECLAIM_LABEL,
            &format!("{RECLAIM_LABEL}{RECLAIM_LABEL}"),
        ] {
            let sanitized = triage_core::attribution::sanitize_component(input);
            assert_ne!(
                sanitized, RECLAIM_LABEL,
                "sanitize_component({input:?}) must never equal RECLAIM_LABEL"
            );
            assert!(sanitized.chars().count() <= MAX_COMPONENT_CHARS);
        }
    }

    /// Covers the other half of the claim -- `Attributor::identity_for`'s
    /// append-after-sanitizing step -- with a genuine collision rather than
    /// arithmetic alone: two profile names that sanitize to the identical
    /// base (`sanitize_component` maps both `<` and `>` to `_`) but carry
    /// different lowercase keys, so `Attributor` treats them as distinct
    /// accounts and must suffix the second.
    #[test]
    fn a_real_attributor_collision_stays_within_the_computed_ceiling() {
        use std::path::Path;
        use triage_core::attribution::{Attributor, Identity};

        let mut attributor = Attributor::new();
        let first = attributor.identity_for(Path::new(r"C:\Users\alice<1\NTUSER.DAT"));
        let second = attributor.identity_for(Path::new(r"C:\Users\alice>1\NTUSER.DAT"));

        let Identity::User(base) = first else {
            panic!("expected Identity::User, got {first:?}")
        };
        let Identity::User(suffixed) = second else {
            panic!("expected Identity::User, got {second:?}")
        };
        assert_eq!(base, "alice_1");
        assert!(
            suffixed.starts_with("alice_1-"),
            "expected a suffixed collision name: got {suffixed}"
        );
        assert!(
            suffixed.chars().count() <= max_identity_label_chars(),
            "a real Attributor-derived label must never exceed the computed \
             ceiling: got {suffixed} ({} chars) > {}",
            suffixed.chars().count(),
            max_identity_label_chars()
        );
        assert_ne!(suffixed, RECLAIM_LABEL);
        assert!(RECLAIM_LABEL.chars().count() > suffixed.chars().count());
    }

    /// `PerUser/<stem>_<RECLAIM_LABEL>.csv` must stay under the common
    /// 255-byte filename-component limit even for the longest realistic
    /// stem, computed from the real registry rather than guessed.
    #[test]
    fn reclaim_label_leaves_headroom_under_the_255_byte_filename_limit() {
        use triage_core::output::router::velo_basename;
        use triage_core::tool::Tool;

        let mut longest_basename_chars = 0usize;
        let mut note_basename = |tool: &dyn Tool| {
            for spec in tool.datasets() {
                let chars = velo_basename(tool.binary_name(), spec).chars().count();
                longest_basename_chars = longest_basename_chars.max(chars);
            }
        };
        for key in crate::registry::all_keys() {
            let tool = crate::registry::tool_for_key(key)
                .unwrap_or_else(|| panic!("registry key {key} builds no tool"));
            note_basename(tool.as_ref());
        }
        // Standalone binaries with a `Tool` impl that never appear in
        // `registry::all_keys()` (mirrors `tests/velo_names.rs`'s
        // `standalone_tools()`).
        note_basename(&lol_triage::LolTool {
            refs: lol_triage::refdata::LolRefs::new(Vec::new(), Vec::new()),
        });
        note_basename(&anydesk_triage::AnyDeskTool);
        note_basename(&srum_net_triage::SrumNetTool {
            tz: srum_net_triage::aggregate::TzOffset(0),
            business_hours: "08:00-18:00".parse().expect("valid business-hours literal"),
        });
        assert!(longest_basename_chars > 0, "no dataset was checked");

        // 32 is `valid_stamp`'s own cap on `TRIAGE_RUN_STAMP`
        // (`crates/triage-core/src/output/router.rs`), the widest a run
        // stamp can legitimately be; execute.rs's stem is
        // `format!("{stamp}_{basename}")`.
        const MAX_STAMP_CHARS: usize = 32;
        let worst_case_stem_chars = MAX_STAMP_CHARS + 1 + longest_basename_chars;
        let filename_chars =
            worst_case_stem_chars + 1 + RECLAIM_LABEL.chars().count() + ".csv".len();

        assert!(
            filename_chars < 255,
            "PerUser/<stem>_<RECLAIM_LABEL>.csv would be {filename_chars} \
             characters (worst-case stem {worst_case_stem_chars} + '_' + \
             RECLAIM_LABEL {} + \".csv\"), at or over the common 255-byte \
             filename-component limit",
            RECLAIM_LABEL.chars().count()
        );
    }

    #[test]
    fn merges_per_user_files_and_appends_the_user_column() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_LETriage_results";
        let router_wrote = vec![
            write(
                &category.join("PerUser"),
                &format!("{stem}_jdoe.csv"),
                "Path,Name\nC:\\a,a\nC:\\b,b\n",
            ),
            write(
                &category.join("PerUser"),
                &format!("{stem}_asmith.csv"),
                "Path,Name\nC:\\c,c\n",
            ),
        ];

        let report = merge_per_user(category, stem, false, &router_wrote, None)
            .unwrap()
            .unwrap();
        assert_eq!(report.sources, 2);
        assert_eq!(report.rows, 3);

        let merged = std::fs::read_to_string(category.join(format!("{stem}.csv"))).unwrap();
        assert!(merged.starts_with("Path,Name,TriageUser\n"), "got {merged}");
        assert!(merged.contains("C:\\a,a,jdoe\n"), "got {merged}");
        assert!(merged.contains("C:\\c,c,asmith\n"), "got {merged}");
        assert_eq!(merged.lines().count(), 4, "header plus three rows");
    }

    /// The merged file holds exactly its sources' rows, so anything building
    /// a view over both would double-count every one of them. Naming the
    /// sources is what lets a caller exclude them; a bare count cannot say
    /// *which* slices were folded in.
    #[test]
    fn a_merge_report_names_every_source_it_consumed() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_LETriage_results";
        let router_wrote = vec![
            write(
                &category.join("PerUser"),
                &format!("{stem}_jdoe.csv"),
                "Path,Name\nC:\\a,a\nC:\\b,b\n",
            ),
            write(
                &category.join("PerUser"),
                &format!("{stem}_asmith.csv"),
                "Path,Name\nC:\\c,c\n",
            ),
        ];

        let report = merge_per_user(category, stem, false, &router_wrote, None)
            .unwrap()
            .unwrap();

        assert_eq!(report.sources, report.source_paths.len());
        assert_eq!(report.source_paths.len(), 2);
        let mut named: Vec<String> = report
            .source_paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        named.sort();
        assert_eq!(
            named,
            vec![format!("{stem}_asmith.csv"), format!("{stem}_jdoe.csv"),]
        );
        // Every named source is one of the slices the router published, not
        // something the merge discovered by listing the directory.
        for path in &report.source_paths {
            assert!(router_wrote.contains(path), "unpublished source: {path:?}");
        }
    }

    /// Codex P1, the layer under C1: a profile whose sanitized name begins
    /// with a sibling dataset's discriminator produces the *same filename*
    /// that sibling produces for another profile. `WxTTriage` ships the pair
    /// (`..._Activity` and `..._Activity_PackageId`), so the Activity slice
    /// for a profile named `PackageId_localadmin` and the PackageId slice for
    /// `localadmin` are one name -- the first physically collided with the
    /// second, and whichever survived was merged under the other dataset's
    /// schema and the other dataset's `TriageUser`. No rule over that name
    /// can separate them, longest match included: the two names are equal.
    ///
    /// The discriminator is a directory instead, so the two identical names
    /// are two files, and each merge reads only its own dataset's directory.
    /// This fixture writes exactly that pair, byte-identical basenames and
    /// all.
    #[test]
    fn a_profile_named_after_a_sibling_discriminator_stays_with_its_own_dataset() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let short = "2026-03-13T192553Z_WxTTriage_results_Activity";
        let long = "2026-03-13T192553Z_WxTTriage_results_Activity_PackageId";
        let colliding_name = format!("{long}_localadmin.csv");
        assert_eq!(
            colliding_name,
            format!("{short}_PackageId_localadmin.csv"),
            "the fixture only tests anything if the two names really are one \
             string"
        );
        let router_wrote = vec![
            write(
                &category.join("PerUser/Activity"),
                &colliding_name,
                "Id,ActivityType\n1,x\n",
            ),
            write(
                &category.join("PerUser/Activity_PackageId"),
                &colliding_name,
                "Id,Platform\n1,windows\n2,windows\n",
            ),
        ];

        let short_report = merge_per_user(category, short, false, &router_wrote, Some("Activity"))
            .unwrap()
            .unwrap();
        assert_eq!(short_report.sources, 1, "only the Activity slice");
        assert_eq!(short_report.rows, 1);
        let merged = std::fs::read_to_string(category.join(format!("{short}.csv"))).unwrap();
        assert!(
            merged.starts_with("Id,ActivityType,TriageUser\n"),
            "the Activity dataset must publish its own schema, got {merged}"
        );
        assert!(
            merged.contains(",x,PackageId_localadmin\n"),
            "and the profile keeps its whole name, got {merged}"
        );

        let long_report = merge_per_user(
            category,
            long,
            false,
            &router_wrote,
            Some("Activity_PackageId"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(long_report.sources, 1);
        assert_eq!(long_report.rows, 2);
        let merged = std::fs::read_to_string(category.join(format!("{long}.csv"))).unwrap();
        assert!(
            merged.starts_with("Id,Platform,TriageUser\n"),
            "got {merged}"
        );
        assert!(
            merged.contains(",windows,localadmin\n"),
            "the PackageId dataset's rows belong to localadmin, not to \
             PackageId_localadmin: got {merged}"
        );
    }

    /// The same guarantee for the NDJSON path, which is a structural mirror
    /// of the CSV one and shares the decoder. NDJSON is the half that
    /// corrupted silently: it has no header for a foreign schema to clash
    /// with, so the wrong dataset's records merged in without complaint.
    #[test]
    fn a_profile_named_after_a_sibling_discriminator_stays_with_its_own_dataset_in_ndjson() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let short = "2026-03-13T192553Z_WxTTriage_results_Activity";
        let long = "2026-03-13T192553Z_WxTTriage_results_Activity_PackageId";
        let colliding_name = format!("{long}_localadmin.json");
        assert_eq!(
            colliding_name,
            format!("{short}_PackageId_localadmin.json"),
            "the fixture only tests anything if the two names really are one \
             string"
        );
        let router_wrote = vec![
            write(
                &category.join("PerUser/Activity"),
                &colliding_name,
                "{\"Id\":1}\n",
            ),
            write(
                &category.join("PerUser/Activity_PackageId"),
                &colliding_name,
                "{\"Id\":1,\"Platform\":\"windows\"}\n",
            ),
        ];

        let report = merge_per_user_ndjson(category, short, false, &router_wrote, Some("Activity"))
            .unwrap()
            .unwrap();
        assert_eq!(report.sources, 1);
        assert_eq!(report.rows, 1);
        let merged = std::fs::read_to_string(category.join(format!("{short}.json"))).unwrap();
        assert!(
            !merged.contains("Platform"),
            "the PackageId dataset's field must not appear in the Activity \
             merge: got {merged}"
        );
        assert!(
            merged.contains("\"TriageUser\":\"PackageId_localadmin\""),
            "got {merged}"
        );

        let report = merge_per_user_ndjson(
            category,
            long,
            false,
            &router_wrote,
            Some("Activity_PackageId"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(report.sources, 1);
        let merged = std::fs::read_to_string(category.join(format!("{long}.json"))).unwrap();
        assert!(
            merged.contains("\"TriageUser\":\"localadmin\""),
            "got {merged}"
        );
    }

    /// C2: a merge that fails part-way must leave nothing at the
    /// destination. Before the staged publish, the destination was truncated
    /// and opened first, so a mid-merge failure left a partial file carrying
    /// the wrong schema at the category root -- absent from the manifest but
    /// picked up by `write_output_hashes`' directory walk, which made the
    /// chain-of-custody record attest to it.
    #[test]
    fn a_failed_csv_merge_leaves_nothing_at_the_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_LETriage_results";
        // Sorted first, so it writes the header and some rows before the
        // second source's mismatched header aborts the merge.
        let router_wrote = vec![
            write(
                &category.join("PerUser"),
                &format!("{stem}_asmith.csv"),
                "Path,Name\nC:\\a,a\n",
            ),
            write(
                &category.join("PerUser"),
                &format!("{stem}_jdoe.csv"),
                "Different,Header\nx,y\n",
            ),
        ];

        assert!(merge_per_user(category, stem, false, &router_wrote, None).is_err());
        assert!(
            !category.join(format!("{stem}.csv")).exists(),
            "a failed merge must publish nothing"
        );
        assert!(
            no_leftovers(category),
            "the staging temporary must be cleaned up: a stray file at the \
             category root would be hashed into OutputHashes.txt"
        );
    }

    /// The NDJSON counterpart of the C2 guarantee. A line that is not a JSON
    /// object is this path's mid-merge hard error.
    #[test]
    fn a_failed_ndjson_merge_leaves_nothing_at_the_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_LETriage_results";
        let router_wrote = vec![
            write(
                &category.join("PerUser"),
                &format!("{stem}_asmith.json"),
                "{\"Path\":\"C:\\\\a\"}\n",
            ),
            write(
                &category.join("PerUser"),
                &format!("{stem}_jdoe.json"),
                "not json at all\n",
            ),
        ];

        assert!(merge_per_user_ndjson(category, stem, false, &router_wrote, None).is_err());
        assert!(
            !category.join(format!("{stem}.json")).exists(),
            "a failed merge must publish nothing"
        );
        assert!(
            no_leftovers(category),
            "the staging temporary must be cleaned up"
        );
    }

    /// True when the category root holds nothing but the `PerUser` directory
    /// -- no published file and no leftover staging temporary (which
    /// `OutputLayout::create_staged` names `.<file>.tmp-<pid>-<n>`).
    fn no_leftovers(category: &std::path::Path) -> bool {
        std::fs::read_dir(category)
            .unwrap()
            .all(|e| e.unwrap().file_name() == std::ffi::OsStr::new("PerUser"))
    }

    /// Minor #4 from the final review: a per-user file with a header and no
    /// data rows is a real case (a profile the tool found nothing for). It
    /// must contribute its header to the merge's compatibility check and
    /// zero rows -- not be skipped, and not fail.
    #[test]
    fn a_header_only_per_user_file_contributes_no_rows_and_no_error() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_LETriage_results";
        // Sorted first, so it is also the source that sets the merged
        // header: an empty file must be able to do that correctly.
        let router_wrote = vec![
            write(
                &category.join("PerUser"),
                &format!("{stem}_asmith.csv"),
                "Path,Name\n",
            ),
            write(
                &category.join("PerUser"),
                &format!("{stem}_jdoe.csv"),
                "Path,Name\nC:\\a,a\n",
            ),
        ];

        let report = merge_per_user(category, stem, false, &router_wrote, None)
            .unwrap()
            .unwrap();
        assert_eq!(report.sources, 2, "the empty slice is still a source");
        assert_eq!(report.rows, 1);
        let merged = std::fs::read_to_string(category.join(format!("{stem}.csv"))).unwrap();
        assert_eq!(merged, "Path,Name,TriageUser\nC:\\a,a,jdoe\n");
    }

    /// Reproduces the `UserElseSystem` router sequence: the system-scope
    /// slice lands at the category root (`<stem>.csv`) before the merge
    /// post-pass runs, on the same path the merged file must occupy.
    #[test]
    fn a_pre_existing_category_root_file_is_reclaimed_as_the_system_slice() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_LETriage_results";
        let mut router_wrote = vec![write(
            &category.join("PerUser"),
            &format!("{stem}_jdoe.csv"),
            "Path,Name\nC:\\a,a\n",
        )];
        // The router's system-scope write, sitting at the exact path the
        // merged file must end up at.
        router_wrote.extend(write_router_system_slice(
            category,
            stem,
            "csv",
            "Path,Name\nC:\\sys,s\n",
        ));

        let report = merge_per_user(category, stem, false, &router_wrote, None)
            .unwrap()
            .unwrap();
        assert_eq!(report.sources, 2, "system slice plus the one user slice");
        assert_eq!(report.rows, 2);

        // The system slice now lives in PerUser/ like any other identity,
        // under RECLAIM_LABEL rather than "system" (never a real account's
        // label -- see RECLAIM_LABEL's doc comment).
        assert!(category
            .join("PerUser")
            .join(format!("{stem}_{RECLAIM_LABEL}.csv"))
            .is_file());

        let merged = std::fs::read_to_string(category.join(format!("{stem}.csv"))).unwrap();
        assert!(merged.starts_with("Path,Name,TriageUser\n"), "got {merged}");
        assert!(merged.contains("C:\\a,a,jdoe\n"), "got {merged}");
        // The TriageUser column value is still the plain "system", decoupled
        // from the collision-proof on-disk label.
        assert!(merged.contains("C:\\sys,s,system\n"), "got {merged}");
    }

    /// Codex P1: a per-user slice this run did not publish is a leftover --
    /// a profile since removed from the host, or an artifact class that
    /// stopped being collected -- and must not enter the merged file under
    /// its old identity, which `write_output_hashes` would then attest to.
    ///
    /// Both halves matter: the leftover is excluded, *and* the reclaimed
    /// system slice is still admitted even though it too is absent from
    /// `router_wrote` (it is created during this merge, after the router
    /// finished). A filter that simply intersected the directory with
    /// `router_wrote` would drop the reclaim and silently delete every
    /// `UserElseSystem` tool's system rows.
    #[test]
    fn a_leftover_per_user_slice_is_not_a_source_but_the_reclaim_still_is() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_LETriage_results";
        // The previous run's slice for a profile this run no longer has:
        // written, but never published, so nothing names it.
        write(
            &category.join("PerUser"),
            &format!("{stem}_ghost.csv"),
            "Path,Name\nC:\\ghost,ghost\n",
        );
        let mut router_wrote = vec![write(
            &category.join("PerUser"),
            &format!("{stem}_jdoe.csv"),
            "Path,Name\nC:\\a,a\n",
        )];
        router_wrote.extend(write_router_system_slice(
            category,
            stem,
            "csv",
            "Path,Name\nC:\\sys,s\n",
        ));

        let report = merge_per_user(category, stem, true, &router_wrote, None)
            .unwrap()
            .unwrap();
        assert_eq!(
            report.sources, 2,
            "jdoe and the reclaimed system slice -- not the leftover"
        );
        let merged = std::fs::read_to_string(category.join(format!("{stem}.csv"))).unwrap();
        assert!(
            !merged.contains("ghost"),
            "a profile this run never wrote must not reappear: got {merged}"
        );
        assert!(merged.contains("C:\\a,a,jdoe\n"), "got {merged}");
        assert!(
            merged.contains("C:\\sys,s,system\n"),
            "the in-merge reclaim is still admitted: got {merged}"
        );
        // The leftover is left exactly where it was: this is a source-
        // selection rule, not a cleanup pass.
        assert!(category
            .join("PerUser")
            .join(format!("{stem}_ghost.csv"))
            .is_file());
    }

    /// NDJSON parity for
    /// `a_leftover_per_user_slice_is_not_a_source_but_the_reclaim_still_is`.
    /// NDJSON is the half that corrupts silently -- no header to disagree --
    /// so the leftover's records simply joined the fresh ones.
    #[test]
    fn ndjson_a_leftover_per_user_slice_is_not_a_source_but_the_reclaim_still_is() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "S_T_results";
        write(
            &category.join("PerUser"),
            &format!("{stem}_ghost.json"),
            "{\"Name\":\"ghost\"}\n",
        );
        let mut router_wrote = vec![write(
            &category.join("PerUser"),
            &format!("{stem}_jdoe.json"),
            "{\"Name\":\"a\"}\n",
        )];
        router_wrote.extend(write_router_system_slice(
            category,
            stem,
            "json",
            "{\"Name\":\"sys\"}\n",
        ));

        let report = merge_per_user_ndjson(category, stem, true, &router_wrote, None)
            .unwrap()
            .unwrap();
        assert_eq!(report.sources, 2);
        assert_eq!(report.rows, 2, "the leftover's record is not appended");
        let merged = std::fs::read_to_string(category.join(format!("{stem}.json"))).unwrap();
        assert!(!merged.contains("ghost"), "got {merged}");
        assert!(merged.contains("\"TriageUser\":\"jdoe\""), "got {merged}");
        assert!(
            merged.contains("\"TriageUser\":\"system\""),
            "the in-merge reclaim is still admitted: got {merged}"
        );
    }

    /// The leftover-only case: every profile this stem had last run is gone,
    /// so this run published no per-user slice for it. The merge must decline
    /// entirely rather than rebuild a merged file out of leftovers -- and,
    /// per the `sources.is_empty()` gate's reasoning, must leave any
    /// category-root file alone rather than reclaiming it.
    #[test]
    fn leftovers_alone_do_not_make_a_merge() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_LETriage_results";
        write(
            &category.join("PerUser"),
            &format!("{stem}_ghost.csv"),
            "Path,Name\nC:\\ghost,ghost\n",
        );
        let router_wrote =
            write_router_system_slice(category, stem, "csv", "Path,Name\nC:\\sys,s\n");

        assert!(merge_per_user(category, stem, true, &router_wrote, None)
            .unwrap()
            .is_none());
        // The system-scope output stays at the category root, Zimmerman-exact
        // with no TriageUser column -- the same outcome a tool that produced
        // system-only output this run gets.
        assert_eq!(
            std::fs::read_to_string(category.join(format!("{stem}.csv"))).unwrap(),
            "Path,Name\nC:\\sys,s\n"
        );
    }

    /// `SystemWide` tools never write a `PerUser/` directory, so the early
    /// return above still leaves their single category-root file untouched
    /// -- no reclaim, no `PerUser/`, no `TriageUser` column.
    #[test]
    fn a_system_wide_tools_output_is_left_alone_with_no_per_user_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_MFTTriage_results";
        write(category, &format!("{stem}.csv"), "Path,Name\nC:\\a,a\n");

        assert!(merge_per_user(category, stem, false, &[], None)
            .unwrap()
            .is_none());
        assert!(!category.join("PerUser").exists());
        let untouched = std::fs::read_to_string(category.join(format!("{stem}.csv"))).unwrap();
        assert_eq!(untouched, "Path,Name\nC:\\a,a\n");
    }

    /// `PerUser/` is a shared *category* directory (e.g. `FileSystem/` holds
    /// `mft`/`pe` alongside `le`/`jle`), not a per-tool one -- so a
    /// `SystemWide` tool's own category-root file must be left alone even
    /// when a `UserElseSystem` sibling in the same category has already
    /// created `PerUser/` for a different stem. Reproduces the real-run
    /// finding: `--only pe,le` swept PETriage's output into `PerUser/` and
    /// re-emitted it with a bogus `TriageUser` column before this gate.
    #[test]
    fn a_system_wide_tools_output_is_left_alone_when_a_sibling_shares_per_user() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let pe_stem = "2026-03-13T192553Z_PETriage_results";
        let le_stem = "2026-03-13T192553Z_LETriage_results";

        // PETriage (SystemWide): its only output, at the category root.
        write(
            category,
            &format!("{pe_stem}.csv"),
            "Path,Name\nC:\\pe,pe\n",
        );
        // LETriage (UserElseSystem) sharing the same category, already having
        // created PerUser/ for its own, unrelated stem.
        write(
            &category.join("PerUser"),
            &format!("{le_stem}_jdoe.csv"),
            "Path,Name\nC:\\a,a\n",
        );

        assert!(
            merge_per_user(category, pe_stem, false, &[], None)
                .unwrap()
                .is_none(),
            "no per-user slice exists for pe_stem, so nothing should be reclaimed"
        );
        assert!(
            !category
                .join("PerUser")
                .join(format!("{pe_stem}_system.csv"))
                .exists(),
            "PETriage's output must never be swept into PerUser/"
        );
        let untouched = std::fs::read_to_string(category.join(format!("{pe_stem}.csv"))).unwrap();
        assert_eq!(
            untouched, "Path,Name\nC:\\pe,pe\n",
            "PETriage's root file must stay byte-exact, no TriageUser column"
        );
    }

    /// The reviewer's reproduction, at the unit level: `Users/System`
    /// (capital S, case preserved by attribution) is a real, distinct
    /// account from the generic system-scope artifact this fixture also
    /// carries. Before this fix, both wanted the exact same
    /// `PerUser/<stem>_system.csv` path -- exact on a case-sensitive
    /// filesystem (`_System` vs `_system` are different names there, so no
    /// collision *there*, but a case-folded fix would have wrongly refused
    /// this safe case) and silently on a case-insensitive one (`_System` and
    /// `_system` are the *same* file there, so the real account's row was
    /// destroyed and the merge emitted the reclaimed row twice). Reclaiming
    /// under `RECLAIM_LABEL` instead means the two paths are never equal on
    /// any filesystem, so this is not a comparison anyone has to get right
    /// per-platform -- it cannot arise.
    #[test]
    fn a_real_account_named_system_case_preserved_never_collides_with_the_reclaim() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_LETriage_results";
        let mut router_wrote = vec![
            write(
                &category.join("PerUser"),
                &format!("{stem}_System.csv"),
                "Path,Name\nC:\\real,real\n",
            ),
            write(
                &category.join("PerUser"),
                &format!("{stem}_jdoe.csv"),
                "Path,Name\nC:\\a,a\n",
            ),
        ];
        // The router's genuine, unrelated system-scope write.
        router_wrote.extend(write_router_system_slice(
            category,
            stem,
            "csv",
            "Path,Name\nC:\\sys,s\n",
        ));

        let report = merge_per_user(category, stem, true, &router_wrote, None)
            .unwrap()
            .unwrap();
        assert_eq!(
            report.sources, 3,
            "System, jdoe, and the reclaimed system slice, as three distinct sources"
        );
        assert_eq!(report.rows, 3);

        // The real account's own file is never touched by the reclaim.
        assert_eq!(
            std::fs::read_to_string(category.join("PerUser").join(format!("{stem}_System.csv")))
                .unwrap(),
            "Path,Name\nC:\\real,real\n"
        );
        // The reclaim lives at its own, permanently distinct path.
        assert!(category
            .join("PerUser")
            .join(format!("{stem}_{RECLAIM_LABEL}.csv"))
            .is_file());

        let merged = std::fs::read_to_string(category.join(format!("{stem}.csv"))).unwrap();
        let body: Vec<&str> = merged.lines().skip(1).collect();
        assert_eq!(
            body.len(),
            3,
            "one row per source, no duplicate: got {body:?}"
        );
        assert!(
            body.iter().any(|l| l.ends_with(",System")),
            "the real account's row, case preserved: got {body:?}"
        );
        assert!(body.iter().any(|l| l.ends_with(",jdoe")), "got {body:?}");
        assert_eq!(
            body.iter().filter(|l| l.ends_with(",system")).count(),
            1,
            "exactly one system-scope row from the reclaim, not duplicated: got {body:?}"
        );
    }

    /// Defensive backstop only (see `RECLAIM_LABEL`'s doc comment): forces
    /// the "invariant violated" branch by claiming, via `router_wrote`, that
    /// the router itself wrote the reclaim's destination this run --
    /// something that cannot happen with any real account, since no real
    /// name sanitizes to `RECLAIM_LABEL`. Exists to prove the backstop still
    /// refuses loudly rather than silently trusting a corrupted signal, not
    /// to model a scenario that can occur in practice.
    #[test]
    fn the_invariant_backstop_refuses_if_router_wrote_ever_claims_the_reclaim_path() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_LETriage_results";
        let system_slice = write(
            &category.join("PerUser"),
            &format!("{stem}_{RECLAIM_LABEL}.csv"),
            "Path,Name\nC:\\x,x\n",
        );
        let mut router_wrote = vec![write(
            &category.join("PerUser"),
            &format!("{stem}_jdoe.csv"),
            "Path,Name\nC:\\a,a\n",
        )];
        router_wrote.extend(write_router_system_slice(
            category,
            stem,
            "csv",
            "Path,Name\nC:\\new,new\n",
        ));
        // The corrupted signal this backstop exists for: `router_wrote`
        // naming the reclaim's own destination.
        router_wrote.push(system_slice);

        let err = merge_per_user(category, stem, true, &router_wrote, None).unwrap_err();
        assert!(
            err.to_string().contains("internal invariant violated"),
            "got {err}"
        );
    }

    /// The same backstop for the case that made it necessary to ask
    /// unconditionally: `router_wrote` names the reclaim's destination but no
    /// file is there. `published_per_user_sources` does no I/O, so it yields
    /// that claimed path as a `RECLAIM_LABEL` source anyway; the rename then
    /// creates the file and the reclaim pushes a *second* entry for the one
    /// path, emitting the system-scope rows twice -- the duplication the
    /// case-folding round fixed. While the backstop was nested inside
    /// `system_slice.exists()` it never fired here. It must refuse, exactly
    /// as it does when the file is present.
    #[test]
    fn the_invariant_backstop_refuses_a_claimed_reclaim_path_that_is_not_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_LETriage_results";
        let mut router_wrote = vec![write(
            &category.join("PerUser"),
            &format!("{stem}_jdoe.csv"),
            "Path,Name\nC:\\a,a\n",
        )];
        router_wrote.extend(write_router_system_slice(
            category,
            stem,
            "csv",
            "Path,Name\nC:\\new,new\n",
        ));
        // Claimed as published, never written: the second half of the double
        // invariant violation this backstop exists for.
        let claimed = category
            .join("PerUser")
            .join(format!("{stem}_{RECLAIM_LABEL}.csv"));
        assert!(
            !claimed.exists(),
            "the fixture's whole point is its absence"
        );
        router_wrote.push(claimed);

        let err = merge_per_user(category, stem, true, &router_wrote, None).unwrap_err();
        assert!(
            err.to_string().contains("internal invariant violated"),
            "got {err}"
        );
    }

    /// NDJSON parity for
    /// `the_invariant_backstop_refuses_a_claimed_reclaim_path_that_is_not_on_disk`.
    /// NDJSON is the half that duplicates silently -- no header to disagree
    /// -- so it is the one that would have shipped the doubled rows.
    #[test]
    fn ndjson_the_invariant_backstop_refuses_a_claimed_reclaim_path_that_is_not_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "S_T_results";
        let mut router_wrote = vec![write(
            &category.join("PerUser"),
            &format!("{stem}_jdoe.json"),
            "{\"Name\":\"a\"}\n",
        )];
        router_wrote.extend(write_router_system_slice(
            category,
            stem,
            "json",
            "{\"Name\":\"new\"}\n",
        ));
        let claimed = category
            .join("PerUser")
            .join(format!("{stem}_{RECLAIM_LABEL}.json"));
        assert!(
            !claimed.exists(),
            "the fixture's whole point is its absence"
        );
        router_wrote.push(claimed);

        let err = merge_per_user_ndjson(category, stem, true, &router_wrote, None).unwrap_err();
        assert!(
            err.to_string().contains("internal invariant violated"),
            "got {err}"
        );
    }

    /// The mirror image of `a_pre_existing_category_root_file_is_reclaimed_as_the_system_slice`:
    /// a category-root file the router did *not* publish this run is a
    /// previous run's output -- most often that run's own merged file -- and
    /// must never be reclaimed. Folding it back in re-attributed its rows to
    /// `system` (NDJSON) or failed on the `TriageUser` column the previous
    /// merge appended (CSV), in both cases *after* the rename had already
    /// carried the only copy off the category root.
    #[test]
    fn a_category_root_file_the_router_did_not_write_is_never_reclaimed() {
        for overwrite in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let category = tmp.path();
            let stem = "2026-03-13T192553Z_LETriage_results";
            let router_wrote = vec![write(
                &category.join("PerUser"),
                &format!("{stem}_jdoe.csv"),
                "Path,Name\nC:\\a,a\n",
            )];
            // A previous run's merged file: this stem's own output, already
            // carrying the trailing column, and named by nothing in
            // `router_wrote` because no router wrote it this run.
            let previous = "Path,Name,TriageUser\nC:\\a,a,jdoe\n";
            write(category, &format!("{stem}.csv"), previous);

            let result = merge_per_user(category, stem, overwrite, &router_wrote, None);

            assert!(
                !category
                    .join("PerUser")
                    .join(format!("{stem}_{RECLAIM_LABEL}.csv"))
                    .exists(),
                "overwrite={overwrite}: the previous merged file must not be \
                 moved into PerUser/"
            );
            let merged = std::fs::read_to_string(category.join(format!("{stem}.csv"))).unwrap();
            if overwrite {
                assert_eq!(result.unwrap().unwrap().sources, 1, "the one user slice");
                assert_eq!(
                    merged, "Path,Name,TriageUser\nC:\\a,a,jdoe\n",
                    "rebuilt from the per-user slice alone, one TriageUser column"
                );
            } else {
                assert!(
                    result.is_err(),
                    "an ordinary output collision without --overwrite"
                );
                assert_eq!(merged, previous, "and the previous file is untouched");
            }
        }
    }

    /// NDJSON parity for
    /// `a_category_root_file_the_router_did_not_write_is_never_reclaimed`.
    /// This is the side that corrupted silently: NDJSON has no header to
    /// disagree, so the previous run's rows were re-read, relabelled
    /// `system`, and appended to the fresh ones.
    #[test]
    fn ndjson_a_category_root_file_the_router_did_not_write_is_never_reclaimed() {
        for overwrite in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let category = tmp.path();
            let stem = "S_T_results";
            let router_wrote = vec![write(
                &category.join("PerUser"),
                &format!("{stem}_jdoe.json"),
                "{\"Name\":\"a\"}\n",
            )];
            let previous = "{\"Name\":\"a\",\"TriageUser\":\"jdoe\"}\n";
            write(category, &format!("{stem}.json"), previous);

            let result = merge_per_user_ndjson(category, stem, overwrite, &router_wrote, None);

            assert!(
                !category
                    .join("PerUser")
                    .join(format!("{stem}_{RECLAIM_LABEL}.json"))
                    .exists(),
                "overwrite={overwrite}: the previous merged file must not be \
                 moved into PerUser/"
            );
            let merged = std::fs::read_to_string(category.join(format!("{stem}.json"))).unwrap();
            if overwrite {
                let report = result.unwrap().unwrap();
                assert_eq!(report.sources, 1, "the one user slice");
                assert_eq!(report.rows, 1, "not doubled by the previous run's rows");
                assert_eq!(
                    merged, previous,
                    "jdoe's row rebuilt as jdoe's, never relabelled system"
                );
            } else {
                assert!(
                    result.is_err(),
                    "an ordinary output collision without --overwrite"
                );
                assert_eq!(merged, previous, "and the previous file is untouched");
            }
        }
    }

    /// A `PerUser/<stem>_system.csv` the router did *not* write this run is
    /// a leftover artifact of a *previous* run's own reclaim -- derived
    /// output of this same pipeline, exactly what `--overwrite` exists to
    /// replace. Without it, this is refused like any other output
    /// collision; with it, the rename below replaces the stale file.
    #[test]
    fn a_stale_leftover_system_slice_is_replaced_only_with_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "2026-03-13T192553Z_LETriage_results";
        write(
            &category.join("PerUser"),
            &format!("{stem}_{RECLAIM_LABEL}.csv"),
            "Path,Name\nC:\\old,old\n",
        );
        let mut router_wrote = vec![write(
            &category.join("PerUser"),
            &format!("{stem}_jdoe.csv"),
            "Path,Name\nC:\\a,a\n",
        )];
        router_wrote.extend(write_router_system_slice(
            category,
            stem,
            "csv",
            "Path,Name\nC:\\new,new\n",
        ));

        // The reclaim's own destination is not named in router_wrote:
        // nothing the router did this run touched it, so without --overwrite
        // this must still refuse, exactly like any other pre-existing output.
        assert!(merge_per_user(category, stem, false, &router_wrote, None).is_err());
        assert_eq!(
            std::fs::read_to_string(
                category
                    .join("PerUser")
                    .join(format!("{stem}_{RECLAIM_LABEL}.csv"))
            )
            .unwrap(),
            "Path,Name\nC:\\old,old\n",
            "refused without --overwrite: the stale slice must be untouched"
        );

        let report = merge_per_user(category, stem, true, &router_wrote, None)
            .unwrap()
            .unwrap();
        assert_eq!(report.sources, 2);
        let merged = std::fs::read_to_string(category.join(format!("{stem}.csv"))).unwrap();
        assert!(merged.contains("C:\\new,new,system\n"), "got {merged}");
        assert!(
            !merged.contains("C:\\old"),
            "the stale leftover was replaced"
        );
    }

    #[test]
    fn a_header_mismatch_fails_rather_than_interleaving_columns() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "S_T_results";
        let router_wrote = vec![
            write(
                &category.join("PerUser"),
                &format!("{stem}_a.csv"),
                "X,Y\n1,2\n",
            ),
            write(
                &category.join("PerUser"),
                &format!("{stem}_b.csv"),
                "Y,X\n3,4\n",
            ),
        ];
        assert!(merge_per_user(category, stem, false, &router_wrote, None).is_err());
    }

    #[test]
    fn no_per_user_directory_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(merge_per_user(tmp.path(), "S_T_results", false, &[], None)
            .unwrap()
            .is_none());
    }

    #[test]
    fn the_user_is_recovered_from_the_filename() {
        let stem = "2026-03-13T192553Z_LETriage_results";
        assert_eq!(
            user_from_per_user_filename(&format!("{stem}_jdoe.csv"), stem).as_deref(),
            Some("jdoe")
        );
        // A user whose sanitized name contains an underscore survives.
        assert_eq!(
            user_from_per_user_filename(&format!("{stem}_j_doe.csv"), stem).as_deref(),
            Some("j_doe")
        );
        // Files that are not this stem's per-user output are not claimed.
        assert_eq!(user_from_per_user_filename("unrelated.csv", stem), None);
        assert_eq!(
            user_from_per_user_filename(&format!("{stem}.csv"), stem),
            None
        );
    }

    #[test]
    fn ndjson_merge_adds_the_user_as_a_field() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "S_T_results";
        let router_wrote = vec![write(
            &category.join("PerUser"),
            &format!("{stem}_jdoe.json"),
            "{\"Name\":\"a\"}\n{\"Name\":\"b\"}\n",
        )];
        let report = merge_per_user_ndjson(category, stem, false, &router_wrote, None)
            .unwrap()
            .unwrap();
        assert_eq!(report.rows, 2);
        let merged = std::fs::read_to_string(category.join(format!("{stem}.json"))).unwrap();
        assert_eq!(
            merged,
            "{\"Name\":\"a\",\"TriageUser\":\"jdoe\"}\n{\"Name\":\"b\",\"TriageUser\":\"jdoe\"}\n"
        );
    }

    /// NDJSON counterpart of `a_pre_existing_category_root_file_is_reclaimed_as_the_system_slice`.
    #[test]
    fn ndjson_reclaims_a_pre_existing_category_root_file_as_the_system_slice() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "S_T_results";
        let mut router_wrote = vec![write(
            &category.join("PerUser"),
            &format!("{stem}_jdoe.json"),
            "{\"Name\":\"a\"}\n",
        )];
        router_wrote.extend(write_router_system_slice(
            category,
            stem,
            "json",
            "{\"Name\":\"sys\"}\n",
        ));

        let report = merge_per_user_ndjson(category, stem, false, &router_wrote, None)
            .unwrap()
            .unwrap();
        assert_eq!(report.sources, 2);
        assert_eq!(report.rows, 2);
        assert!(category
            .join("PerUser")
            .join(format!("{stem}_{RECLAIM_LABEL}.json"))
            .is_file());
        let merged = std::fs::read_to_string(category.join(format!("{stem}.json"))).unwrap();
        assert!(merged.contains("\"TriageUser\":\"system\""), "got {merged}");
        assert!(merged.contains("\"TriageUser\":\"jdoe\""), "got {merged}");
    }

    #[test]
    fn a_malformed_ndjson_line_fails_rather_than_being_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "S_T_results";
        let router_wrote = vec![write(
            &category.join("PerUser"),
            &format!("{stem}_a.json"),
            "{not json}\n",
        )];
        assert!(merge_per_user_ndjson(category, stem, false, &router_wrote, None).is_err());
    }

    /// NDJSON parity for `a_real_account_named_system_case_preserved_never_collides_with_the_reclaim`.
    /// The CSV and NDJSON reclaim paths are parity-identical, and that
    /// parity (asserted only "by inspection", not by a test) is exactly what
    /// let the case-folding hole survive an earlier round untested on this
    /// side.
    #[test]
    fn ndjson_a_real_account_named_system_never_collides_with_the_reclaim() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "S_T_results";
        let mut router_wrote = vec![
            write(
                &category.join("PerUser"),
                &format!("{stem}_System.json"),
                "{\"Name\":\"real\"}\n",
            ),
            write(
                &category.join("PerUser"),
                &format!("{stem}_jdoe.json"),
                "{\"Name\":\"a\"}\n",
            ),
        ];
        router_wrote.extend(write_router_system_slice(
            category,
            stem,
            "json",
            "{\"Name\":\"sys\"}\n",
        ));

        let report = merge_per_user_ndjson(category, stem, true, &router_wrote, None)
            .unwrap()
            .unwrap();
        assert_eq!(report.sources, 3);
        assert_eq!(report.rows, 3);

        // The real account's own file is never touched by the reclaim.
        assert_eq!(
            std::fs::read_to_string(category.join("PerUser").join(format!("{stem}_System.json")))
                .unwrap(),
            "{\"Name\":\"real\"}\n"
        );
        assert!(category
            .join("PerUser")
            .join(format!("{stem}_{RECLAIM_LABEL}.json"))
            .is_file());

        let merged = std::fs::read_to_string(category.join(format!("{stem}.json"))).unwrap();
        assert_eq!(
            merged.lines().count(),
            3,
            "one line per source, no duplicate: got {merged}"
        );
        assert!(
            merged.contains("\"TriageUser\":\"System\""),
            "the real account's row, case preserved: got {merged}"
        );
        assert!(merged.contains("\"TriageUser\":\"jdoe\""), "got {merged}");
        assert_eq!(
            merged.matches("\"TriageUser\":\"system\"").count(),
            1,
            "exactly one system-scope row from the reclaim, not duplicated: got {merged}"
        );
    }

    /// NDJSON parity for `a_stale_leftover_system_slice_is_replaced_only_with_overwrite`.
    #[test]
    fn ndjson_a_stale_leftover_system_slice_is_replaced_only_with_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let category = tmp.path();
        let stem = "S_T_results";
        write(
            &category.join("PerUser"),
            &format!("{stem}_{RECLAIM_LABEL}.json"),
            "{\"Name\":\"old\"}\n",
        );
        let mut router_wrote = vec![write(
            &category.join("PerUser"),
            &format!("{stem}_jdoe.json"),
            "{\"Name\":\"a\"}\n",
        )];
        router_wrote.extend(write_router_system_slice(
            category,
            stem,
            "json",
            "{\"Name\":\"new\"}\n",
        ));

        assert!(merge_per_user_ndjson(category, stem, false, &router_wrote, None).is_err());
        assert_eq!(
            std::fs::read_to_string(
                category
                    .join("PerUser")
                    .join(format!("{stem}_{RECLAIM_LABEL}.json"))
            )
            .unwrap(),
            "{\"Name\":\"old\"}\n",
            "refused without --overwrite: the stale slice must be untouched"
        );

        let report = merge_per_user_ndjson(category, stem, true, &router_wrote, None)
            .unwrap()
            .unwrap();
        assert_eq!(report.sources, 2);
        let merged = std::fs::read_to_string(category.join(format!("{stem}.json"))).unwrap();
        assert!(
            merged.contains("\"Name\":\"new\",\"TriageUser\":\"system\""),
            "got {merged}"
        );
        assert!(
            !merged.contains("\"old\""),
            "the stale leftover was replaced"
        );
    }
}
