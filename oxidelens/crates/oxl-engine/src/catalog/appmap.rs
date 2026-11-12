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
/// timestamps are the first. The rest were found on real runs, each
/// written in the Z form wherever it was set: AppCompat's last-modified time,
/// a prefetch file's last run, a scheduled task's creation and last start
/// and stop, UserAssist's last execution, a shellbag's key write and first
/// and last interaction, the times BrowserTriage decodes from each
/// browser's own clock, the common dialogs' and recent documents' last
/// opening (set on the most recent slot alone), an Office file's last
/// opening and closing, and the times SumETriage
/// decodes from the User Access Logging databases. A column whose name says
/// local time, or whose values are written another way, is not here. Nor
/// are SUM's per-day client rows: their times are the client's for the
/// whole year, the same on every day, so on a timeline every day would sit
/// at one moment.
///
/// A `retriage_*_regback` relation takes the entries of the relation it
/// copies (`base_relation`), so none is listed here.
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
    utc_z("browsertriage_autofill", "First Used"),
    utc_z("browsertriage_autofill", "Last Used"),
    utc_z("browsertriage_bookmarks", "Date Added"),
    utc_z("browsertriage_bookmarks", "Date Modified"),
    utc_z("browsertriage_bookmarks", "Date Last Used"),
    utc_z("browsertriage_cookies", "Created"),
    utc_z("browsertriage_cookies", "Last Accessed"),
    utc_z("browsertriage_cookies", "Last Updated"),
    utc_z("browsertriage_cookies", "Expires"),
    utc_z("browsertriage_downloads", "Start Time"),
    utc_z("browsertriage_downloads", "End Time"),
    utc_z("browsertriage_downloads", "Last Access Time"),
    utc_z("browsertriage_extensions", "Install Time"),
    utc_z("browsertriage_extensions", "Update Time"),
    utc_z("browsertriage_history", "Visit Time"),
    utc_z("browsertriage_history", "Last Visit Time"),
    utc_z("browsertriage_keyword_searches", "Search Time"),
    utc_z("browsertriage_keyword_searches", "Last Visit Time"),
    utc_z("browsertriage_logins", "Date Created"),
    utc_z("browsertriage_timeline", "Timestamp"),
    utc_z("retriage_comdlg32_cidsizemru_ntuser_dat", "OpenedOn"),
    utc_z(
        "retriage_comdlg32_lastvisitedpidlmru_ntuser_dat",
        "OpenedOn",
    ),
    utc_z("retriage_comdlg32_opensavepidlmru_ntuser_dat", "OpenedOn"),
    utc_z("retriage_recent_documents_ntuser_dat", "OpenedOn"),
    utc_z("retriage_first_folder_ntuser_dat", "OpenedOn"),
    // LastClosed was empty on the real run; RETriage writes it the way it
    // writes LastOpened, and only the Z form is cast.
    utc_z("retriage_office_mru_ntuser_dat", "LastOpened"),
    utc_z("retriage_office_mru_ntuser_dat", "LastClosed"),
    utc_z("sumetriage_clients", "InsertDate"),
    utc_z("sumetriage_clients", "LastAccess"),
    utc_z("sumetriage_dns_info", "LastSeen"),
    utc_z("sumetriage_role_accesses", "FirstSeen"),
    utc_z("sumetriage_role_accesses", "LastSeen"),
    utc_z("sumetriage_system_ident", "CreationTime"),
];

