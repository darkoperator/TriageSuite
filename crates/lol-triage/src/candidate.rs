use crate::sniff::SourceKind;

#[derive(Debug, Clone)]
pub struct Candidate {
    pub source_tool: &'static str,
    pub source_dataset: &'static str,
    pub evidence_path: String,
    pub basename: String,
    pub sha1: Option<String>,
    pub evidence_timestamp: String,
    pub source_file: String,
}

fn basename_of(path: &str) -> String {
    let start = path.rfind(['\\', '/']).map_or(0, |i| i + 1);
    path[start..].to_ascii_lowercase()
}

pub fn from_record(
    kind: SourceKind,
    row: &csv::StringRecord,
    source_file: &str,
) -> Option<Candidate> {
    let get = |i: usize| row.get(i).map(str::to_string).unwrap_or_default();
    match kind {
        SourceKind::AmcacheFileEntries => {
            if row.len() != 21 {
                return None;
            }
            let full_path = get(5);
            let sha1 = get(3);
            Some(Candidate {
                source_tool: "AmcacheTriage",
                // AmcacheTriage routes both its Associated and Unassociated
                // file-entry datasets through the identical schema, so a row's
                // content cannot tell us which of the two files it came from.
                source_dataset: "FileEntries",
                basename: basename_of(&full_path),
                sha1: (!sha1.is_empty()).then(|| sha1.to_ascii_lowercase()),
                evidence_timestamp: get(2),
                evidence_path: full_path,
                source_file: source_file.to_string(),
            })
        }
        SourceKind::AmcacheDriveBinaries => {
            if row.len() != 20 {
                return None;
            }
            let driver_name = get(4);
            let driver_id = get(10);
            Some(Candidate {
                source_tool: "AmcacheTriage",
                source_dataset: "DriveBinaries",
                basename: basename_of(&driver_name),
                sha1: (!driver_id.is_empty()).then(|| driver_id.to_ascii_lowercase()),
                evidence_timestamp: get(3),
                evidence_path: driver_name,
                source_file: source_file.to_string(),
            })
        }
        SourceKind::AppCompatCache => {
            if row.len() != 7 {
                return None;
            }
            let path = get(2);
            Some(Candidate {
                source_tool: "AppCompatTriage",
                source_dataset: "AppCompatCache",
                basename: basename_of(&path),
                sha1: None,
                evidence_timestamp: get(3),
                evidence_path: path,
                source_file: source_file.to_string(),
            })
        }
        SourceKind::Mft => {
            if row.len() != 34 {
                return None;
            }
            let file_name = get(6);
            Some(Candidate {
                source_tool: "MFTriage",
                source_dataset: "$MFT",
                basename: basename_of(&file_name),
                sha1: None,
                evidence_timestamp: get(19), // Created0x10
                evidence_path: file_name,
                source_file: source_file.to_string(),
            })
        }
        SourceKind::Prefetch => {
            if row.len() != 27 {
                return None;
            }
            let exe = get(5);
            Some(Candidate {
                source_tool: "PETriage",
                source_dataset: "Prefetch",
                basename: basename_of(&exe),
                sha1: None,
                evidence_timestamp: get(10), // LastRun
                evidence_path: exe,
                source_file: source_file.to_string(),
            })
        }
        SourceKind::RecycleBin => {
            if row.len() != 5 {
                return None;
            }
            let file_name = get(2);
            Some(Candidate {
                source_tool: "RBTriage",
                source_dataset: "RecycleBin",
                basename: basename_of(&file_name),
                sha1: None,
                evidence_timestamp: get(4), // DeletedOn
                evidence_path: file_name,
                source_file: source_file.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use csv::StringRecord;

    fn record(fields: &[&str]) -> StringRecord {
        StringRecord::from(fields.to_vec())
    }

    #[test]
    fn amcache_file_entries_extracts_hash_and_basename() {
        let row = record(&[
            "Unassociated",
            "prog-1",
            "2024-01-01T00:00:00.0000000Z",
            "B9C3F4DCC7463CBEC84B808D880194BBC304CCD0",
            "False",
            r"C:\Windows\System32\drivers\nvflash.sys",
            "nvflash.sys",
            ".sys",
            "",
            "",
            "1024",
            "",
            "",
            "",
            "",
            "False",
            "",
            "",
            "0",
            "",
            "",
        ]);
        let c = from_record(SourceKind::AmcacheFileEntries, &row, "amcache.csv").unwrap();
        assert_eq!(c.source_tool, "AmcacheTriage");
        assert_eq!(c.source_dataset, "FileEntries");
        assert_eq!(c.basename, "nvflash.sys");
        assert_eq!(
            c.sha1.as_deref(),
            Some("b9c3f4dcc7463cbec84b808d880194bbc304ccd0")
        );
        assert_eq!(c.evidence_timestamp, "2024-01-01T00:00:00.0000000Z");
    }

    #[test]
    fn appcompat_cache_has_no_hash() {
        let row = record(&[
            "1",
            "0",
            r"C:\Windows\System32\kitty.exe",
            "2024-01-01T00:00:00.0000000Z",
            "Yes",
            "False",
            r"C:\triage\SYSTEM",
        ]);
        let c = from_record(SourceKind::AppCompatCache, &row, "appcompat.csv").unwrap();
        assert_eq!(c.source_tool, "AppCompatTriage");
        assert_eq!(c.basename, "kitty.exe");
        assert_eq!(c.sha1, None);
    }

    #[test]
    fn short_row_returns_none() {
        let row = record(&["only", "two"]);
        assert!(from_record(SourceKind::AppCompatCache, &row, "x.csv").is_none());
    }

    /// DriveBinaries: 20 columns; DriverName is col 4, DriverLastWriteTime col
    /// 3, DriverId col 10 (used as the SHA-1-shaped hash field).
    #[test]
    fn amcache_drive_binaries_extracts_name_id_and_timestamp() {
        let row = record(&[
            "Root\\InventoryDriverBinary\\nvflash.sys",
            "2024-02-02T00:00:00.0000000Z",
            "2023-01-01T00:00:00.0000000Z",
            "2024-03-03T00:00:00.0000000Z",
            r"C:\Windows\System32\drivers\NVFLASH.SYS",
            "False",
            "True",
            "True",
            "0x12345",
            "NVIDIA",
            "B9C3F4DCC7463CBEC84B808D880194BBC304CCD0",
            "strongname",
            "kernel",
            "1.0.0.0",
            "4096",
            "oem1.inf",
            "NVIDIA Driver",
            "1.0",
            "nvflash",
            "1.11",
        ]);
        assert_eq!(row.len(), 20);
        let c = from_record(SourceKind::AmcacheDriveBinaries, &row, "drivebinaries.csv").unwrap();
        assert_eq!(c.source_tool, "AmcacheTriage");
        assert_eq!(c.source_dataset, "DriveBinaries");
        assert_eq!(c.basename, "nvflash.sys");
        assert_eq!(
            c.sha1.as_deref(),
            Some("b9c3f4dcc7463cbec84b808d880194bbc304ccd0")
        );
        assert_eq!(c.evidence_timestamp, "2024-03-03T00:00:00.0000000Z");
        assert_eq!(c.evidence_path, r"C:\Windows\System32\drivers\NVFLASH.SYS");
        assert_eq!(c.source_file, "drivebinaries.csv");
    }

    /// $MFT: 34 columns; FileName is col 6, Created0x10 is col 19. No hash.
    #[test]
    fn mft_extracts_filename_and_created_timestamp() {
        let mut fields = vec![""; 34];
        fields[5] = r".\Windows\System32\drivers";
        fields[6] = "NvFlash.sys";
        fields[7] = ".sys";
        fields[19] = "2024-04-04T00:00:00.0000000Z";
        fields[20] = "2024-05-05T00:00:00.0000000Z"; // Created0x30 must be ignored
        fields[33] = r"C:\triage\$MFT";
        let row = record(&fields);
        assert_eq!(row.len(), 34);
        let c = from_record(SourceKind::Mft, &row, "mft.csv").unwrap();
        assert_eq!(c.source_tool, "MFTriage");
        assert_eq!(c.source_dataset, "$MFT");
        assert_eq!(c.basename, "nvflash.sys");
        assert_eq!(c.sha1, None);
        assert_eq!(c.evidence_timestamp, "2024-04-04T00:00:00.0000000Z");
        assert_eq!(c.evidence_path, "NvFlash.sys");
    }

    /// Prefetch: 27 columns; ExecutableName is col 5, LastRun col 10. No hash.
    #[test]
    fn prefetch_extracts_executable_and_last_run() {
        let mut fields = vec![""; 27];
        fields[1] = r"C:\Windows\Prefetch\KITTY.EXE-1A2B3C4D.pf";
        fields[5] = "KITTY.EXE";
        fields[6] = "1A2B3C4D"; // Prefetch "Hash" is a path hash, never a file hash
        fields[9] = "3";
        fields[10] = "2024-06-06T00:00:00.0000000Z";
        fields[11] = "2024-05-05T00:00:00.0000000Z"; // PreviousRun0 must be ignored
        let row = record(&fields);
        assert_eq!(row.len(), 27);
        let c = from_record(SourceKind::Prefetch, &row, "prefetch.csv").unwrap();
        assert_eq!(c.source_tool, "PETriage");
        assert_eq!(c.source_dataset, "Prefetch");
        assert_eq!(c.basename, "kitty.exe");
        assert_eq!(c.sha1, None);
        assert_eq!(c.evidence_timestamp, "2024-06-06T00:00:00.0000000Z");
        assert_eq!(c.evidence_path, "KITTY.EXE");
    }

    /// Recycle Bin: 5 columns; FileName is col 2, DeletedOn col 4. No hash.
    #[test]
    fn recycle_bin_extracts_filename_and_deleted_on() {
        let row = record(&[
            r"C:\$Recycle.Bin\S-1-5-21-1\$I8Q2K3L.exe",
            "$I",
            r"C:\Users\alice\Downloads\KiTTY.exe",
            "1024",
            "2024-07-07T00:00:00.0000000Z",
        ]);
        assert_eq!(row.len(), 5);
        let c = from_record(SourceKind::RecycleBin, &row, "recyclebin.csv").unwrap();
        assert_eq!(c.source_tool, "RBTriage");
        assert_eq!(c.source_dataset, "RecycleBin");
        assert_eq!(c.basename, "kitty.exe");
        assert_eq!(c.sha1, None);
        assert_eq!(c.evidence_timestamp, "2024-07-07T00:00:00.0000000Z");
        assert_eq!(c.evidence_path, r"C:\Users\alice\Downloads\KiTTY.exe");
    }

    /// Every kind rejects a row whose column count does not match its schema.
    #[test]
    fn wrong_column_count_returns_none_for_every_kind() {
        for kind in [
            SourceKind::AmcacheFileEntries,
            SourceKind::AmcacheDriveBinaries,
            SourceKind::AppCompatCache,
            SourceKind::Mft,
            SourceKind::Prefetch,
            SourceKind::RecycleBin,
        ] {
            let row = record(&["a", "b", "c"]);
            assert!(
                from_record(kind, &row, "x.csv").is_none(),
                "{kind:?} accepted a 3-column row"
            );
        }
    }

    #[test]
    fn basename_of_handles_bare_names_and_both_separators() {
        assert_eq!(basename_of("NVFLASH.SYS"), "nvflash.sys");
        assert_eq!(basename_of(r"C:\Windows\nvflash.sys"), "nvflash.sys");
        assert_eq!(basename_of("/usr/local/KiTTY.exe"), "kitty.exe");
        assert_eq!(basename_of(""), "");
        assert_eq!(basename_of(r"C:\Windows\"), "");
    }
}
