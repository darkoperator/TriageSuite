use crate::attribution::Identity;
use crate::error::TriageError;
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct StagedFile {
    pub file: File,
    pub temporary: PathBuf,
    pub destination: PathBuf,
}

/// Selects how output paths are arranged on disk.
///
/// `Nested` builds the spec section 4.3 tree
/// (`<root>/<BinaryName>/users/<name>/...`); `Flat` writes every file directly
/// under `<root>` with the identity encoded in the filename; `Velo` writes
/// system-scope output directly under `<root>` and per-user output under
/// `<root>/PerUser/[<dataset discriminator>/]` with the user suffixed onto the
/// filename (see `OutputLayout::for_velo_dataset`).
///
/// `Velo` carries no category or host: the orchestrator bakes
/// `Processed-<HOST>-<stamp>/<Category>` into the `root` it passes in, which
/// keeps this enum `Copy` and keeps `triage-core` ignorant of forensic
/// categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputLayoutMode {
    Flat,
    Nested,
    Velo,
}

/// Builds and creates output paths per spec section 4.3:
/// `<root>/<BinaryName>/users/<name>/...`, `<root>/<BinaryName>/system/...`,
/// `<root>/<BinaryName>/users/unknown/...`.
///
/// `Clone` so a caller can keep a detached copy of the routing rules --
/// `SideCarReference` does, to name a side-car in a CSV column while the
/// router itself is borrowed for writing.
#[derive(Clone)]
pub struct OutputLayout {
    root: PathBuf,
    binary_name: String,
    overwrite: bool,
    mode: OutputLayoutMode,
    /// The dataset discriminator whose per-user slices this layout writes,
    /// under `OutputLayoutMode::Velo` only. See `for_velo_dataset`.
    velo_dataset_dir: Option<String>,
}

impl OutputLayout {
    pub fn new(root: &Path, binary_name: &str, overwrite: bool, mode: OutputLayoutMode) -> Self {
        Self {
            root: root.to_path_buf(),
            binary_name: binary_name.to_string(),
            overwrite,
            mode,
            velo_dataset_dir: None,
        }
    }

    /// The same layout, writing one dataset's per-user slices into their own
    /// `PerUser/<discriminator>/` directory.
    ///
    /// `discriminator` is the `<Disc>` of a Velo name
    /// `<Tool>_results[_<Disc>]` (`crate::output::router::velo_discriminator`),
    /// and `None` -- a dataset that has none -- keeps writing straight into
    /// `PerUser/`, exactly as the filename convention itself leaves the
    /// discriminator out.
    ///
    /// This is what makes a per-user filename mean one dataset rather than
    /// two. A per-user name is `<stem>_<label>`, both halves of which are
    /// `_`-joined and neither of which is escapable: a label is a sanitized
    /// profile name (`crate::attribution::sanitize_component` replaces only
    /// what a filesystem forbids, so every character it emits is one some
    /// real account name can also produce), and a stem is
    /// `<stamp>_<Tool>_results[_<Disc>]`, so a tool with both a bare and a
    /// discriminated dataset has one stem that is a literal `<other>_` prefix
    /// of the other. A profile named `Downloads_alice` therefore produced
    /// exactly the filename the `Downloads` dataset produces for a profile
    /// named `alice` -- one file for two datasets, so one of them was refused
    /// (no `--overwrite`) or silently clobbered, and whichever survived was
    /// merged under the other dataset's schema. No parse of that name can
    /// separate the two, because the two are the same string.
    ///
    /// A directory can: the discriminator moves out of the name into a path
    /// component of its own, so a stem and its discriminated sibling never
    /// share a directory, and within one directory no label can reach across
    /// to another stem. `every_per_user_directory_decodes_to_exactly_one_stem`
    /// (`triage-orchestrator/tests/velo_names.rs`) is the guard over the
    /// residue -- two *tools* whose stems share a directory -- which is the
    /// part this construction cannot make impossible on its own.
    pub fn for_velo_dataset(&self, discriminator: Option<&str>) -> Self {
        Self {
            root: self.root.clone(),
            binary_name: self.binary_name.clone(),
            overwrite: self.overwrite,
            mode: self.mode,
            velo_dataset_dir: discriminator.map(str::to_string),
        }
    }

