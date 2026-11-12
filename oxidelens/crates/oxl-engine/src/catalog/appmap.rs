//! What the app knows about datasets that the suite does not say itself:
//! columns the suite leaves as text but whose shape the app can type, and
//! which column a relation's timeline runs on.

use super::{SqlType, TimeSemantics};

/// A column the app types at load, beside the suite's own typing.
#[derive(Debug, Clone, Copy)]
pub struct AppType {
    pub relation: &'static str,
    pub column: &'static str,
    pub sql_type: SqlType,
    pub rule: AppRule,
}

/// Which values of the column the app casts, and what it claims about them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppRule {
    /// Cast only a value whose text ends in `Z`, and catalogue the column as
    /// UTC. A value without the `Z` names no zone, so it stays untyped
    /// rather than being read as UTC on a guess.
    UtcWhenTextEndsInZ,
}

impl AppRule {
    /// What the rule claims about the values it casts.
    pub fn time_semantics(&self) -> Option<TimeSemantics> {
        match self {
            AppRule::UtcWhenTextEndsInZ => Some(TimeSemantics::Utc),
        }
    }
}

/// A timestamp the app types by `AppRule::UtcWhenTextEndsInZ`.
const fn utc_z(relation: &'static str, column: &'static str) -> AppType {
    AppType {
        relation,
        column,
        sql_type: SqlType::Timestamp,
        rule: AppRule::UtcWhenTextEndsInZ,
    }
}

/// Every column the app types. Each holds times as text the suite does not
/// declare a zone for; with a trailing `Z` a value is UTC. Hayabusa's
/// timestamps are the first. The rest were found on a real run, each
/// written in the Z form wherever it was set: AppCompat's last-modified time,
/// a prefetch file's last run, a scheduled task's creation and last start
/// and stop, UserAssist's last execution, and a shellbag's key write and
/// first and last interaction. A column whose name says local time, or
/// whose values are written another way, is not here.
pub const APP_TYPES: &[AppType] = &[
    utc_z("ext_hayabusa_csv_timeline", "Timestamp"),
    utc_z("appcompattriage_appcompat", "LastModifiedTimeUTC"),
    utc_z("petriage_main", "LastRun"),
    utc_z("retriage_taskcache_software", "CreatedOn"),
    utc_z("retriage_taskcache_software", "LastStart"),
    utc_z("retriage_taskcache_software", "LastStop"),
    utc_z("retriage_userassist_ntuser_dat", "LastExecuted"),
    utc_z("sbetriage_shellbags", "LastWriteTime"),
    utc_z("sbetriage_shellbags", "FirstInteracted"),
    utc_z("sbetriage_shellbags", "LastInteracted"),
];

/// The column each relation's timeline runs on by default, when it is a
/// catalogued timestamp. A relation not listed here takes its first
/// catalogued timestamp column.
pub const PRIMARY_TIME: &[(&str, &str)] = &[
    ("evtxtriage_events", "TimeCreated"),
    ("mftriage_mft", "Created0x10"),
    ("mftriage_usn", "UpdateTimestamp"),
    ("petriage_timeline", "RunTime"),
    ("rbtriage_main", "DeletedOn"),
    ("ext_hayabusa_csv_timeline", "Timestamp"),
    ("appcompattriage_appcompat", "LastModifiedTimeUTC"),
    // The last run, not the first of the earlier runs the suite types.
    ("petriage_main", "LastRun"),
    // Set for every task, where the last start and stop often are not.
    ("retriage_taskcache_software", "CreatedOn"),
    ("retriage_userassist_ntuser_dat", "LastExecuted"),
    // Its BagMRU key's last write, which SBETriage writes on every row;
    // LastInteracted is the same time on the most recent slot alone.
    ("sbetriage_shellbags", "LastWriteTime"),
];

