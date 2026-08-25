//! SrumETriage: SrumECmd-compatible Windows SRUM (SRUDB.dat) parser.

pub mod cli;
pub mod datasets;
pub mod idmap;
pub mod profiles;
pub mod sidtype;

use std::path::{Path, PathBuf};

use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};
use triage_ese::{Database, EseError};

pub const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        id: "network_usage",
        default_basename: "SrumETriage_NetworkUsages_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    },
    DatasetSpec {
        id: "network_connections",
        default_basename: "SrumETriage_NetworkConnections_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_NetworkConnections"),
    },
    DatasetSpec {
        id: "app_resource_usage",
        default_basename: "SrumETriage_AppResourceUseInfo_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_AppResourceUseInfo"),
    },
    DatasetSpec {
        id: "push_notifications",
        default_basename: "SrumETriage_PushNotifications_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_PushNotifications"),
    },
    DatasetSpec {
        id: "energy_usage",
        default_basename: "SrumETriage_EnergyUsage_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_EnergyUsage"),
    },
    DatasetSpec {
        id: "app_timeline",
        default_basename: "SrumETriage_AppTimelineProvider_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_AppTimelineProvider"),
    },
    DatasetSpec {
        id: "vfuprov",
        default_basename: "SrumETriage_vfuprov_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_vfuprov"),
    },
];

#[derive(Default)]
pub struct SrumeTool {
    pub software: Option<PathBuf>,
}

impl Tool for SrumeTool {
    fn binary_name(&self) -> &'static str {
        "SrumETriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &["SRUDB.dat"]
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
        // Check format revision before attempting a load.
        let rev = triage_ese::header::format_revision(path).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        if rev > triage_ese::header::MAX_SUPPORTED_REVISION {
            eprintln!(
                "SrumETriage: {}: unsupported ESE format revision {rev} \
                 — newer than any available parser; skipping",
                path.display()
            );
            return Ok(0);
        }

        let db = match Database::open(path) {
            Ok(d) => d,
            Err(EseError::UnsupportedRevision(r)) => {
                eprintln!(
                    "SrumETriage: {}: unsupported ESE format revision {r} \
                     — newer than any available parser; skipping",
                    path.display()
                );
                return Ok(0);
            }
            Err(e) => {
                return Err(TriageError::Artifact {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                })
            }
        };

        if db.is_dirty() {
            eprintln!(
                "SrumETriage: {}: WARNING — database was not cleanly shut down (dirty); \
                 emitted records may be incomplete or inconsistent",
                path.display()
            );
        }

        let maps = idmap::build_id_maps(&db).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        // Resolve SID -> username: prefer explicit --software, else auto-locate.
        let sw = self
            .software
            .clone()
            .or_else(|| profiles::closest_software_hive(path));
        let users = sw.map(|p| profiles::sid_to_user(&p)).unwrap_or_default();

        let mut count = 0u64;

        // 1. NetworkUsages
        if db.table_exists(datasets::NETWORK_USAGE_TABLE) {
            for rec in datasets::read_network_usage(&db, &maps, &users) {
                out.write("network_usage", &rec)?;
                count += 1;
            }
        }

        // 2. NetworkConnections
        if db.table_exists(datasets::NETWORK_CONNECTION_TABLE) {
            for rec in datasets::read_network_connection(&db, &maps, &users) {
                out.write("network_connections", &rec)?;
                count += 1;
            }
        }

        // 3. AppResourceUseInfo
        if db.table_exists(datasets::APP_RESOURCE_USE_TABLE) {
            for rec in datasets::read_app_resource_use(&db, &maps, &users) {
                out.write("app_resource_usage", &rec)?;
                count += 1;
            }
        }

        // 4. PushNotifications
        if db.table_exists(datasets::PUSH_NOTIFICATION_TABLE) {
            for rec in datasets::read_push_notification(&db, &maps, &users) {
                out.write("push_notifications", &rec)?;
                count += 1;
            }
        }

        // 5. EnergyUsage (reads both base + LT tables internally — no gate needed)
        for rec in datasets::read_energy_usage(&db, &maps, &users) {
            out.write("energy_usage", &rec)?;
            count += 1;
        }

        // 6. AppTimelineProvider
        if db.table_exists(datasets::TIMELINE_PROVIDER_TABLE) {
            for rec in datasets::read_timeline_provider(&db, &maps, &users) {
                out.write("app_timeline", &rec)?;
                count += 1;
            }
        }

        // 7. Vfuprov
        if db.table_exists(datasets::VFUPROV_TABLE) {
            for rec in datasets::read_vfuprov(&db, &maps, &users) {
                out.write("vfuprov", &rec)?;
                count += 1;
            }
        }

        Ok(count)
    }
}