    /// `PerUser/` for this layout's dataset: the shared directory, plus the
    /// dataset's own discriminator level when it has one.
    fn velo_per_user_dir(&self) -> PathBuf {
        let shared = self.root.join("PerUser");
        match &self.velo_dataset_dir {
            Some(dir) => shared.join(dir),
            None => shared,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn base(&self) -> PathBuf {
        self.root.join(&self.binary_name)
    }

    /// Returns the canonical output path for the given identity and filename.
    ///
    /// `filename` is trusted (dataset constants or explicit user overrides);
    /// this layer does not re-sanitize it.
    pub fn file_path(&self, identity: &Identity, filename: &str) -> PathBuf {
        match self.mode {
            OutputLayoutMode::Nested => self.nested_dir(identity).join(filename),
            OutputLayoutMode::Flat => self.root.join(Self::flat_filename(identity, filename)),
            OutputLayoutMode::Velo => match identity {
                Identity::System => self.root.join(filename),
                Identity::User(_) | Identity::Unknown => self
                    .velo_per_user_dir()
                    .join(Self::velo_filename(identity, filename)),
            },
        }
    }

    /// Filename for a dynamically-written side-car file. In Flat mode the
    /// identity is folded into the name (same rule as primary output) so files
    /// for different identities don't collide at the shared root; in Nested mode
    /// the bare name is kept because the directory already carries the identity.
    pub fn dynamic_filename(&self, identity: &Identity, name: &str) -> String {
        match self.mode {
            OutputLayoutMode::Flat => Self::flat_filename(identity, name),
            OutputLayoutMode::Nested => name.to_string(),
            OutputLayoutMode::Velo => match identity {
                Identity::System => name.to_string(),
                Identity::User(_) | Identity::Unknown => Self::velo_filename(identity, name),
            },
        }
    }

    /// The per-identity directory `Nested` mode writes into.
    fn nested_dir(&self, identity: &Identity) -> PathBuf {
        match identity {
            Identity::User(name) => self.base().join("users").join(name),
            Identity::System => self.base().join("system"),
            Identity::Unknown => self.base().join("users").join("unknown"),
        }
    }

    /// The directory a reference written *into* this tool's output is
    /// relative to: the one holding the primary CSV an analyst opens, which
    /// is also the root of the tree the side-cars live in.
    ///
    /// `Nested` keeps a tool's whole output for one identity in that
    /// identity's own directory, so that directory is the anchor. `Flat`
    /// writes everything straight to the root. `Velo` splits per-user output
    /// into `PerUser/[<discriminator>/]` but publishes the merged file --
    /// and any system-scope output -- at the root, so the root is the anchor
    /// there for both identities. See `side_car_reference` for why a per-user
    /// slice cannot be its own anchor.
    fn reference_root(&self, identity: &Identity) -> PathBuf {
        match self.mode {
            OutputLayoutMode::Flat | OutputLayoutMode::Velo => self.root.clone(),
            OutputLayoutMode::Nested => self.nested_dir(identity),
        }
    }

    /// Where a dynamic side-car named `filename` actually ends up, written as
    /// a `/`-separated path relative to [`Self::reference_root`] -- the form
    /// a cross-reference column (RETriage's `PluginDetailFile`) carries so an
    /// analyst can follow it to the file.
    ///
    /// Derived from `file_path`, the same call that routes the write, so the
    /// reference and the file it names cannot be computed apart. Both halves
    /// of the reference used to be guessed instead, and both were wrong for a
    /// per-user hive: the identity the router folds into the filename was
    /// missing (`TypedURLs_NTUSER.DAT.csv` for a file written as
    /// `TypedURLs_NTUSER.DAT_alice.csv`), and so was the directory the Velo
    /// layout puts per-user output in.
    ///
    /// **The anchor is the category root, not the containing CSV's own
    /// directory, and under `Velo` those differ for a per-user slice.** One
    /// row's text is published twice there -- once into
    /// `PerUser/<Discriminator>/<stem>_<user>.csv` and again into the merged
    /// `<stem>.csv` at the root that `triage_orchestrator::velo::merge`
    /// builds from it (that crate depends on this one, not the reverse) --
    /// and the two sit at different depths, so no single relative path can
    /// resolve against both. The root wins because the merged file is the one
    /// an analyst opens (it is the only batch file carrying `TriageUser`) and
    /// because the same anchor then serves system-scope rows, whose output is
    /// at the root already.
    ///
    /// `/` rather than the platform separator: the value is read by a human
    /// and compared between runs on different platforms, and Windows accepts
    /// it as a path separator too.
    pub fn side_car_reference(&self, identity: &Identity, filename: &str) -> String {
        let destination = self.file_path(identity, filename);
        let anchor = self.reference_root(identity);
        // `anchor` is an ancestor of `destination` in every mode above; the
        // fallback keeps this total rather than relying on that.
        let relative = destination.strip_prefix(&anchor).unwrap_or(&destination);
        relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Builds the flat-mode filename, encoding the identity into it.
    ///
    /// Two cases:
    /// 1. If `filename` starts with a 14-digit run-stamp followed by `_`
    ///    (e.g. `20260624153022_StubTriage_Output.csv`), the identity label is
    ///    prefixed: `<label>_<filename>`.
    /// 2. Otherwise the label is inserted before the extension:
    ///    `<stem>_<label>.<ext>` (or `<stem>_<label>` when there is no
    ///    extension).
    fn flat_filename(identity: &Identity, filename: &str) -> String {
        let label = Self::identity_label(identity);

        if Self::has_run_stamp_prefix(filename) {
            return format!("{label}_{filename}");
        }

        match filename.rsplit_once('.') {
            Some((stem, ext)) => format!("{stem}_{label}.{ext}"),
            None => format!("{filename}_{label}"),
        }
    }

    /// Detects a leading 14-digit `yyyyMMddHHmmss_` run-stamp prefix.
    fn has_run_stamp_prefix(filename: &str) -> bool {
        let bytes = filename.as_bytes();
        let Some(end) = bytes.iter().position(|b| *b == b'_') else {
            return false;
        };
        end >= 14 && bytes[..end].iter().all(u8::is_ascii_digit)
    }

    /// Velo per-user filename: the identity label is always appended before the
    /// extension, never prefixed. A merged file and its per-user files must share
    /// a stem so they sort together and so a `<stem>*.csv` glob finds both.
    fn velo_filename(identity: &Identity, filename: &str) -> String {
        let label = Self::identity_label(identity);
        match filename.rsplit_once('.') {
            Some((stem, ext)) => format!("{stem}_{label}.{ext}"),
            None => format!("{filename}_{label}"),
        }
    }

    fn identity_label(identity: &Identity) -> &str {
        match identity {
            Identity::User(name) => name.as_str(),
            Identity::System => "system",
            Identity::Unknown => "unknown",
        }
    }

    /// Create the output file, erroring on collision unless --overwrite.
    pub fn create(&self, identity: &Identity, filename: &str) -> Result<File, TriageError> {
        let path = self.file_path(identity, filename);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| TriageError::Output {
                path: path.clone(),
                message: e.to_string(),
            })?;
        }

        if self.overwrite {
            File::create(&path).map_err(|e| TriageError::Output {
                path,
                message: e.to_string(),
            })
        } else {
            File::options()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| {
                    if e.kind() == ErrorKind::AlreadyExists {
                        TriageError::Output {
                            path,
                            message: "output file exists; pass --overwrite to replace it".into(),
                        }
                    } else {
                        TriageError::Output {
                            path,
                            message: e.to_string(),
                        }
                    }
                })
        }
    }

