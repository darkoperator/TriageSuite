//! PETriage: Windows Prefetch parser producing PECmd-compatible output.
//!
//! Field semantics are ported from Eric Zimmerman's PECmd
//! (PECmd-master/PECmd/Program.cs, `GetCsvFormat` and the CSV/timeline
//! output loop) and his Prefetch library (Prefetch-master/Prefetch).

use serde::Serialize;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_core::tool::{Scope, Tool};
use triage_prefetch::format::{parse_file_bytes, PrefetchFile};

/// One row of the main output. Column names and order match PECmd's CsvOut
/// (verified against the PECmd fixture header).
///
/// Field TYPES mirror PECmd's null-vs-empty-string split because the JSON
/// writer omits null properties (like PECmd's ServiceStack `ToJson()`) but
/// keeps empty strings: `Option<_>` here = nullable in CsvOut (omitted from
/// JSON when unset), `String`/`WinTimestamp`-rendered-empty = C# string.Empty
/// (kept as `""`). Both serialize to an empty CSV cell.
#[derive(Serialize)]
pub struct PrefetchRecord {
    /// None unless the pf file references more than two volumes
    /// (Program.cs GetCsvFormat leaves CsvOut.Note null otherwise, so the
    /// property is absent from PECmd's JSON).
    #[serde(rename = "Note")]
    pub note: Option<String>,
    #[serde(rename = "SourceFilename")]
    pub source_filename: String,
    /// Pre-rendered string (PECmd's CsvOut stores formatted strings, never
    /// null): present in JSON even when empty.
    #[serde(rename = "SourceCreated")]
    pub source_created: String,
    #[serde(rename = "SourceModified")]
    pub source_modified: String,
    #[serde(rename = "SourceAccessed")]
    pub source_accessed: String,
    #[serde(rename = "ExecutableName")]
    pub executable_name: String,
    /// Header hash as uppercase hex without zero-padding
    /// (Header.cs: `ToString("X")`).
    #[serde(rename = "Hash")]
    pub hash: String,
    /// Decompressed pf size from the header field at 0x0C, not the on-disk
    /// MAM size. String to match PECmd's CsvOut (`FileSize.ToString()`).
    #[serde(rename = "Size")]
    pub size: String,
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "RunCount")]
    pub run_count: String,
    /// "" when the file records no run times (PECmd initializes lrTime to
    /// string.Empty), so the JSON property is always present.
    #[serde(rename = "LastRun")]
    pub last_run: String,
    /// None (omitted from JSON) when the file records fewer runs, exactly
    /// like PECmd's nullable PreviousRunN strings.
    #[serde(rename = "PreviousRun0")]
    pub previous_run0: WinTimestamp,
    #[serde(rename = "PreviousRun1")]
    pub previous_run1: WinTimestamp,
    #[serde(rename = "PreviousRun2")]
    pub previous_run2: WinTimestamp,
    #[serde(rename = "PreviousRun3")]
    pub previous_run3: WinTimestamp,
    #[serde(rename = "PreviousRun4")]
    pub previous_run4: WinTimestamp,
    #[serde(rename = "PreviousRun5")]
    pub previous_run5: WinTimestamp,
    #[serde(rename = "PreviousRun6")]
    pub previous_run6: WinTimestamp,
    /// "" when the file lists no volumes (PECmd initializes these to
    /// string.Empty, so the JSON properties are always present).
    #[serde(rename = "Volume0Name")]
    pub volume0_name: String,
    /// Uppercase hex without zero-padding (Version30or31.cs `ToString("X")`).
    #[serde(rename = "Volume0Serial")]
    pub volume0_serial: String,
    /// Pre-rendered string: "" when there are no volumes OR when the year
    /// is 1601 (PECmd blanks it to string.Empty, which its JSON keeps).
    #[serde(rename = "Volume0Created")]
    pub volume0_created: String,
    /// None when the file lists fewer than two volumes (nullable in CsvOut,
    /// omitted from PECmd's JSON).
    #[serde(rename = "Volume1Name")]
    pub volume1_name: Option<String>,
    #[serde(rename = "Volume1Serial")]
    pub volume1_serial: Option<String>,
    #[serde(rename = "Volume1Created")]
    pub volume1_created: Option<String>,
    #[serde(rename = "Directories")]
    pub directories: String,
    #[serde(rename = "FilesLoaded")]
    pub files_loaded: String,
    /// "False"/"True" to match PECmd's C# bool rendering in CSV. Always
    /// "False" here: parse failures abort the record instead of emitting
    /// a partial one.
    #[serde(rename = "ParsingError")]
    pub parsing_error: String,
}

