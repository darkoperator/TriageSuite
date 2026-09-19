//! MFTriage: MFTECmd-compatible NTFS $MFT / $J / $Boot parser.

pub mod cli;

use std::path::{Path, PathBuf};

use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::duckdb::types::{ColumnType, DatasetColumnTypes, SqlType, TimeSemantics};
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

/// Declared SQL types for the DuckDB view layer.
///
/// The timestamp columns are `String` on the record structs, not
/// `WinTimestamp` -- but unlike PETriage's pre-rendered PECmd-parity
/// strings, every one of them is produced by a single call to
/// `triage_core::timestamp::filetime_to_iso8601` (via `format_filetime`,
/// `format_filetime_or_empty` and `format_0x30_timestamp`), whose only two
/// outcomes are a canonical ISO-8601 instant or the empty string. So the
/// shape is proven by that one core function rather than by the field type,
/// and an empty cell is a missing value -- NULL in both the typed column and
/// its `__text` companion -- not a conversion failure.
///
/// `EntryNumber`, `FileSize` and the other `u64` fields are UBIGINT: a
/// $MFT reference count or file size genuinely uses the top bit, and BIGINT
/// would turn those into negative numbers.
///
/// Left undeclared, all free text: `ParentPath`, `FileName`, `Extension`,
/// `ReparseTarget`, `SiFlags`, `NameType` (flag names joined with `|`),
/// `ObjectIdFileDroid`, `LoggedUtilStream`, `ZoneIdContents`, `SourceFile`,
/// the `$J` `UpdateReasons`/`FileAttributes` flag strings, and every `$Boot`
/// field that is a hex spelling (`EntryPoint`, `Signature`,
/// `VolumeSerialNumber*`, `SectorSignature`) rather than a number.
pub const COLUMN_TYPES: &[DatasetColumnTypes] = &[
    DatasetColumnTypes {
        dataset_id: "mft",
        columns: &[
            ColumnType {
                column: "EntryNumber",
                sql_type: SqlType::UBigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "SequenceNumber",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "InUse",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "ParentEntryNumber",
                sql_type: SqlType::UBigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ParentSequenceNumber",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "FileSize",
                sql_type: SqlType::UBigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ReferenceCount",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "IsDirectory",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "HasAds",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "IsAds",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "SI<FN",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "uSecZeros",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "Copied",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "Created0x10",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "Created0x30",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "LastModified0x10",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "LastModified0x30",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "LastRecordChange0x10",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "LastRecordChange0x30",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "LastAccess0x10",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "LastAccess0x30",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "UpdateSequenceNumber",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "LogfileSequenceNumber",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "SecurityId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "mft_file_listing",
        columns: &[
            ColumnType {
                column: "IsDirectory",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "FileSize",
                sql_type: SqlType::UBigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "Created0x10",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "LastModified0x10",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "usn",
        columns: &[
            ColumnType {
                column: "EntryNumber",
                sql_type: SqlType::UBigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "SequenceNumber",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ParentEntryNumber",
                sql_type: SqlType::UBigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ParentSequenceNumber",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "UpdateSequenceNumber",
                sql_type: SqlType::UBigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "UpdateTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "OffsetToData",
                sql_type: SqlType::UBigInt,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "boot",
        columns: &[
            ColumnType {
                column: "BytesPerSector",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "SectorsPerCluster",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ClusterSize",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ReservedSectors",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "TotalSectors",
                sql_type: SqlType::UBigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "MftClusterBlockNumber",
                sql_type: SqlType::UBigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "MftMirrClusterBlockNumber",
                sql_type: SqlType::UBigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "MftEntrySize",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "IndexEntrySize",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
        ],
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

    fn column_types(&self) -> &'static [DatasetColumnTypes] {
        COLUMN_TYPES
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
