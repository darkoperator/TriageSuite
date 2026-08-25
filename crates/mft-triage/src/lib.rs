//! MFTriage: MFTECmd-compatible NTFS $MFT / $J / $Boot parser.

pub mod cli;

use std::path::{Path, PathBuf};

use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};

pub const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        id: "mft",
        default_basename: "MFTriage_$MFT_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    },
    DatasetSpec {
        id: "mft_file_listing",
        default_basename: "MFTriage_$MFT_Output_FileListing",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_FileListing"),
    },
    DatasetSpec {
        id: "usn",
        default_basename: "MFTriage_$J_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_J"),
    },
    DatasetSpec {
        id: "boot",
        default_basename: "MFTriage_$Boot_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_Boot"),
    },
];

#[derive(Default)]
pub struct MftTool {
    pub sn: bool,
    pub at: bool,
    pub fl: bool,
    pub mft: Option<PathBuf>,
}

impl MftTool {
    /// Build an $MFT path index for $J parent-path resolution: prefer the explicit
    /// `--mft`, else the sibling `$MFT` (the $J file's parent's parent, joined with
    /// "$MFT" — i.e. the NTFS root). Returns None if no $MFT is found/parseable.
    fn resolve_mft_index(&self, j_path: &Path) -> Option<triage_mft::mft::MftPathIndex> {
        let mft_path = self.mft.clone().or_else(|| {
            // $J lives at <root>/$Extend/$UsnJrnl%3A$J; $MFT at <root>/$MFT.
            let candidates = [
                j_path
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|r| r.join("$MFT")),
                j_path.parent().map(|p| p.join("$MFT")),
            ];
            candidates.into_iter().flatten().find(|p| p.is_file())
        })?;
        triage_mft::mft::visit_mft(
            &mft_path,
            triage_mft::mft::MftParseOptions::default(),
            |_| Ok::<(), std::convert::Infallible>(()),
        )
        .ok()
    }
}

impl Tool for MftTool {
    fn binary_name(&self) -> &'static str {
        "MFTriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &["$MFT", "$Boot", "$UsnJrnl%3A$J", "$UsnJrnl:$J", "$J"]
    }

    fn validate_legacy(&self, path: &Path) -> bool {
        use triage_mft::detect::{detect_by_path, ArtifactType};
        let mut buf = [0u8; 8];
        let read = {
            use std::io::Read;
            match std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)) {
                Ok(n) => n,
                Err(_) => return false,
            }
        };
        match detect_by_path(path) {
            // $MFT FILE records start with "FILE".
            ArtifactType::Mft => read >= 4 && &buf[0..4] == b"FILE",
            // $Boot OEM id "NTFS" sits at offset 3.
            ArtifactType::Boot => read >= 7 && &buf[3..7] == b"NTFS",
            // $UsnJrnl:$J is a sparse file with no fixed header magic; accept on the
            // name match (the only signal available — documented exception to the
            // content-validation rule).
            ArtifactType::UsnJournal => true,
            _ => false,
        }
    }

    fn invalid_content_is_corrupt(&self) -> bool {
        true
    }

    fn datasets(&self) -> &'static [DatasetSpec] {
        DATASETS
    }

    fn scope(&self) -> Scope {
        Scope::SystemWide
    }

    fn resource_class(&self) -> triage_core::tool::ResourceClass {
        triage_core::tool::ResourceClass::Heavy
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        use triage_mft::detect::{detect_by_path, ArtifactType};
        let to_err = |e: triage_mft::error::MftriageError| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        };

        let mut count = 0u64;
        match detect_by_path(path) {
            ArtifactType::Mft => {
                let opts = triage_mft::mft::MftParseOptions {
                    include_dos_names: self.sn,
                    include_all_file_name_timestamps: self.at,
                };
                match triage_mft::mft::visit_mft(path, opts, |rec| {
                    out.write("mft", &rec)?;
                    count += 1;
                    if self.fl && !rec.is_ads {
                        let listing = triage_mft::mft::FileListingRecord::from(&rec);
                        out.write("mft_file_listing", &listing)?;
                        count += 1;
                    }
                    Ok::<(), TriageError>(())
                }) {
                    Ok(_) => {}
                    Err(triage_mft::mft::MftVisitError::Parse(error)) => return Err(to_err(error)),
                    Err(triage_mft::mft::MftVisitError::Visitor(error)) => return Err(error),
                }
            }
            ArtifactType::Boot => {
                let rec = triage_mft::boot::parse_boot(path).map_err(to_err)?;
                out.write("boot", &rec)?;
                count += 1;
            }
            ArtifactType::UsnJournal => {
                let index = self.resolve_mft_index(path);
                match triage_mft::usn::visit_usn_journal(path, |mut rec| {
                    if let Some(index) = &index {
                        if let Some(parent) =
                            index.full_path(rec.parent_entry_number, rec.parent_sequence_number)
                        {
                            rec.parent_path = parent;
                        }
                    }
                    out.write("usn", &rec)?;
                    count += 1;
                    Ok::<(), TriageError>(())
                }) {
                    Ok(()) => {}
                    Err(triage_mft::usn::UsnVisitError::Parse(error)) => return Err(to_err(error)),
                    Err(triage_mft::usn::UsnVisitError::Visitor(error)) => return Err(error),
                }
            }
            _ => {} // Sds / LogFile / Unknown: not handled by this milestone
        }
        Ok(count)
    }
}
