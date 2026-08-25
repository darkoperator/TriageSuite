//! Read each Timeline table into its record vector. Missing tables yield an
//! empty vector (caller skips the dataset). Id columns may arrive as BLOB
//! (.NET GUID) or TEXT (already a GUID string).

use triage_sqlite::{Database, SqliteValue};

use crate::appid::{decode_payload, executable_from_appid, executable_from_appid_operation};
use crate::decode::{activity_type_name, duration_str, epoch_ts, guid_from_blob};
use crate::record::{ActivityOperationRecord, ActivityRecord, PackageIdRecord, TitleCaseBool};

/// Render an Id cell: BLOB -> .NET GUID text; TEXT -> as-is; else empty.
fn id_text(v: &SqliteValue) -> String {
    match v {
        SqliteValue::Blob(b) => guid_from_blob(b).unwrap_or_default(),
        SqliteValue::Text(s) => s.clone(),
        _ => String::new(),
    }
}

/// ASCII-decode a payload BLOB/TEXT the way WxTCmd does (`Encoding.ASCII`).
fn ascii_payload(v: &SqliteValue) -> String {
    match v {
        SqliteValue::Blob(b) => b.iter().map(|&c| (c & 0x7f) as char).collect(),
        SqliteValue::Text(s) => s.clone(),
        _ => String::new(),
    }
}

/// ClipboardPayload: WxTCmd ASCII-decodes only when the blob is non-empty,
/// else empty string.
fn clip_or_empty(v: &SqliteValue) -> String {
    match v {
        SqliteValue::Blob(b) if !b.is_empty() => b.iter().map(|&c| (c & 0x7f) as char).collect(),
        SqliteValue::Text(s) => s.clone(),
        _ => String::new(),
    }
}

fn i64_of(v: &SqliteValue) -> i64 {
    v.as_i64().unwrap_or(0)
}

fn text_of(v: &SqliteValue) -> String {
    v.as_text().unwrap_or("").to_string()
}

pub fn read_activity(db: &Database) -> Result<Vec<ActivityRecord>, rusqlite::Error> {
    if !db.table_exists("Activity")? {
        return Ok(Vec::new());
    }
    let rows = db.query(
        "SELECT Id, AppId, ActivityType, Payload, ClipboardPayload, StartTime, EndTime, \
         LastModifiedTime, LastModifiedOnClient, OriginalLastModifiedOnClient, ExpirationTime, \
         CreatedInCloud, IsLocalOnly, ETag, PackageIdHash, PlatformDeviceId FROM Activity",
    )?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let app_id = text_of(&r[1]);
        let at = i64_of(&r[2]);
        let payload_raw = ascii_payload(&r[3]);
        let dp = decode_payload(&payload_raw);
        let start = i64_of(&r[5]);
        let end_raw = i64_of(&r[6]);
        let end_opt = if end_raw == 0 { None } else { Some(end_raw) };
        out.push(ActivityRecord {
            id: id_text(&r[0]),
            activity_type_org: at,
            activity_type: activity_type_name(at),
            executable: executable_from_appid(&app_id),
            display_text: dp.display_text,
            content_info: dp.content_info,
            payload: dp.rendered_payload,
            clipboard_payload: clip_or_empty(&r[4]),
            start_time: epoch_ts(start),
            end_time: epoch_ts(end_raw),
            duration: duration_str(start, end_opt),
            last_modified_time: epoch_ts(i64_of(&r[7])),
            last_modified_on_client: epoch_ts(i64_of(&r[8])),
            original_last_modified_on_client: epoch_ts(i64_of(&r[9])),
            expiration_time: epoch_ts(i64_of(&r[10])),
            created_in_cloud: epoch_ts(i64_of(&r[11])),
            is_local_only: TitleCaseBool(i64_of(&r[12]) == 1),
            etag: i64_of(&r[13]),
            package_id_hash: text_of(&r[14]),
            platform_device_id: text_of(&r[15]),
            device_platform: dp.device_platform,
            time_zone: dp.time_zone,
        });
    }
    Ok(out)
}

pub fn read_activity_operation(
    db: &Database,
) -> Result<Vec<ActivityOperationRecord>, rusqlite::Error> {
    if !db.table_exists("ActivityOperation")? {
        return Ok(Vec::new());
    }
    let rows = db.query(
        "SELECT Id, OperationOrder, OperationType, AppId, ActivityType, LastModifiedTime, \
         ExpirationTime, Payload, CreatedTime, EndTime, LastModifiedOnClient, \
         OperationExpirationTime, PlatformDeviceId, StartTime, ClipboardPayload FROM ActivityOperation",
    )?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let app_id = text_of(&r[3]);
        let at = i64_of(&r[4]);
        let payload_raw = ascii_payload(&r[7]);
        let dp = decode_payload(&payload_raw);
        let start = i64_of(&r[13]);
        let end_raw = i64_of(&r[9]);
        let end_opt = if end_raw == 0 { None } else { Some(end_raw) };
        out.push(ActivityOperationRecord {
            id: id_text(&r[0]),
            activity_type_org: at,
            activity_type: activity_type_name(at),
            executable: executable_from_appid_operation(&app_id),
            display_text: dp.display_text,
            content_info: dp.content_info,
            payload: dp.rendered_payload,
            clipboard_payload: clip_or_empty(&r[14]),
            start_time: epoch_ts(start),
            end_time: epoch_ts(end_raw),
            duration: duration_str(start, end_opt),
            last_modified_time: epoch_ts(i64_of(&r[5])),
            last_modified_time_on_client: epoch_ts(i64_of(&r[10])),
            created_time: epoch_ts(i64_of(&r[8])),
            expiration_time: epoch_ts(i64_of(&r[6])),
            operation_expiration_time: epoch_ts(i64_of(&r[11])),
            operation_order: i64_of(&r[1]),
            app_id,
            operation_type: i64_of(&r[2]),
            description: String::new(),
            platform_device_id: text_of(&r[12]),
            device_platform: dp.device_platform,
            time_zone: dp.time_zone,
        });
    }
    Ok(out)
}

