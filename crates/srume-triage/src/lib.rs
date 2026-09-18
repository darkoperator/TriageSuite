//! SrumETriage: SrumECmd-compatible Windows SRUM (SRUDB.dat) parser.

pub mod cli;
pub mod datasets;
pub mod idmap;
pub mod profiles;
pub mod sidtype;

use std::path::{Path, PathBuf};

use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::duckdb::types::{ColumnType, DatasetColumnTypes, SqlType, TimeSemantics};
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

/// Declared SQL types for the DuckDB view layer.
///
/// Every `i64`/`WinTimestamp` field from `datasets.rs` is declared.
/// `EnergyUsageRecord::IsLt` carries `serialize_with = "serialize_bool_titlecase"`
/// and `VfuprovRecord::Duration` is a computed/formatted `String`
/// ("d.hh:mm:ss"); both are OMIT cases. `ExeInfo`, `ExeInfoDescription`,
/// `SidType`, `Sid`, `UserName`, `InterfaceType` and `ProfileName` are free
/// text and also stay undeclared.
pub const COLUMN_TYPES: &[DatasetColumnTypes] = &[
    DatasetColumnTypes {
        dataset_id: "network_usage",
        columns: &[
            ColumnType {
                column: "Id",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "Timestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "ExeTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "UserId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "AppId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "BytesReceived",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "BytesSent",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "InterfaceLuid",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "L2ProfileFlags",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "L2ProfileId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "network_connections",
        columns: &[
            ColumnType {
                column: "Id",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "Timestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "ExeTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "UserId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "AppId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ConnectedTime",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ConnectStartTime",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "InterfaceLuid",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "L2ProfileFlags",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "L2ProfileId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "app_resource_usage",
        columns: &[
            ColumnType {
                column: "Id",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "Timestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "ExeTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "UserId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "AppId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "BackgroundBytesRead",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "BackgroundBytesWritten",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "BackgroundContextSwitches",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "BackgroundCycleTime",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "BackgroundNumberOfFlushes",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "BackgroundNumReadOperations",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "BackgroundNumWriteOperations",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "FaceTime",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ForegroundBytesRead",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ForegroundBytesWritten",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ForegroundContextSwitches",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ForegroundCycleTime",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ForegroundNumberOfFlushes",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ForegroundNumReadOperations",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ForegroundNumWriteOperations",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "push_notifications",
        columns: &[
            ColumnType {
                column: "Id",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "Timestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "ExeTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "UserId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "AppId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "NetworkType",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "NotificationType",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "PayloadSize",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "energy_usage",
        columns: &[
            ColumnType {
                column: "Id",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "Timestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "ExeTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "UserId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "AppId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ConfigurationHash",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "EventTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "StateTransition",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ChargeLevel",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "CycleCount",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "DesignedCapacity",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "FullChargedCapacity",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ActiveAcTime",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ActiveDcTime",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ActiveDischargeTime",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ActiveEnergy",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "CsAcTime",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "CsDcTime",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "CsDischargeTime",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "CsEnergy",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "app_timeline",
        columns: &[
            ColumnType {
                column: "Id",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "Timestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "ExeTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "UserId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "AppId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "EndTime",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "DurationMs",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "vfuprov",
        columns: &[
            ColumnType {
                column: "Id",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "Timestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "UserId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "AppId",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "ExeTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "StartTime",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "EndTime",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "Flags",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
        ],
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
