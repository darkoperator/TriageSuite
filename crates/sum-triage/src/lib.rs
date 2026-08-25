//! SumETriage: SumECmd-compatible Windows SUM / User Access Logging parser.

pub mod datasets;
pub mod detail;
pub mod helpers;
pub mod identity;

use std::path::Path;

use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};

pub const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        id: "system_ident",
        default_basename: "SumETriage_SystemIdentInfo_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_SystemIdentInfo"),
    },
    DatasetSpec {
        id: "role_infos",
        default_basename: "SumETriage_RoleInfos_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_RoleInfos"),
    },
    DatasetSpec {
        id: "chained_db",
        default_basename: "SumETriage_ChainedDbInfo_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_ChainedDbInfo"),
    },
    DatasetSpec {
        id: "clients",
        default_basename: "SumETriage_Clients_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_Clients"),
    },
    DatasetSpec {
        id: "clients_detailed",
        default_basename: "SumETriage_ClientsDetailed_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_ClientsDetailed"),
    },
    DatasetSpec {
        id: "dns_info",
        default_basename: "SumETriage_DnsInfo_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_DnsInfo"),
    },
    DatasetSpec {
        id: "role_accesses",
        default_basename: "SumETriage_RoleAccesses_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_RoleAccesses"),
    },
    DatasetSpec {
        id: "vm_info",
        default_basename: "SumETriage_VmInfo_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_VmInfo"),
    },
];

pub struct SumTool;

impl Default for SumTool {
    fn default() -> Self {
        SumTool
    }
}

impl Tool for SumTool {
    fn binary_name(&self) -> &'static str {
        "SumETriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &["SystemIdentity.mdb"]
    }

    fn validate_legacy(&self, path: &Path) -> bool {
        triage_ese::header::is_ese(path)
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
        let artifact_err = |e: triage_ese::EseError| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        };

        let db = triage_ese::Database::open(path).map_err(artifact_err)?;
        if db.is_dirty() {
            eprintln!(
                "SumETriage: {}: WARNING — database was not cleanly shut down (dirty); \
                 emitted records may be incomplete or inconsistent",
                path.display()
            );
        }

        let ident = identity::read(&db).map_err(artifact_err)?;

        let mut count = 0u64;
        for rec in &ident.system_idents {
            out.write("system_ident", rec)?;
            count += 1;
        }
        for rec in &ident.role_infos {
            out.write("role_infos", rec)?;
            count += 1;
        }
        for rec in &ident.chained {
            out.write("chained_db", rec)?;
            count += 1;
        }

        // --- DETAIL pass: Current.mdb (current year) + each chained .mdb ---
        use chrono::Datelike;
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let current_year = chrono::Utc::now().year();

        // Work list: (filename, year). Current.mdb first (SumECmd order), then
        // chained DBs from CHAINED_DATABASES.
        let mut work: Vec<(String, i32)> = vec![("Current.mdb".to_string(), current_year)];
        for c in &ident.chained {
            work.push((c.file_name.clone(), c.year as i32));
        }

        for (filename, year) in work {
            let db_path = dir.join(&filename);
            if !db_path.is_file() {
                continue; // listed but not captured — skip silently (SumECmd parity)
            }
            let sib = match triage_ese::Database::open(&db_path) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("SumETriage: {}: skipping ({e})", db_path.display());
                    continue;
                }
            };
            if sib.is_dirty() {
                eprintln!(
                    "SumETriage: {}: WARNING — database was not cleanly shut down (dirty); \
                     emitted records may be incomplete or inconsistent",
                    db_path.display()
                );
            }
            let det = detail::read(&sib, &ident.role_map, year, &filename);
            for rec in &det.clients {
                out.write("clients", rec)?;
                count += 1;
            }
            for rec in &det.clients_detailed {
                out.write("clients_detailed", rec)?;
                count += 1;
            }
            for rec in &det.dns {
                out.write("dns_info", rec)?;
                count += 1;
            }
            for rec in &det.role_accesses {
                out.write("role_accesses", rec)?;
                count += 1;
            }
            for rec in &det.vms {
                out.write("vm_info", rec)?;
                count += 1;
            }
        }

        Ok(count)
    }
}
