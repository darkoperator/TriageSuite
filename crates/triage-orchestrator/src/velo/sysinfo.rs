//! `CaseInfo/<stamp>_SysInfo.txt`: host context, read back out of RETriage's
//! own output rather than re-parsing the hives.
//!
//! RETriage already extracts these values via its plugins and the embedded
//! 516-entry `DFIRBatch.reb`; re-parsing the hives here would duplicate that
//! logic and create a second source of truth that can disagree with the CSVs
//! an analyst is reading alongside this file. So this module is a second pass
//! over `Registry/*_RETriage_results*.csv`, nothing more.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use triage_core::error::TriageError;

/// One reported field: the label an analyst reads, and the registry value it
/// is read from.
struct Field {
    label: &'static str,
    key_suffix: &'static str,
    value_name: &'static str,
}

// Verified against a real RETriage run over `test captures/Collection-
// DESKTOP-OA8SHHC-2026-03-12T22_54_56Z`'s SOFTWARE and SYSTEM hives: every
// `KeyPath` RETriage emits for these seven fields carries a `ROOT\` prefix
// (RECmd's hive-root convention), and the `TimeZoneKeyName` row comes from
// the `TimeZoneInfo` plugin while the other six are the batch file's plain
// default-dump rows -- so matching on a suffix rather than equality is what
// tolerates both the `ROOT\` prefix and the numbered `ControlSetNNN` that
// replaces the batch file's `ControlSet00*` wildcard, regardless of which
// path produced the row.
const FIELDS: &[Field] = &[
    Field {
        label: "Computer name",
        key_suffix: r"ControlSet001\Control\ComputerName\ComputerName",
        value_name: "ComputerName",
    },
    Field {
        label: "Operating system",
        key_suffix: r"Microsoft\Windows NT\CurrentVersion",
        value_name: "ProductName",
    },
    Field {
        label: "Build",
        key_suffix: r"Microsoft\Windows NT\CurrentVersion",
        value_name: "CurrentBuild",
    },
    Field {
        label: "Install date",
        key_suffix: r"Microsoft\Windows NT\CurrentVersion",
        value_name: "InstallDate",
    },
    Field {
        label: "Registered owner",
        key_suffix: r"Microsoft\Windows NT\CurrentVersion",
        value_name: "RegisteredOwner",
    },
    Field {
        label: "Time zone",
        key_suffix: r"Control\TimeZoneInformation",
        value_name: "TimeZoneKeyName",
    },
    Field {
        label: "Last shutdown",
        key_suffix: r"Control\Windows",
        value_name: "ShutdownTime",
    },
];

/// One row of RETriage's batch CSV, keyed on the two columns every `FIELDS`
/// lookup and the `ProfileList` scan need.
struct Row {
    key_path: String,
    value_name: String,
    value_data: String,
    value_data3: String,
}