/// The columns whose texts make a row's one-line summary in the merged row
/// list, in order: what the row is (a path, a program, a task), not the
/// key paths, slots and counters the tools write first. A listed column a
/// relation lacks is passed over. A relation not listed here is summarised
/// by its first four non-empty values among columns that are neither
/// metadata nor timestamps. The column names are the ones a real run's
/// datasets carry.
pub const SUMMARY: &[(&str, &[&str])] = &[
    (
        "evtxtriage_events",
        &["Channel", "EventId", "MapDescription", "UserName"],
    ),
    (
        "ext_hayabusa_csv_timeline",
        &["Level", "RuleTitle", "Details"],
    ),
    // File system.
    ("mftriage_mft", &["ParentPath", "FileName", "FileSize"]),
    ("mftriage_usn", &["ParentPath", "Name", "UpdateReasons"]),
    ("sbetriage_shellbags", &["AbsolutePath", "ShellType"]),
    (
        "letriage_main",
        &[
            "LocalPath",
            "NetworkPath",
            "TargetIDAbsolutePath",
            "Arguments",
        ],
    ),
    ("jletriage_auto", &["AppIdDescription", "Path", "LocalPath"]),
    (
        "jletriage_custom",
        &["AppIdDescription", "EntryName", "LocalPath"],
    ),
    // Execution.
    ("petriage_main", &["ExecutableName", "RunCount"]),
    ("appcompattriage_appcompat", &["Path", "Executed"]),
    ("retriage_appcompatcache_system", &["ProgramName"]),
    ("retriage_bamdam_system", &["Program"]),
    (
        "retriage_userassist_ntuser_dat",
        &["ProgramName", "RunCounter"],
    ),
    (
        "amcachetriage_associated_file_entries",
        &["FullPath", "Name", "SHA1", "ProductName"],
    ),
    (
        "amcachetriage_unassociated_file_entries",
        &["FullPath", "Name", "SHA1", "ProductName"],
    ),
    (
        "amcachetriage_program_entries",
        &["Name", "Version", "Publisher"],
    ),
    ("amcachetriage_shortcuts", &["LnkName"]),
    (
        "amcachetriage_drive_binaries",
        &["DriverName", "DriverCompany"],
    ),
    (
        "amcachetriage_device_pnps",
        &["Description", "Manufacturer"],
    ),
    (
        "amcachetriage_device_containers",
        &["FriendlyName", "Manufacturer"],
    ),
    // Persistence and configuration.
    (
        "retriage_taskcache_software",
        &["Path", "Command", "Arguments"],
    ),
    (
        "retriage_services_system",
        &["Name", "ImagePath", "StartMode"],
    ),
    (
        "retriage_uninstall_software",
        &["DisplayName", "DisplayVersion", "Publisher"],
    ),
    (
        "retriage_uninstall_ntuser_dat",
        &["DisplayName", "DisplayVersion", "Publisher"],
    ),
    ("retriage_products_software", &["DisplayName", "Publisher"]),
    ("retriage_batch", &["Description", "KeyPath", "ValueData"]),
    ("retriage_apppaths_software", &["FileName", "Path1"]),
    ("retriage_profilelist_software", &["ProfileImagePath"]),
    ("retriage_radar_software", &["Filename"]),
    ("retriage_typedurls_ntuser_dat", &["Url"]),
    ("retriage_typedurls_ntuser_dat_default", &["Url"]),
    ("retriage_typedurls_ntuser_dat_localservice", &["Url"]),
    ("retriage_typedurls_ntuser_dat_networkservice", &["Url"]),
    // Devices.
    (
        "retriage_scsi_system",
        &["Manufacturer", "Title", "SerialNumber"],
    ),
    ("retriage_deviceclasses_system", &["Name", "SerialNumber"]),
    ("retriage_networkadapters_system", &["DriverDesc"]),
    (
        "retriage_volumeinfocache_software",
        &["DriveName", "VolumeLabel"],
    ),
    // SRUM: which program, for whom, and what it did.
    ("srumetriage_app_resource_usage", &["ExeInfo", "UserName"]),
    (
        "srumetriage_app_timeline",
        &["ExeInfo", "UserName", "DurationMs"],
    ),
    (
        "srumetriage_network_usage",
        &["ExeInfo", "UserName", "BytesSent", "BytesReceived"],
    ),
    (
        "srumetriage_network_connections",
        &["ExeInfo", "UserName", "InterfaceType"],
    ),
    (
        "srumetriage_push_notifications",
        &["ExeInfo", "UserName", "NotificationType"],
    ),
    ("srumetriage_energy_usage", &["ExeInfo", "UserName"]),
    ("srumetriage_vfuprov", &["ExeInfo", "UserName"]),
    // Windows Timeline.
    (
        "wxttriage_activity",
        &["Executable", "DisplayText", "ActivityType"],
    ),
];

/// The `SUMMARY` entry for `relation`, if it has one.
pub(crate) fn summary_for(relation: &str) -> Option<&'static [&'static str]> {
    SUMMARY
        .iter()
        .find(|(r, _)| *r == relation)
        .map(|(_, c)| *c)
}

/// The rule the app types `relation.column` by, if it does.
pub(crate) fn rule_for(relation: &str, column: &str) -> Option<AppRule> {
    APP_TYPES
        .iter()
        .find(|t| t.relation == relation && t.column == column)
        .map(|t| t.rule)
}

/// The `PRIMARY_TIME` entry for `relation`, if it has one.
pub(crate) fn primary_time_for(relation: &str) -> Option<&'static str> {
    PRIMARY_TIME
        .iter()
        .find(|(r, _)| *r == relation)
        .map(|(_, c)| *c)
}