/// One row of the timeline output (CSV only, like PECmd's CsvOutTl).
#[derive(Serialize)]
pub struct TimelineRecord {
    #[serde(rename = "RunTime")]
    pub run_time: WinTimestamp,
    /// Volume-qualified path of the executable: the files-loaded entry that
    /// ends with the header executable name, falling back to the bare name
    /// when no entry matches (Program.cs timeline loop).
    #[serde(rename = "ExecutableName")]
    pub executable_name: String,
}

pub const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        id: "main",
        default_basename: "PETriage_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    },
    DatasetSpec {
        id: "timeline",
        default_basename: "PETriage_Output_Timeline",
        framing: JsonFraming::Ndjson, // unused; csv_only
        csv_only: true,
        override_suffix: Some("_Timeline"),
    },
];

/// Per-version OS description, matching the Description attributes on the
/// Prefetch library's Version enum (IPrefetch.cs).
fn version_description(version: u32) -> String {
    match version {
        17 => "Windows XP or Windows Server 2003".into(),
        23 => "Windows Vista or Windows 7".into(),
        26 => "Windows 8.0, Windows 8.1, or Windows Server 2012(R2)".into(),
        30 => "Windows 10 or Windows 11".into(),
        31 => "Windows 11".into(),
        other => other.to_string(),
    }
}

/// Full-precision WinTimestamp from a filesystem SystemTime.
fn from_system_time(t: SystemTime) -> WinTimestamp {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => WinTimestamp::from_unix_nanos(d.as_secs() as i64, d.subsec_nanos()),
        // Pre-1970 mtimes are legal on disk; carry the negative offset.
        Err(e) => {
            let d = e.duration();
            if d.subsec_nanos() == 0 {
                WinTimestamp::from_unix_nanos(-(d.as_secs() as i64), 0)
            } else {
                WinTimestamp::from_unix_nanos(
                    -(d.as_secs() as i64) - 1,
                    1_000_000_000 - d.subsec_nanos(),
                )
            }
        }
    }
}

/// Volume0Created with PECmd's year-1601 blanking (GetCsvFormat blanks the
/// volume 0 creation time when its year is 1601; volume 1 is NOT blanked).
fn volume0_created(filetime: u64) -> WinTimestamp {
    // 1601 is not a leap year: anything below 365 days of ticks is in 1601.
    const TICKS_END_OF_1601: u64 = 365 * 86_400 * 10_000_000;
    if filetime < TICKS_END_OF_1601 {
        WinTimestamp::none()
    } else {
        WinTimestamp::from_filetime(filetime)
    }
}

fn run_time(pf: &PrefetchFile, idx: usize) -> WinTimestamp {
    pf.run_times
        .get(idx)
        .map(|&ft| WinTimestamp::from_filetime(ft))
        .unwrap_or_else(WinTimestamp::none)
}