/// Read every `*_RETriage_results*.csv` under `Registry/` into rows.
///
/// Returns `None` when the directory does not exist or holds no RETriage
/// batch CSV, which callers treat as "RETriage produced no output for this
/// collection" rather than an error.
fn read_retriage_rows(registry: &Path) -> Result<Option<Vec<Row>>, TriageError> {
    if !registry.is_dir() {
        return Ok(None);
    }

    let mut rows = Vec::new();
    let mut saw_any = false;
    for entry in std::fs::read_dir(registry).map_err(|e| TriageError::Output {
        path: registry.to_path_buf(),
        message: e.to_string(),
    })? {
        let entry = entry.map_err(|e| TriageError::Output {
            path: registry.to_path_buf(),
            message: e.to_string(),
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name.contains("_RETriage_results") && name.ends_with(".csv")) {
            continue;
        }
        saw_any = true;
        let mut reader = csv::Reader::from_path(entry.path()).map_err(|e| TriageError::Output {
            path: entry.path(),
            message: e.to_string(),
        })?;
        for record in reader.deserialize::<HashMap<String, String>>() {
            let Ok(record) = record else { continue };
            rows.push(Row {
                key_path: record.get("KeyPath").cloned().unwrap_or_default(),
                value_name: record.get("ValueName").cloned().unwrap_or_default(),
                value_data: record.get("ValueData").cloned().unwrap_or_default(),
                value_data3: record.get("ValueData3").cloned().unwrap_or_default(),
            });
        }
    }

    Ok(saw_any.then_some(rows))
}

/// Render one `FIELDS` entry as a two-line block: the label and value on the
/// first line, the registry key it came from on the second so an analyst can
/// verify the claim against the CSVs sitting next to this file.
fn render_field(out: &mut impl Write, field: &Field, rows: &[Row]) -> std::io::Result<()> {
    let value = rows
        .iter()
        .find(|row| row.value_name == field.value_name && row.key_path.ends_with(field.key_suffix))
        .map(|row| row.value_data.as_str());

    // "(not found)" rather than an omitted line: a missing line cannot be
    // told apart from a field this report does not cover.
    writeln!(out, "{}: {}", field.label, value.unwrap_or("(not found)"))?;
    writeln!(
        out,
        "    source: {}\\{}",
        field.key_suffix, field.value_name
    )
}

/// `ProfileList` rows are not a plain name/value pair: RETriage's plugin
/// consolidates each SID's subkey into one `Multiple` row per the C#
/// `ValuesOut` shape (`ValueData` = `"KeyName: {sid}"`, `ValueData3` =
/// `"ProfileImagePath: {path}"`), confirmed against the same real run. A
/// `FIELDS`-style single-column lookup would never match this shape.
fn profile_list_entries(rows: &[Row]) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = rows
        .iter()
        .filter(|row| row.value_name == "Multiple" && row.key_path.ends_with(r"ProfileList"))
        .filter_map(|row| {
            let sid = row.value_data.strip_prefix("KeyName: ")?;
            let path = row.value_data3.strip_prefix("ProfileImagePath: ")?;
            Some((sid.to_string(), path.to_string()))
        })
        .collect();
    entries.sort();
    entries.dedup();
    entries
}

