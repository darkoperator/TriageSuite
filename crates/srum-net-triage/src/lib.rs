//! SrumNetTriage: rolls up SrumETriage's NetworkUsage/NetworkConnection CSV
//! output into per-day exfil-volume and per-hour-of-day activity-fingerprint
//! tables. No Zimmerman equivalent. Consumes other TriageSuite tool output
//! rather than raw artifacts, the same shape as LolTriage.

pub mod aggregate;
pub mod sniff;
pub mod timezone;

use std::io::BufRead;
use std::path::Path;

use aggregate::{BusinessHours, ConnectionRow, TzOffset, UsageRow};
use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};

pub const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        id: "daily_summary",
        default_basename: "SrumNetTriage_DailySummary_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    },
    DatasetSpec {
        id: "hourly_fingerprint",
        default_basename: "SrumNetTriage_HourlyFingerprint_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_HourlyFingerprint"),
    },
    DatasetSpec {
        id: "session_summary",
        default_basename: "SrumNetTriage_SessionSummary_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_SessionSummary"),
    },
];

pub struct SrumNetTool {
    pub tz: TzOffset,
    pub business_hours: BusinessHours,
}

impl Tool for SrumNetTool {
    fn binary_name(&self) -> &'static str {
        "SrumNetTriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &["*.csv"]
    }

    fn validate_legacy(&self, path: &Path) -> bool {
        // Inputs are SrumETriage's full CSV output and can be large; read
        // only the header line.
        let Ok(file) = std::fs::File::open(path) else {
            return false;
        };
        let Some(Ok(first_line)) = std::io::BufReader::new(file).lines().next() else {
            return false;
        };
        sniff::sniff(&first_line).is_some()
    }

    fn invalid_content_is_corrupt(&self) -> bool {
        false
    }

    fn datasets(&self) -> &'static [DatasetSpec] {
        DATASETS
    }

    fn scope(&self) -> Scope {
        Scope::SystemWide
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        parse_impl(self.tz, self.business_hours, path, out)
    }
}

fn artifact_err(path: &Path) -> impl Fn(csv::Error) -> TriageError + '_ {
    move |e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: e.to_string(),
    }
}

fn parse_impl(
    tz: TzOffset,
    business_hours: BusinessHours,
    path: &Path,
    out: &mut OutputRouter,
) -> Result<u64, TriageError> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_path(path)
        .map_err(artifact_err(path))?;
    let header = reader
        .headers()
        .map_err(artifact_err(path))?
        .iter()
        .collect::<Vec<_>>()
        .join(",");
    let Some(kind) = sniff::sniff(&header) else {
        return Ok(0);
    };

    let mut count = 0u64;
    match kind {
        sniff::SourceKind::NetworkUsage => {
            let mut rows = Vec::new();
            for result in reader.deserialize::<UsageRow>() {
                rows.push(result.map_err(artifact_err(path))?);
            }
            for record in aggregate::aggregate_daily(&rows, tz) {
                out.write("daily_summary", &record)?;
                count += 1;
            }
            for record in aggregate::aggregate_hourly(&rows, tz, business_hours) {
                out.write("hourly_fingerprint", &record)?;
                count += 1;
            }
        }
        sniff::SourceKind::NetworkConnection => {
            let mut rows = Vec::new();
            for result in reader.deserialize::<ConnectionRow>() {
                rows.push(result.map_err(artifact_err(path))?);
            }
            for record in aggregate::aggregate_sessions(&rows, tz) {
                out.write("session_summary", &record)?;
                count += 1;
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tool_tests {
    use super::*;
    use triage_core::output::layout::OutputLayoutMode;
    use triage_core::output::router::{run_stamp, RouterOptions};

    fn tool() -> SrumNetTool {
        SrumNetTool {
            tz: TzOffset(0),
            business_hours: "08:00-18:00".parse().unwrap(),
        }
    }

    #[test]
    fn parses_network_usage_csv_into_daily_and_hourly_datasets() {
        let tool = tool();
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("SrumETriage_NetworkUsages_Output.csv");
        std::fs::write(
            &input,
            "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,UserId,AppId,BytesReceived,BytesSent,InterfaceLuid,InterfaceType,L2ProfileFlags,L2ProfileId,ProfileName\n\
             1,2024-06-29T02:00:00.0000000Z,chrome.exe,,,,,alice,1,1,50,900,0,Wired80211,0,1,\n",
        )
        .unwrap();

        assert!(tool.validate_legacy(&input));

        let out_dir = tmp.path().join("out");
        let mut router = OutputRouter::new(
            tool.binary_name(),
            tool.datasets(),
            RouterOptions {
                csv_root: Some(out_dir.clone()),
                json_root: None,
                csvf: None,
                jsonf: None,
                pretty: false,
                overwrite: false,
                run_stamp: Some(run_stamp()),
                layout_mode: OutputLayoutMode::Flat,
            },
        )
        .unwrap();
        router.set_identity(triage_core::attribution::Identity::System);

        let count = tool.parse(&input, &mut router).unwrap();
        // 1 daily_summary row + 1 hourly_fingerprint row.
        assert_eq!(count, 2);
    }

    #[test]
    fn parses_network_connection_csv_into_session_dataset() {
        let tool = tool();
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("SrumETriage_NetworkConnections_Output.csv");
        std::fs::write(
            &input,
            "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,UserId,AppId,ConnectedTime,ConnectStartTime,InterfaceLuid,InterfaceType,L2ProfileFlags,L2ProfileId,ProfileName\n\
             1,2024-06-29T02:00:00.0000000Z,chrome.exe,,,,,alice,1,1,300,,0,Wired80211,0,1,\n",
        )
        .unwrap();

        assert!(tool.validate_legacy(&input));

        let out_dir = tmp.path().join("out");
        let mut router = OutputRouter::new(
            tool.binary_name(),
            tool.datasets(),
            RouterOptions {
                csv_root: Some(out_dir.clone()),
                json_root: None,
                csvf: None,
                jsonf: None,
                pretty: false,
                overwrite: false,
                run_stamp: Some(run_stamp()),
                layout_mode: OutputLayoutMode::Flat,
            },
        )
        .unwrap();
        router.set_identity(triage_core::attribution::Identity::System);

        let count = tool.parse(&input, &mut router).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn unrelated_csv_is_rejected_by_validate_legacy() {
        let tool = tool();
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("other.csv");
        std::fs::write(&input, "Foo,Bar\n1,2\n").unwrap();
        assert!(!tool.validate_legacy(&input));
    }
}