fn build_record(pf: &PrefetchFile, path: &Path) -> PrefetchRecord {
    let meta = std::fs::metadata(path);
    // created() is unavailable on some filesystems; unset rather than invented.
    let (created, modified, accessed) = match &meta {
        Ok(m) => (
            m.created()
                .map(from_system_time)
                .unwrap_or_else(|_| WinTimestamp::none()),
            m.modified()
                .map(from_system_time)
                .unwrap_or_else(|_| WinTimestamp::none()),
            m.accessed()
                .map(from_system_time)
                .unwrap_or_else(|_| WinTimestamp::none()),
        ),
        Err(_) => (
            WinTimestamp::none(),
            WinTimestamp::none(),
            WinTimestamp::none(),
        ),
    };

    let (v0_name, v0_serial, v0_created) = match pf.volumes.first() {
        Some(v) => (
            v.device_name.clone(),
            format!("{:X}", v.serial_number),
            volume0_created(v.creation_time).to_string(),
        ),
        None => (String::new(), String::new(), String::new()),
    };
    // Note: PECmd does not apply the 1601 blanking to Volume1Created.
    let (v1_name, v1_serial, v1_created) = match pf.volumes.get(1) {
        Some(v) => (
            Some(v.device_name.clone()),
            Some(format!("{:X}", v.serial_number)),
            Some(WinTimestamp::from_filetime(v.creation_time).to_string()),
        ),
        None => (None, None, None),
    };

    let note = (pf.volumes.len() > 2).then(|| {
        "File contains > 2 volumes! Please inspect output from main program for full details!"
            .to_string()
    });

    // PECmd quirk replicated exactly: each volume's directories are joined
    // with ", ", but consecutive volumes' lists are appended with NO
    // separator between them (GetCsvFormat's sbDirs.Append loop).
    let directories: String = pf
        .volumes
        .iter()
        .map(|v| v.directories.join(", "))
        .collect::<Vec<_>>()
        .concat();

    PrefetchRecord {
        note,
        source_filename: path.display().to_string(),
        source_created: created.to_string(),
        source_modified: modified.to_string(),
        source_accessed: accessed.to_string(),
        executable_name: pf.executable_name.clone(),
        hash: format!("{:X}", pf.hash),
        size: pf.file_size.to_string(),
        version: version_description(pf.version),
        run_count: pf.run_count.to_string(),
        last_run: run_time(pf, 0).to_string(),
        previous_run0: run_time(pf, 1),
        previous_run1: run_time(pf, 2),
        previous_run2: run_time(pf, 3),
        previous_run3: run_time(pf, 4),
        previous_run4: run_time(pf, 5),
        previous_run5: run_time(pf, 6),
        previous_run6: run_time(pf, 7),
        volume0_name: v0_name,
        volume0_serial: v0_serial,
        volume0_created: v0_created,
        volume1_name: v1_name,
        volume1_serial: v1_serial,
        volume1_created: v1_created,
        directories,
        files_loaded: pf.files_loaded.join(", "),
        parsing_error: "False".to_string(),
    }
}

/// Timeline executable path: the files-loaded entry ending with the
/// executable name, else the bare executable name (Program.cs).
fn timeline_exe_path(pf: &PrefetchFile) -> String {
    pf.files_loaded
        .iter()
        .find(|f| f.ends_with(&pf.executable_name))
        .cloned()
        .unwrap_or_else(|| pf.executable_name.clone())
}

pub struct PeTool {
    /// Lowercased keywords to flag (PECmd -k semantics; temp/tmp always
    /// included). Hits are surfaced via tracing::warn!; output files are
    /// unaffected, matching PECmd's console-only highlighting.
    pub keywords: Vec<String>,
}

impl Default for PeTool {
    fn default() -> Self {
        PeTool {
            keywords: vec!["temp".into(), "tmp".into()],
        }
    }
}

impl Tool for PeTool {
    fn binary_name(&self) -> &'static str {
        "PETriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &["*.pf"]
    }

    fn validate_legacy(&self, path: &Path) -> bool {
        use std::io::Read;
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        let mut magic = [0u8; 8];
        if f.read_exact(&mut magic).is_err() {
            return false;
        }
        &magic[..4] == b"MAM\x04" || &magic[4..8] == b"SCCA"
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

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        let raw = std::fs::read(path).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let pf = parse_file_bytes(&raw).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let hits = keyword_hits(&pf, &self.keywords);
        if !hits.is_empty() {
            tracing::warn!("keyword hit in {}: {}", path.display(), hits.join(", "));
        }

        out.write("main", &build_record(&pf, path))?;

        let exe_path = timeline_exe_path(&pf);
        for &ft in &pf.run_times {
            out.write(
                "timeline",
                &TimelineRecord {
                    run_time: WinTimestamp::from_filetime(ft),
                    executable_name: exe_path.clone(),
                },
            )?;
        }
        Ok(1 + pf.run_times.len() as u64)
    }
}