/// Write `CaseInfo/<stamp>_SysInfo.txt`.
///
/// `Ok(None)` when RETriage produced no output for this collection (no
/// `Registry/` directory, or none of its CSVs are RETriage batch output) --
/// there is nothing to report a second pass over.
///
/// `time_range_notice` is the run's honesty statement
/// (`crate::time_range_notice`), `None` when no `--start`/`--end` was given.
pub fn write_sysinfo(
    collection_dir: &Path,
    stamp: &str,
    time_range_notice: Option<&str>,
) -> Result<Option<PathBuf>, TriageError> {
    let registry = collection_dir.join("Registry");
    let Some(rows) = read_retriage_rows(&registry)? else {
        return Ok(None);
    };

    let case_info = collection_dir.join("CaseInfo");
    std::fs::create_dir_all(&case_info).map_err(|e| TriageError::Output {
        path: case_info.clone(),
        message: e.to_string(),
    })?;
    let destination = case_info.join(format!("{stamp}_SysInfo.txt"));
    let mut out = std::fs::File::create(&destination).map_err(|e| TriageError::Output {
        path: destination.clone(),
        message: e.to_string(),
    })?;

    (|| -> std::io::Result<()> {
        writeln!(
            out,
            "# TriageSuite SysInfo -- derived from RETriage output, not from the hives directly."
        )?;
        writeln!(
            out,
            "# User accounts are from SOFTWARE\\...\\ProfileList (SID to profile path)."
        )?;
        writeln!(
            out,
            "# This is not the SAM local-account list: RETriage has no SAM account plugin."
        )?;
        writeln!(out, "# Run stamp: {stamp}")?;
        if let Some(notice) = time_range_notice {
            writeln!(out, "# {notice}")?;
        }
        writeln!(out)?;

        for field in FIELDS {
            render_field(&mut out, field, &rows)?;
        }

        writeln!(out)?;
        writeln!(out, "User profiles (from ProfileList):")?;
        let profiles = profile_list_entries(&rows);
        if profiles.is_empty() {
            writeln!(out, "    (not found)")?;
        } else {
            for (sid, path) in &profiles {
                writeln!(out, "    {sid}  {path}")?;
            }
        }
        Ok(())
    })()
    .map_err(|e| TriageError::Output {
        path: destination.clone(),
        message: e.to_string(),
    })?;

    Ok(Some(destination))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysinfo_reads_what_retriage_wrote() {
        let tmp = tempfile::tempdir().unwrap();
        let collection = tmp.path();
        std::fs::create_dir_all(collection.join("Registry")).unwrap();
        std::fs::write(
            collection.join("Registry/2026-03-13T192553Z_RETriage_results_Batch.csv"),
            "HivePath,HiveType,Description,Category,KeyPath,ValueName,ValueType,ValueData,ValueData2,ValueData3,Comment,Recursive,Deleted,LastWriteTimestamp,PluginDetailFile\n\
             S,System,TimeZone,System,ROOT\\ControlSet001\\Control\\TimeZoneInformation,TimeZoneKeyName,(plugin),Eastern Standard Time,Eastern Standard Time,,,false,false,2026-03-01T00:00:00.0000000Z,TimeZoneInfo_SYSTEM.csv\n\
             S,Software,ProductName,System,ROOT\\Microsoft\\Windows NT\\CurrentVersion,ProductName,RegSz,Windows 11 Pro,,,,false,false,2026-03-01T00:00:00.0000000Z,\n\
             S,Software,User Accounts,User Accounts,ROOT\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList,Multiple,(plugin),KeyName: S-1-5-21-1,Timestamp: 2026-03-01 00:00:00.0000000,ProfileImagePath: C:\\Users\\alice,User accounts,false,false,2026-03-01T00:00:00.0000000Z,ProfileList_SOFTWARE.csv\n",
        )
        .unwrap();

        let written = write_sysinfo(collection, "2026-03-13T192553Z", None)
            .unwrap()
            .unwrap();
        let body = std::fs::read_to_string(written).unwrap();
        assert!(body.contains("Eastern Standard Time"), "got {body}");
        assert!(body.contains("Windows 11 Pro"), "got {body}");
        assert!(
            body.contains("ProfileList"),
            "the report must name its source for user accounts: {body}"
        );
        assert!(
            body.contains("S-1-5-21-1") && body.contains(r"C:\Users\alice"),
            "the ProfileList row must be decoded from its Multiple/KeyName/ProfileImagePath \
             shape, not looked up as a plain ValueName: {body}"
        );
        assert!(
            body.contains("(not found)"),
            "fields RETriage did not emit (e.g. Computer name here) must say so: {body}"
        );
        assert!(
            body.to_lowercase().contains("sam"),
            "the header must state the SAM gap: {body}"
        );
    }

    /// The honesty statement must be readable in the SysInfo report itself,
    /// not just inferable from the manifest's `time_filter` field.
    #[test]
    fn time_range_notice_is_written_when_given() {
        let tmp = tempfile::tempdir().unwrap();
        let collection = tmp.path();
        std::fs::create_dir_all(collection.join("Registry")).unwrap();
        std::fs::write(
            collection.join("Registry/2026-03-13T192553Z_RETriage_results_Batch.csv"),
            "HivePath,HiveType,Description,Category,KeyPath,ValueName,ValueType,ValueData,ValueData2,ValueData3,Comment,Recursive,Deleted,LastWriteTimestamp,PluginDetailFile\n",
        )
        .unwrap();

        let notice = "Time range 2026-03-01T00:00:00Z .. 2026-03-10T00:00:00Z applies to \
                       event logs only (EvtxTriage, Hayabusa, Takajo). All other tools emit \
                       their full output.";
        let written = write_sysinfo(collection, "2026-03-13T192553Z", Some(notice))
            .unwrap()
            .unwrap();
        let body = std::fs::read_to_string(written).unwrap();
        assert!(body.contains(notice), "got {body}");
    }

    #[test]
    fn sysinfo_is_skipped_when_retriage_produced_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(write_sysinfo(tmp.path(), "X", None).unwrap().is_none());
    }

    #[test]
    fn sysinfo_is_skipped_when_there_is_no_registry_directory_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(write_sysinfo(tmp.path(), "X", None).unwrap().is_none());
        assert!(!tmp.path().join("CaseInfo").exists());
    }
}
