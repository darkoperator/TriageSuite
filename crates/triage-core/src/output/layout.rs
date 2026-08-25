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
/// under `<root>` with the identity encoded in the filename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputLayoutMode {
    Flat,
    Nested,
}

/// Builds and creates output paths per spec section 4.3:
/// `<root>/<BinaryName>/users/<name>/...`, `<root>/<BinaryName>/system/...`,
/// `<root>/<BinaryName>/users/unknown/...`.
pub struct OutputLayout {
    root: PathBuf,
    binary_name: String,
    overwrite: bool,
    mode: OutputLayoutMode,
}

impl OutputLayout {
    pub fn new(root: &Path, binary_name: &str, overwrite: bool, mode: OutputLayoutMode) -> Self {
        Self {
            root: root.to_path_buf(),
            binary_name: binary_name.to_string(),
            overwrite,
            mode,
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
            OutputLayoutMode::Nested => {
                let dir = match identity {
                    Identity::User(name) => self.base().join("users").join(name),
                    Identity::System => self.base().join("system"),
                    Identity::Unknown => self.base().join("users").join("unknown"),
                };
                dir.join(filename)
            }
            OutputLayoutMode::Flat => self.root.join(Self::flat_filename(identity, filename)),
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
        }
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
}
