//! The three WxTCmd output records. Field order = WxTCmd CSV column order;
//! `#[serde(rename)]` gives each column its exact header. Timestamps are
//! `WinTimestamp` (empty CSV cell / omitted JSON when unset).

use serde::Serialize;
use triage_core::timestamp::WinTimestamp;

/// A boolean that serializes as `"True"` / `"False"` (WxTCmd .NET convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitleCaseBool(pub bool);

impl Serialize for TitleCaseBool {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(if self.0 { "True" } else { "False" })
    }
}

/// `Activity` table -> 22 columns.
#[derive(Debug, Serialize)]
pub struct ActivityRecord {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "ActivityTypeOrg")]
    pub activity_type_org: i64,
    #[serde(rename = "ActivityType")]
    pub activity_type: String,
    #[serde(rename = "Executable")]
    pub executable: String,
    #[serde(rename = "DisplayText")]
    pub display_text: String,
    #[serde(rename = "ContentInfo")]
    pub content_info: String,
    #[serde(rename = "Payload")]
    pub payload: String,
    #[serde(rename = "ClipboardPayload")]
    pub clipboard_payload: String,
    #[serde(rename = "StartTime")]
    pub start_time: WinTimestamp,
    #[serde(rename = "EndTime")]
    pub end_time: WinTimestamp,
    #[serde(rename = "Duration")]
    pub duration: String,
    #[serde(rename = "LastModifiedTime")]
    pub last_modified_time: WinTimestamp,
    #[serde(rename = "LastModifiedOnClient")]
    pub last_modified_on_client: WinTimestamp,
    #[serde(rename = "OriginalLastModifiedOnClient")]
    pub original_last_modified_on_client: WinTimestamp,
    #[serde(rename = "ExpirationTime")]
    pub expiration_time: WinTimestamp,
    #[serde(rename = "CreatedInCloud")]
    pub created_in_cloud: WinTimestamp,
    #[serde(rename = "IsLocalOnly")]
    pub is_local_only: TitleCaseBool,
    #[serde(rename = "ETag")]
    pub etag: i64,
    #[serde(rename = "PackageIdHash")]
    pub package_id_hash: String,
    #[serde(rename = "PlatformDeviceId")]
    pub platform_device_id: String,
    #[serde(rename = "DevicePlatform")]
    pub device_platform: String,
    #[serde(rename = "TimeZone")]
    pub time_zone: String,
}

/// `ActivityOperation` table -> 23 columns.
#[derive(Debug, Serialize)]
pub struct ActivityOperationRecord {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "ActivityTypeOrg")]
    pub activity_type_org: i64,
    #[serde(rename = "ActivityType")]
    pub activity_type: String,
    #[serde(rename = "Executable")]
    pub executable: String,
    #[serde(rename = "DisplayText")]
    pub display_text: String,
    #[serde(rename = "ContentInfo")]
    pub content_info: String,
    #[serde(rename = "Payload")]
    pub payload: String,
    #[serde(rename = "ClipboardPayload")]
    pub clipboard_payload: String,
    #[serde(rename = "StartTime")]
    pub start_time: WinTimestamp,
    #[serde(rename = "EndTime")]
    pub end_time: WinTimestamp,
    #[serde(rename = "Duration")]
    pub duration: String,
    #[serde(rename = "LastModifiedTime")]
    pub last_modified_time: WinTimestamp,
    #[serde(rename = "LastModifiedTimeOnClient")]
    pub last_modified_time_on_client: WinTimestamp,
    #[serde(rename = "CreatedTime")]
    pub created_time: WinTimestamp,
    #[serde(rename = "ExpirationTime")]
    pub expiration_time: WinTimestamp,
    #[serde(rename = "OperationExpirationTime")]
    pub operation_expiration_time: WinTimestamp,
    #[serde(rename = "OperationOrder")]
    pub operation_order: i64,
    #[serde(rename = "AppId")]
    pub app_id: String,
    #[serde(rename = "OperationType")]
    pub operation_type: i64,
    #[serde(rename = "Description")]
    pub description: String,
    #[serde(rename = "PlatformDeviceId")]
    pub platform_device_id: String,
    #[serde(rename = "DevicePlatform")]
    pub device_platform: String,
    #[serde(rename = "TimeZone")]
    pub time_zone: String,
}

