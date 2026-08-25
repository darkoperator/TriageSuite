pub mod candidate;
pub mod matcher;
pub mod refdata;
pub mod sniff;
pub mod update_refs;

use refdata::LolRefs;
use std::io::BufRead;
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};

pub const DATASETS: &[DatasetSpec] = &[DatasetSpec {
    id: "findings",
    default_basename: "LolTriage_Output",
    framing: JsonFraming::Ndjson,
    csv_only: false,
    override_suffix: None,
}];

pub struct LolTool {
    pub refs: LolRefs,
}

impl Tool for LolTool {
    fn binary_name(&self) -> &'static str {
        "LolTriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &["*.csv"]
    }

    fn validate_legacy(&self, path: &Path) -> bool {
        // Read only the header line: these inputs are other tools' full CSV
        // output and can be hundreds of MB.
        let Ok(file) = std::fs::File::open(path) else {
            return false;
        };
        let Some(Ok(first_line)) = std::io::BufReader::new(file).lines().next() else {
            return false;
        };
        // BufRead::lines strips the trailing newline and any CR, matching the
        // previous str::lines behaviour.
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
        parse_impl(&self.refs, path, out)
    }
}

fn parse_impl(refs: &LolRefs, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
    // Stream the file rather than buffering it: inputs are other tools' full
    // CSV output and can be very large.
    let artifact_err = |e: csv::Error| TriageError::Artifact {
        path: path.to_path_buf(),
        message: e.to_string(),
    };
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_path(path)
        .map_err(artifact_err)?;
    let header = reader
        .headers()
        .map_err(artifact_err)?
        .iter()
        .collect::<Vec<_>>()
        .join(",");
    let Some(kind) = sniff::sniff(&header) else {
        return Ok(0);
    };
    let source_file = path.display().to_string();
    let mut count = 0u64;
    for result in reader.records() {
        let row = result.map_err(artifact_err)?;
        let Some(candidate) = candidate::from_record(kind, &row, &source_file) else {
            continue;
        };
        for finding in matcher::match_candidate(&candidate, refs) {
            out.write("findings", &finding)?;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tool_tests {
    use super::*;
    use refdata::{LolDriverEntry, LolRmmEntry};
    use triage_core::output::layout::OutputLayoutMode;
    use triage_core::output::router::{run_stamp, RouterOptions};

    fn refs() -> LolRefs {
        LolRefs::new(
            vec![LolDriverEntry {
                id: "2a6a38ca-f2e6-456e-9ccf-db59d8c80c9e".into(),
                category: "vulnerable driver".into(),
                mitre_id: "T1068".into(),
                tags: vec!["nvflash.sys".into()],
                md5: String::new(),
                sha1: "b9c3f4dcc7463cbec84b808d880194bbc304ccd0".into(),
                sha256: String::new(),
            }],
            vec![LolRmmEntry {
                name: "KiTTY".into(),
                category: "RAT".into(),
                install_basenames: vec!["kitty.exe".into()],
                sha256_hashes: vec![],
            }],
        )
    }

    #[test]
    fn parse_amcache_csv_emits_one_finding() {
        let tool = LolTool { refs: refs() };
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("amcache.csv");
        std::fs::write(
            &input,
            "ApplicationName,ProgramId,FileKeyLastWriteTimestamp,SHA1,IsOsComponent,FullPath,Name,FileExtension,LinkDate,ProductName,Size,Version,ProductVersion,LongPathHash,BinaryType,IsPeFile,BinFileVersion,BinProductVersion,Usn,Language,Description\n\
             Unassociated,prog-1,2024-01-01T00:00:00.0000000Z,b9c3f4dcc7463cbec84b808d880194bbc304ccd0,False,C:\\Windows\\System32\\drivers\\nvflash.sys,nvflash.sys,.sys,,,1024,,,,,False,,,0,,\n",
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
}