    /// Create a sibling temporary file for atomic publication on router finish.
    pub fn create_staged(
        &self,
        identity: &Identity,
        filename: &str,
    ) -> Result<StagedFile, TriageError> {
        let destination = self.file_path(identity, filename);
        if !self.overwrite && destination.exists() {
            return Err(TriageError::Output {
                path: destination,
                message: "output file exists; pass --overwrite to replace it".into(),
            });
        }
        let parent = destination.parent().ok_or_else(|| TriageError::Output {
            path: destination.clone(),
            message: "output path has no parent".into(),
        })?;
        std::fs::create_dir_all(parent).map_err(|e| TriageError::Output {
            path: destination.clone(),
            message: e.to_string(),
        })?;
        for _ in 0..100 {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let name = destination
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("output");
            let temporary = parent.join(format!(".{name}.tmp-{}-{sequence}", std::process::id()));
            match File::options()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => {
                    return Ok(StagedFile {
                        file,
                        temporary,
                        destination,
                    })
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(TriageError::Output {
                        path: temporary,
                        message: e.to_string(),
                    })
                }
            }
        }
        Err(TriageError::Output {
            path: destination,
            message: "could not allocate a unique temporary output file".into(),
        })
    }

    pub fn publish(&self, temporary: &Path, destination: &Path) -> Result<(), TriageError> {
        if !self.overwrite && destination.exists() {
            let _ = std::fs::remove_file(temporary);
            return Err(TriageError::Output {
                path: destination.to_path_buf(),
                message: "output file exists; pass --overwrite to replace it".into(),
            });
        }
        std::fs::rename(temporary, destination).map_err(|e| TriageError::Output {
            path: destination.to_path_buf(),
            message: e.to_string(),
        })
    }
}