/// `Activity_PackageId` table -> 5 columns.
#[derive(Debug, Serialize)]
pub struct PackageIdRecord {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Platform")]
    pub platform: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "AdditionalInformation")]
    pub additional_information: String,
    #[serde(rename = "Expires")]
    pub expires: WinTimestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers<T: Serialize>(rec: &T) -> String {
        let mut wtr = csv::Writer::from_writer(vec![]);
        wtr.serialize(rec).unwrap();
        let data = String::from_utf8(wtr.into_inner().unwrap()).unwrap();
        data.lines().next().unwrap().to_string()
    }

    #[test]
    fn activity_headers_match_wxtcmd() {
        let r = ActivityRecord {
            id: String::new(),
            activity_type_org: 0,
            activity_type: String::new(),
            executable: String::new(),
            display_text: String::new(),
            content_info: String::new(),
            payload: String::new(),
            clipboard_payload: String::new(),
            start_time: WinTimestamp::none(),
            end_time: WinTimestamp::none(),
            duration: String::new(),
            last_modified_time: WinTimestamp::none(),
            last_modified_on_client: WinTimestamp::none(),
            original_last_modified_on_client: WinTimestamp::none(),
            expiration_time: WinTimestamp::none(),
            created_in_cloud: WinTimestamp::none(),
            is_local_only: TitleCaseBool(false),
            etag: 0,
            package_id_hash: String::new(),
            platform_device_id: String::new(),
            device_platform: String::new(),
            time_zone: String::new(),
        };
        assert_eq!(
            headers(&r),
            "Id,ActivityTypeOrg,ActivityType,Executable,DisplayText,ContentInfo,Payload,\
ClipboardPayload,StartTime,EndTime,Duration,LastModifiedTime,LastModifiedOnClient,\
OriginalLastModifiedOnClient,ExpirationTime,CreatedInCloud,IsLocalOnly,ETag,PackageIdHash,\
PlatformDeviceId,DevicePlatform,TimeZone"
        );
    }

    #[test]
    fn operation_headers_match_wxtcmd() {
        let r = ActivityOperationRecord {
            id: String::new(),
            activity_type_org: 0,
            activity_type: String::new(),
            executable: String::new(),
            display_text: String::new(),
            content_info: String::new(),
            payload: String::new(),
            clipboard_payload: String::new(),
            start_time: WinTimestamp::none(),
            end_time: WinTimestamp::none(),
            duration: String::new(),
            last_modified_time: WinTimestamp::none(),
            last_modified_time_on_client: WinTimestamp::none(),
            created_time: WinTimestamp::none(),
            expiration_time: WinTimestamp::none(),
            operation_expiration_time: WinTimestamp::none(),
            operation_order: 0,
            app_id: String::new(),
            operation_type: 0,
            description: String::new(),
            platform_device_id: String::new(),
            device_platform: String::new(),
            time_zone: String::new(),
        };
        assert_eq!(
            headers(&r),
            "Id,ActivityTypeOrg,ActivityType,Executable,DisplayText,ContentInfo,Payload,\
ClipboardPayload,StartTime,EndTime,Duration,LastModifiedTime,LastModifiedTimeOnClient,\
CreatedTime,ExpirationTime,OperationExpirationTime,OperationOrder,AppId,OperationType,\
Description,PlatformDeviceId,DevicePlatform,TimeZone"
        );
    }

    #[test]
    fn packageid_headers_match_wxtcmd() {
        let r = PackageIdRecord {
            id: String::new(),
            platform: String::new(),
            name: String::new(),
            additional_information: String::new(),
            expires: WinTimestamp::none(),
        };
        assert_eq!(
            headers(&r),
            "Id,Platform,Name,AdditionalInformation,Expires"
        );
    }
}