pub fn read_package_id(db: &Database) -> Result<Vec<PackageIdRecord>, rusqlite::Error> {
    if !db.table_exists("Activity_PackageId")? {
        return Ok(Vec::new());
    }
    let rows = db.query(
        "SELECT ActivityId, Platform, PackageName, ExpirationTime FROM Activity_PackageId",
    )?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let platform_raw = text_of(&r[1]);
        let package_name = text_of(&r[2]);
        out.push(PackageIdRecord {
            id: id_text(&r[0]),
            platform: map_platform(&platform_raw),
            name: package_name.clone(),
            additional_information: additional_info(&package_name),
            expires: epoch_ts(i64_of(&r[3])),
        });
    }
    Ok(out)
}

/// WxTCmd's platform renaming for Activity_PackageId.
fn map_platform(p: &str) -> String {
    match p {
        "windows_win32" => "Win32".to_string(),
        "x_exe_path" => "ExecutablePath".to_string(),
        "packageId" => "Package".to_string(),
        other => other.to_string(),
    }
}

/// AdditionalInformation: WxTCmd derives the exe name only when PackageName
/// contains ".exe" AND its first `\`-segment is a brace GUID (mapped); else
/// empty.
fn additional_info(package_name: &str) -> String {
    if !package_name.contains(".exe") {
        return String::new();
    }
    let segs: Vec<&str> = package_name.split('\\').collect();
    if let Some(first) = segs.first() {
        if first.starts_with('{') {
            if let Some(desc) = triage_guidmap::description_for(first) {
                let mut owned: Vec<String> = segs.iter().map(|s| s.to_string()).collect();
                owned[0] = desc.to_string();
                return owned.join("\\");
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::path::Path;

    fn build_timeline(path: &Path) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE Activity(
                Id BLOB, AppId TEXT, ActivityType INTEGER, Payload BLOB, ClipboardPayload BLOB,
                StartTime INTEGER, EndTime INTEGER, LastModifiedTime INTEGER,
                LastModifiedOnClient INTEGER, OriginalLastModifiedOnClient INTEGER,
                ExpirationTime INTEGER, CreatedInCloud INTEGER, IsLocalOnly INTEGER, ETag INTEGER,
                PackageIdHash TEXT, PlatformDeviceId TEXT);",
        )
        .unwrap();
        // One row: Id = 16-byte guid blob, win32 exe, ActivityType=6 (InFocus).
        let guid = [
            0x00u8, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let app = r#"[{"Application":"C:\\app\\foo.exe","Platform":"windows_win32"}]"#;
        let payload = r#"{"displayText":"Hi","devicePlatform":"Windows","userTimezone":"UTC"}"#;
        c.execute(
            "INSERT INTO Activity VALUES (?1, ?2, 6, ?3, NULL, 1681999196, 1681999209, \
             1681999300, 1681999301, 0, 1690000000, 0, 1, 42, 'hash', 'devid')",
            rusqlite::params![guid.to_vec(), app, payload.as_bytes().to_vec()],
        )
        .unwrap();
    }

    #[test]
    fn reads_activity_row_with_all_derivations() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ActivitiesCache.db");
        build_timeline(&p);
        let db = Database::open(&p).unwrap();
        let recs = read_activity(&db).unwrap();
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.id, "33221100-5544-7766-8899-aabbccddeeff");
        assert_eq!(r.activity_type_org, 6);
        assert_eq!(r.activity_type, "InFocus");
        assert_eq!(r.executable, "C:\\app\\foo.exe");
        assert_eq!(r.display_text, "Hi");
        assert_eq!(r.duration, "00:00:13");
        assert_eq!(r.is_local_only, TitleCaseBool(true));
        assert_eq!(r.etag, 42);
        assert_eq!(r.device_platform, "Windows");
        assert_eq!(r.clipboard_payload, "");
    }

    #[test]
    fn missing_tables_yield_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ActivitiesCache.db");
        build_timeline(&p);
        let db = Database::open(&p).unwrap();
        assert!(read_activity_operation(&db).unwrap().is_empty());
        assert!(read_package_id(&db).unwrap().is_empty());
    }

    #[test]
    fn platform_mapping() {
        assert_eq!(map_platform("windows_win32"), "Win32");
        assert_eq!(map_platform("x_exe_path"), "ExecutablePath");
        assert_eq!(map_platform("packageId"), "Package");
        assert_eq!(map_platform("custom"), "custom");
    }
}