/// Names a tool's dynamic side-car files the way a cross-reference column
/// must name them, for one identity and one layout
/// ([`OutputLayout::side_car_reference`]).
///
/// It is a detached snapshot of the routing rules
/// (`OutputRouter::side_car_reference`) rather than a method on the router
/// because the reference is written into a record while the router is
/// already borrowed to write that record: RETriage builds its `PluginDetailFile`
/// column inside the batch engine, whose sink holds `&mut OutputRouter`.
pub struct SideCarReference {
    layout: Option<OutputLayout>,
    identity: Identity,
}

impl SideCarReference {
    /// `layout` is the layout the side-cars are actually written through;
    /// `None` (a router with no output root configured) leaves the filename
    /// as its own reference.
    pub fn new(layout: Option<OutputLayout>, identity: Identity) -> Self {
        Self { layout, identity }
    }

    /// The reference for the side-car written as `filename` (with extension).
    pub fn for_file(&self, filename: &str) -> String {
        match &self.layout {
            Some(layout) => layout.side_car_reference(&self.identity, filename),
            None => filename.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribution::Identity;

    #[test]
    fn paths_follow_spec_section_4_3() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = OutputLayout::new(tmp.path(), "StubTriage", false, OutputLayoutMode::Nested);
        assert_eq!(
            layout.file_path(&Identity::User("alice".into()), "Out.csv"),
            tmp.path().join("StubTriage/users/alice/Out.csv")
        );
        assert_eq!(
            layout.file_path(&Identity::System, "Out.csv"),
            tmp.path().join("StubTriage/system/Out.csv")
        );
        assert_eq!(
            layout.file_path(&Identity::Unknown, "Out.csv"),
            tmp.path().join("StubTriage/users/unknown/Out.csv")
        );
    }

    #[test]
    fn flat_paths_include_identity_in_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = OutputLayout::new(tmp.path(), "StubTriage", false, OutputLayoutMode::Flat);
        assert_eq!(
            layout.file_path(
                &Identity::User("alice".into()),
                "20260624153022_StubTriage_Output.csv"
            ),
            tmp.path()
                .join("alice_20260624153022_StubTriage_Output.csv")
        );
        assert_eq!(
            layout.file_path(&Identity::System, "20260624153022_StubTriage_Output.csv"),
            tmp.path()
                .join("system_20260624153022_StubTriage_Output.csv")
        );
        assert_eq!(
            layout.file_path(&Identity::Unknown, "20260624153022_StubTriage_Output.csv"),
            tmp.path()
                .join("unknown_20260624153022_StubTriage_Output.csv")
        );
    }