/// Keywords matched anywhere in the loaded-file or directory strings
/// (case-insensitive; keywords are pre-lowercased by main).
fn keyword_hits(pf: &PrefetchFile, keywords: &[String]) -> Vec<String> {
    keywords
        .iter()
        .filter(|k| {
            pf.files_loaded
                .iter()
                .any(|f| f.to_lowercase().contains(k.as_str()))
                || pf
                    .volumes
                    .iter()
                    .flat_map(|v| &v.directories)
                    .any(|d| d.to_lowercase().contains(k.as_str()))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_descriptions_match_prefetch_library_enum() {
        assert_eq!(version_description(17), "Windows XP or Windows Server 2003");
        assert_eq!(version_description(23), "Windows Vista or Windows 7");
        assert_eq!(
            version_description(26),
            "Windows 8.0, Windows 8.1, or Windows Server 2012(R2)"
        );
        assert_eq!(version_description(30), "Windows 10 or Windows 11");
        assert_eq!(version_description(31), "Windows 11");
    }

    #[test]
    fn hash_and_serial_format_is_unpadded_uppercase_hex() {
        // C# ToString("X"): 0x02C9109F9 stays unpadded
        assert_eq!(format!("{:X}", 0x2C9109F9u32), "2C9109F9");
        assert_eq!(format!("{:X}", 0x0000ABCu32), "ABC");
    }

    #[test]
    fn volume0_created_blanks_year_1601() {
        // 1601-06-01 ~= 151 days of ticks: blanked
        assert!(volume0_created(151 * 86_400 * 10_000_000).is_none());
        // 1602-01-02: kept
        let kept = volume0_created(366 * 86_400 * 10_000_000);
        assert!(kept.to_string().starts_with("1602-"));
    }

    #[test]
    fn main_csv_header_matches_pecmd_fixture() {
        let mut buf = Vec::new();
        {
            let mut w = triage_core::output::dataset::CsvSink::new(&mut buf);
            w.write(&empty_record()).unwrap();
            w.finish().unwrap();
        }
        let text = String::from_utf8(buf).unwrap();
        let header = text.lines().next().unwrap();
        assert_eq!(
            header,
            "Note,SourceFilename,SourceCreated,SourceModified,SourceAccessed,\
             ExecutableName,Hash,Size,Version,RunCount,LastRun,PreviousRun0,\
             PreviousRun1,PreviousRun2,PreviousRun3,PreviousRun4,PreviousRun5,\
             PreviousRun6,Volume0Name,Volume0Serial,Volume0Created,Volume1Name,\
             Volume1Serial,Volume1Created,Directories,FilesLoaded,ParsingError"
        );
    }

    #[test]
    fn timeline_csv_header_matches_pecmd() {
        let mut buf = Vec::new();
        {
            let mut w = triage_core::output::dataset::CsvSink::new(&mut buf);
            w.write(&TimelineRecord {
                run_time: WinTimestamp::none(),
                executable_name: String::new(),
            })
            .unwrap();
            w.finish().unwrap();
        }
        let text = String::from_utf8(buf).unwrap();
        assert_eq!(text.lines().next().unwrap(), "RunTime,ExecutableName");
    }

    fn empty_record() -> PrefetchRecord {
        PrefetchRecord {
            note: None,
            source_filename: String::new(),
            source_created: String::new(),
            source_modified: String::new(),
            source_accessed: String::new(),
            executable_name: String::new(),
            hash: String::new(),
            size: String::new(),
            version: String::new(),
            run_count: String::new(),
            last_run: String::new(),
            previous_run0: WinTimestamp::none(),
            previous_run1: WinTimestamp::none(),
            previous_run2: WinTimestamp::none(),
            previous_run3: WinTimestamp::none(),
            previous_run4: WinTimestamp::none(),
            previous_run5: WinTimestamp::none(),
            previous_run6: WinTimestamp::none(),
            volume0_name: String::new(),
            volume0_serial: String::new(),
            volume0_created: String::new(),
            volume1_name: None,
            volume1_serial: None,
            volume1_created: None,
            directories: String::new(),
            files_loaded: String::new(),
            parsing_error: String::new(),
        }
    }
}