/// The column each relation's timeline runs on by default, when it is a
/// catalogued timestamp. A relation not listed here takes its first
/// catalogued timestamp column: for a browser dataset the time its row
/// records (a visit, a download's start, a cookie's creation), for SUM the
/// first time it saw the client or role.
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
    // Browsers: what was visited, fetched or stored, not which browser and
    // profile, which every row starts with.
    ("browsertriage_history", &["URL", "Title"]),
    (
        "browsertriage_downloads",
        &["Target Path", "Download URL", "State"],
    ),
    ("browsertriage_cookies", &["Host", "Name", "Path"]),
    ("browsertriage_bookmarks", &["Title", "URL", "Folder Path"]),
    ("browsertriage_autofill", &["Field Name", "Value"]),
    (
        "browsertriage_extensions",
        &["Name", "Version", "Extension ID"],
    ),
    (
        "browsertriage_keyword_searches",
        &["Search Term", "Search Engine Host"],
    ),
    ("browsertriage_logins", &["Origin URL", "Username"]),
    (
        "browsertriage_timeline",
        &["Timestamp Type", "Artifact", "Value"],
    ),
    // Files opened through the shell.
    ("retriage_comdlg32_cidsizemru_ntuser_dat", &["Executable"]),
    (
        "retriage_comdlg32_lastvisitedpidlmru_ntuser_dat",
        &["Executable", "AbsolutePath"],
    ),
    (
        "retriage_comdlg32_opensavepidlmru_ntuser_dat",
        &["Extension", "AbsolutePath"],
    ),
    (
        "retriage_recent_documents_ntuser_dat",
        &["TargetName", "LnkName"],
    ),
    (
        "retriage_first_folder_ntuser_dat",
        &["Executable", "FolderName"],
    ),
    ("retriage_office_mru_ntuser_dat", &["FileName"]),
    // User Access Logging: who reached which role from where.
    (
        "sumetriage_clients",
        &[
            "AuthenticatedUserName",
            "IpAddress",
            "ClientName",
            "RoleDescription",
        ],
    ),
    ("sumetriage_dns_info", &["HostName", "Address"]),
    ("sumetriage_role_accesses", &["RoleDescription"]),
];

/// The relation whose entries `relation` takes: for RETriage's copy of a
/// dataset read from a hive's RegBack backup (`retriage_<dataset>_regback`),
/// the dataset it copies, which the same plugin wrote with the same columns;
/// for any other, `relation` itself.
fn base_relation(relation: &str) -> &str {
    match relation.strip_suffix("_regback") {
        Some(base) if base.starts_with("retriage_") => base,
        _ => relation,
    }
}

/// The `SUMMARY` entry for `relation`, if it has one.
pub(crate) fn summary_for(relation: &str) -> Option<&'static [&'static str]> {
    let relation = base_relation(relation);
    SUMMARY
        .iter()
        .find(|(r, _)| *r == relation)
        .map(|(_, c)| *c)
}

/// The `APP_TYPES` entries for `relation`, in order.
pub(crate) fn app_types_for(relation: &str) -> impl Iterator<Item = &'static AppType> + '_ {
    let relation = base_relation(relation);
    APP_TYPES.iter().filter(move |t| t.relation == relation)
}

/// The rule the app types `relation.column` by, if it does.
pub(crate) fn rule_for(relation: &str, column: &str) -> Option<AppRule> {
    app_types_for(relation)
        .find(|t| t.column == column)
        .map(|t| t.rule)
}

/// The `PRIMARY_TIME` entry for `relation`, if it has one.
pub(crate) fn primary_time_for(relation: &str) -> Option<&'static str> {
    let relation = base_relation(relation);
    PRIMARY_TIME
        .iter()
        .find(|(r, _)| *r == relation)
        .map(|(_, c)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_regback_copy_takes_the_entries_of_the_dataset_it_copies() {
        assert_eq!(
            base_relation("retriage_taskcache_software_regback"),
            "retriage_taskcache_software"
        );
        assert_eq!(
            primary_time_for("retriage_taskcache_software_regback"),
            Some("CreatedOn")
        );
        assert_eq!(
            rule_for("retriage_taskcache_software_regback", "LastStop"),
            Some(AppRule::UtcWhenTextEndsInZ)
        );
        // Only RETriage writes RegBack copies; another tool's name ending
        // the same way is its own relation.
        assert_eq!(base_relation("othertool_x_regback"), "othertool_x_regback");
        assert_eq!(base_relation("retriage_regback"), "retriage_regback");
    }

    #[test]
    fn no_entry_names_a_regback_copy_itself() {
        let names = APP_TYPES
            .iter()
            .map(|t| t.relation)
            .chain(PRIMARY_TIME.iter().map(|(r, _)| *r))
            .chain(SUMMARY.iter().map(|(r, _)| *r));
        for name in names {
            assert_eq!(base_relation(name), name, "{name} would never be looked up");
        }
    }
}