    #[test]
    fn nested_paths_keep_existing_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = OutputLayout::new(tmp.path(), "StubTriage", false, OutputLayoutMode::Nested);
        assert_eq!(
            layout.file_path(&Identity::User("alice".into()), "Out.csv"),
            tmp.path().join("StubTriage/users/alice/Out.csv")
        );
        assert_eq!(
            layout.file_path(&Identity::System, "Out.csv"),
            tmp.path().join("StubTriage/system/Out.csv")
        );
        assert_eq!(
            layout.file_path(&Identity::Unknown, "Out.csv"),
            tmp.path().join("StubTriage/users/unknown/Out.csv")
        );
    }

    #[test]
    fn flat_custom_names_insert_identity_before_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = OutputLayout::new(tmp.path(), "StubTriage", false, OutputLayoutMode::Flat);
        assert_eq!(
            layout.file_path(&Identity::User("alice".into()), "custom.csv"),
            tmp.path().join("custom_alice.csv")
        );
        assert_eq!(
            layout.file_path(&Identity::System, "custom"),
            tmp.path().join("custom_system")
        );
    }

    #[test]
    fn flat_inserts_identity_before_last_extension_only() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = OutputLayout::new(tmp.path(), "StubTriage", false, OutputLayoutMode::Flat);
        assert_eq!(
            layout.file_path(&Identity::User("alice".into()), "a.tar.gz"),
            tmp.path().join("a.tar_alice.gz")
        );
    }

    #[test]
    fn dynamic_filename_folds_identity_in_flat_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = OutputLayout::new(tmp.path(), "RETriage", false, OutputLayoutMode::Flat);
        assert_eq!(
            layout.dynamic_filename(&Identity::User("alice".into()), "RecentDocs_NTUSER.DAT.csv"),
            "RecentDocs_NTUSER.DAT_alice.csv"
        );
        assert_eq!(
            layout.dynamic_filename(&Identity::System, "RecentDocs_NTUSER.DAT.csv"),
            "RecentDocs_NTUSER.DAT_system.csv"
        );
    }

    #[test]
    fn dynamic_filename_unchanged_in_nested_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = OutputLayout::new(tmp.path(), "RETriage", false, OutputLayoutMode::Nested);
        assert_eq!(
            layout.dynamic_filename(&Identity::User("alice".into()), "RecentDocs_NTUSER.DAT.csv"),
            "RecentDocs_NTUSER.DAT.csv"
        );
        assert_eq!(
            layout.dynamic_filename(&Identity::System, "RecentDocs_NTUSER.DAT.csv"),
            "RecentDocs_NTUSER.DAT.csv"
        );
    }

    /// A side-car reference names the file that is actually written -- the
    /// directory it lands in as well as the name it lands under. Asserted
    /// against `file_path`'s own answer so a routing change cannot move the
    /// file without moving the reference with it.
    #[test]
    fn side_car_reference_names_the_routed_file_in_every_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let alice = Identity::User("alice".into());
        let requested = "TypedURLs_NTUSER.DAT.csv";
        for (mode, identity, expected) in [
            (
                OutputLayoutMode::Velo,
                &alice,
                "PerUser/TypedURLs_NTUSER.DAT_alice.csv",
            ),
            (
                OutputLayoutMode::Velo,
                &Identity::System,
                "TypedURLs_NTUSER.DAT.csv",
            ),
            (
                OutputLayoutMode::Flat,
                &alice,
                "TypedURLs_NTUSER.DAT_alice.csv",
            ),
            (
                OutputLayoutMode::Flat,
                &Identity::System,
                "TypedURLs_NTUSER.DAT_system.csv",
            ),
            (OutputLayoutMode::Nested, &alice, "TypedURLs_NTUSER.DAT.csv"),
            (
                OutputLayoutMode::Nested,
                &Identity::System,
                "TypedURLs_NTUSER.DAT.csv",
            ),
        ] {
            let layout = OutputLayout::new(tmp.path(), "RETriage", false, mode);
            let reference = layout.side_car_reference(identity, requested);
            assert_eq!(reference, expected, "{mode:?} / {identity:?}");
            assert_eq!(
                layout.reference_root(identity).join(&reference),
                layout.file_path(identity, requested),
                "the reference must resolve to the routed destination ({mode:?})"
            );
        }
    }

    /// Velo is the layout where the reference needs a directory component at
    /// all: per-user side-cars go to `PerUser/` while the merged batch CSV an
    /// analyst follows the reference from sits at the category root.
    #[test]
    fn velo_per_user_reference_carries_the_per_user_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = OutputLayout::new(tmp.path(), "RETriage", false, OutputLayoutMode::Velo);
        let reference =
            layout.side_car_reference(&Identity::User("alice".into()), "TypedURLs_NTUSER.DAT.csv");
        assert_eq!(reference, "PerUser/TypedURLs_NTUSER.DAT_alice.csv");
        // The level the reference above does *not* carry, shown for
        // contrast: a dataset-scoped layout writes its primary output one
        // directory deeper, while side-cars are written -- and referenced --
        // through the base layout. This asserts `for_velo_dataset`'s side of
        // that, not `side_car_reference`'s; what proves the two line up on a
        // real run is
        // `every_plugin_detail_file_reference_resolves_against_the_category_root`
        // (`triage-orchestrator/tests/velo_plugin_detail_file.rs`).
        assert_eq!(
            layout
                .for_velo_dataset(Some("Batch"))
                .file_path(&Identity::User("alice".into()), "x.csv"),
            tmp.path().join("PerUser/Batch/x_alice.csv")
        );
    }

    #[test]
    fn a_side_car_reference_without_a_layout_is_the_filename() {
        let reference = SideCarReference::new(None, Identity::System);
        assert_eq!(reference.for_file("BamDam_SYSTEM.csv"), "BamDam_SYSTEM.csv");
    }

    #[test]
    fn collision_errors_without_overwrite_and_succeeds_with_it() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = OutputLayout::new(tmp.path(), "StubTriage", false, OutputLayoutMode::Nested);
        layout.create(&Identity::System, "Out.csv").unwrap();
        let err = layout.create(&Identity::System, "Out.csv").unwrap_err();
        assert!(err.to_string().contains("Out.csv"));

        let overwriting =
            OutputLayout::new(tmp.path(), "StubTriage", true, OutputLayoutMode::Nested);
        overwriting.create(&Identity::System, "Out.csv").unwrap();
    }

    #[test]
    fn create_new_semantics_without_overwrite() {
        // file created between exists-check and open cannot be truncated:
        // create() with overwrite=false must fail on a pre-existing file
        // even when called twice in a row (create_new is atomic).
        let tmp = tempfile::tempdir().unwrap();
        let layout = OutputLayout::new(tmp.path(), "T", false, OutputLayoutMode::Nested);
        use std::io::Write;
        let mut f = layout.create(&Identity::System, "a.csv").unwrap();
        f.write_all(b"data").unwrap();
        drop(f);
        assert!(layout.create(&Identity::System, "a.csv").is_err());
        // and the original content is untouched
        let content =
            std::fs::read_to_string(layout.file_path(&Identity::System, "a.csv")).unwrap();
        assert_eq!(content, "data");
    }

    #[test]
    fn velo_puts_system_at_the_root_and_users_under_per_user() {
        let root = std::path::Path::new("/out/Processed-WS01-2026-03-13T192553Z/FileSystem");
        let l = OutputLayout::new(root, "LETriage", false, OutputLayoutMode::Velo);

        assert_eq!(
            l.file_path(&Identity::System, "2026-03-13T192553Z_LETriage_results.csv"),
            root.join("2026-03-13T192553Z_LETriage_results.csv")
        );
        assert_eq!(
            l.file_path(
                &Identity::User("jdoe".into()),
                "2026-03-13T192553Z_LETriage_results.csv"
            ),
            root.join("PerUser/2026-03-13T192553Z_LETriage_results_jdoe.csv")
        );
        assert_eq!(
            l.file_path(
                &Identity::Unknown,
                "2026-03-13T192553Z_LETriage_results.csv"
            ),
            root.join("PerUser/2026-03-13T192553Z_LETriage_results_unknown.csv")
        );
    }

    /// Two different datasets of the same tool, two different real profiles,
    /// one filename: `<stem>_<label>` cannot distinguish the bare dataset's
    /// output for a profile named `Downloads_alice` from the `Downloads`
    /// dataset's output for a profile named `alice`. Both halves of that are
    /// legal -- a profile may be named anything a filesystem accepts, and a
    /// tool may have both a bare and a discriminated dataset -- so the two
    /// must land on different paths by construction, not by luck.
    #[test]
    fn a_discriminated_dataset_cannot_collide_with_a_profile_named_after_it() {
        let root = std::path::Path::new("/out/Processed-WS01-S/BrowserActivity");
        let bare = OutputLayout::new(root, "BrowserTriage", false, OutputLayoutMode::Velo)
            .for_velo_dataset(None);
        let downloads = OutputLayout::new(root, "BrowserTriage", false, OutputLayoutMode::Velo)
            .for_velo_dataset(Some("Downloads"));

        let history_of_downloads_alice = bare.file_path(
            &Identity::User("Downloads_alice".into()),
            "S_BrowserTriage_results.csv",
        );
        let downloads_of_alice = downloads.file_path(
            &Identity::User("alice".into()),
            "S_BrowserTriage_results_Downloads.csv",
        );

        assert_eq!(
            history_of_downloads_alice,
            root.join("PerUser/S_BrowserTriage_results_Downloads_alice.csv"),
            "a dataset with no discriminator keeps writing straight into PerUser/"
        );
        assert_eq!(
            downloads_of_alice,
            root.join("PerUser/Downloads/S_BrowserTriage_results_Downloads_alice.csv"),
            "the discriminator is a directory, so the two are different files"
        );
        assert_ne!(history_of_downloads_alice, downloads_of_alice);
    }

    #[test]
    fn velo_suffixes_the_user_even_when_the_name_has_no_extension() {
        let root = std::path::Path::new("/out/X/FileSystem");
        let l = OutputLayout::new(root, "LETriage", false, OutputLayoutMode::Velo);
        assert_eq!(
            l.file_path(&Identity::User("jdoe".into()), "noext"),
            root.join("PerUser/noext_jdoe")
        );
    }

    /// Flat mode prefixes the identity when a run stamp leads the filename;
    /// Velo always suffixes it, because a merged file and its per-user files
    /// must sort together under one stem.
    #[test]
    fn velo_suffixes_where_flat_would_prefix() {
        let root = std::path::Path::new("/out");
        let velo = OutputLayout::new(root, "T", false, OutputLayoutMode::Velo);
        let flat = OutputLayout::new(root, "T", false, OutputLayoutMode::Flat);
        let name = "20260101000000_T_Output.csv";
        assert_eq!(
            velo.file_path(&Identity::User("jdoe".into()), name),
            root.join("PerUser/20260101000000_T_Output_jdoe.csv")
        );
        assert_eq!(
            flat.file_path(&Identity::User("jdoe".into()), name),
            root.join("jdoe_20260101000000_T_Output.csv")
        );
    }
}
