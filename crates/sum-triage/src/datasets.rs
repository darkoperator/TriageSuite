//! SUM dataset record structs. Field ORDER == public-property DECLARATION ORDER
//! of the matching C# class in `SumData/Sum.cs` (CsvHelper AutoMap column order),
//! verified against the SumECmd oracle headers.
//!
//! All timestamps are `WinTimestamp` built from Int64 FILETIME via
//! `helpers::filetime_to_wints`. The compat test normalizes our fractional ISO
//! rendering against SumECmd's whole-second `yyyy-MM-dd HH:mm:ss`.

use serde::Serialize;
use triage_core::timestamp::WinTimestamp;

// ---- SUMMARY (SystemIdentity.mdb) ------------------------------------------

#[derive(Debug, Serialize)]
pub struct SystemIdentInfo {
    #[serde(rename = "CreationTime")]
    pub creation_time: WinTimestamp,
    #[serde(rename = "OsMajor")]
    pub os_major: i64,
    #[serde(rename = "OsMinor")]
    pub os_minor: i64,
    #[serde(rename = "OsBuild")]
    pub os_build: i64,
}

#[derive(Debug, Serialize)]
pub struct RoleInfo {
    #[serde(rename = "RoleGuid")]
    pub role_guid: String,
    #[serde(rename = "RoleName")]
    pub role_name: String,
    #[serde(rename = "ProductName")]
    pub product_name: String,
}

#[derive(Debug, Serialize)]
pub struct ChainedDbInfo {
    #[serde(rename = "Year")]
    pub year: i64,
    #[serde(rename = "FileName")]
    pub file_name: String,
}

// ---- DETAIL (Current.mdb + chained .mdb) -----------------------------------

#[derive(Debug, Serialize)]
pub struct ClientEntry {
    #[serde(rename = "RoleGuid")]
    pub role_guid: String,
    #[serde(rename = "RoleDescription")]
    pub role_description: String,
    #[serde(rename = "AuthenticatedUserName")]
    pub authenticated_user_name: String,
    #[serde(rename = "TotalAccesses")]
    pub total_accesses: i64,
    #[serde(rename = "InsertDate")]
    pub insert_date: WinTimestamp,
    #[serde(rename = "LastAccess")]
    pub last_access: WinTimestamp,
    #[serde(rename = "IpAddress")]
    pub ip_address: String,
    #[serde(rename = "ClientName")]
    pub client_name: String,
    #[serde(rename = "TenantId")]
    pub tenant_id: String,
    #[serde(rename = "SourceFile")]
    pub source_file: String,
}

#[derive(Debug, Serialize)]
pub struct ClientDayDetail {
    #[serde(rename = "Date")]
    pub date: String,
    #[serde(rename = "Count")]
    pub count: i64,
    #[serde(rename = "DayNumber")]
    pub day_number: i64,
    #[serde(rename = "RoleGuid")]
    pub role_guid: String,
    #[serde(rename = "RoleDescription")]
    pub role_description: String,
    #[serde(rename = "AuthenticatedUserName")]
    pub authenticated_user_name: String,
    #[serde(rename = "TotalAccesses")]
    pub total_accesses: i64,
    #[serde(rename = "InsertDate")]
    pub insert_date: WinTimestamp,
    #[serde(rename = "LastAccess")]
    pub last_access: WinTimestamp,
    #[serde(rename = "IpAddress")]
    pub ip_address: String,
    #[serde(rename = "ClientName")]
    pub client_name: String,
    #[serde(rename = "TenantId")]
    pub tenant_id: String,
    #[serde(rename = "SourceFile")]
    pub source_file: String,
}

#[derive(Debug, Serialize)]
pub struct DnsEntry {
    #[serde(rename = "HostName")]
    pub host_name: String,
    #[serde(rename = "Address")]
    pub address: String,
    #[serde(rename = "LastSeen")]
    pub last_seen: WinTimestamp,
    #[serde(rename = "SourceFile")]
    pub source_file: String,
}

#[derive(Debug, Serialize)]
pub struct RoleAccessEntry {
    #[serde(rename = "RoleGuid")]
    pub role_guid: String,
    #[serde(rename = "RoleDescription")]
    pub role_description: String,
    #[serde(rename = "FirstSeen")]
    pub first_seen: WinTimestamp,
    #[serde(rename = "LastSeen")]
    pub last_seen: WinTimestamp,
    #[serde(rename = "SourceFile")]
    pub source_file: String,
}

#[derive(Debug, Serialize)]
pub struct VmEntry {
    #[serde(rename = "SerialNumber")]
    pub serial_number: String,
    #[serde(rename = "CreationTime")]
    pub creation_time: WinTimestamp,
    #[serde(rename = "LastSeenActive")]
    pub last_seen_active: WinTimestamp,
    #[serde(rename = "BiosGuid")]
    pub bios_guid: String,
    #[serde(rename = "VmGuid")]
    pub vm_guid: String,
    #[serde(rename = "SourceFile")]
    pub source_file: String,
}

// `WinTimestamp` has no `Default`, so the two client structs hand-roll one
// (unset timestamps -> `WinTimestamp::none()`) rather than touch triage-core.
impl Default for ClientEntry {
    fn default() -> Self {
        Self {
            role_guid: String::new(),
            role_description: String::new(),
            authenticated_user_name: String::new(),
            total_accesses: 0,
            insert_date: WinTimestamp::none(),
            last_access: WinTimestamp::none(),
            ip_address: String::new(),
            client_name: String::new(),
            tenant_id: String::new(),
            source_file: String::new(),
        }
    }
}

impl Default for ClientDayDetail {
    fn default() -> Self {
        Self {
            date: String::new(),
            count: 0,
            day_number: 0,
            role_guid: String::new(),
            role_description: String::new(),
            authenticated_user_name: String::new(),
            total_accesses: 0,
            insert_date: WinTimestamp::none(),
            last_access: WinTimestamp::none(),
            ip_address: String::new(),
            client_name: String::new(),
            tenant_id: String::new(),
            source_file: String::new(),
        }
    }
}

// Table-name constants (C# table identifiers).
pub const SYSTEM_IDENTITY_TABLE: &str = "SYSTEM_IDENTITY";
pub const ROLE_IDS_TABLE: &str = "ROLE_IDS";
pub const CHAINED_DATABASES_TABLE: &str = "CHAINED_DATABASES";
pub const CLIENTS_TABLE: &str = "CLIENTS";
pub const DNS_TABLE: &str = "DNS";
pub const ROLE_ACCESS_TABLE: &str = "ROLE_ACCESS";
pub const VIRTUALMACHINES_TABLE: &str = "VIRTUALMACHINES";

#[cfg(test)]
mod tests {
    use super::*;
    use triage_core::timestamp::WinTimestamp;

    /// Serialize one record to a 1-row CSV and return the header line.
    fn header_of<T: serde::Serialize>(rec: &T) -> String {
        let mut w = csv::Writer::from_writer(vec![]);
        w.serialize(rec).unwrap();
        let data = String::from_utf8(w.into_inner().unwrap()).unwrap();
        data.lines().next().unwrap().to_string()
    }

    #[test]
    fn headers_match_sumecmd_order() {
        assert_eq!(
            header_of(&SystemIdentInfo {
                creation_time: WinTimestamp::none(),
                os_major: 0,
                os_minor: 0,
                os_build: 0
            }),
            "CreationTime,OsMajor,OsMinor,OsBuild"
        );
        assert_eq!(
            header_of(&RoleInfo {
                role_guid: String::new(),
                role_name: String::new(),
                product_name: String::new()
            }),
            "RoleGuid,RoleName,ProductName"
        );
        assert_eq!(
            header_of(&ChainedDbInfo {
                year: 0,
                file_name: String::new()
            }),
            "Year,FileName"
        );
        assert_eq!(
            header_of(&ClientEntry::default()),
            "RoleGuid,RoleDescription,AuthenticatedUserName,TotalAccesses,InsertDate,LastAccess,IpAddress,ClientName,TenantId,SourceFile"
        );
        assert_eq!(
            header_of(&ClientDayDetail::default()),
            "Date,Count,DayNumber,RoleGuid,RoleDescription,AuthenticatedUserName,TotalAccesses,InsertDate,LastAccess,IpAddress,ClientName,TenantId,SourceFile"
        );
        assert_eq!(
            header_of(&DnsEntry {
                host_name: String::new(),
                address: String::new(),
                last_seen: WinTimestamp::none(),
                source_file: String::new()
            }),
            "HostName,Address,LastSeen,SourceFile"
        );
        assert_eq!(
            header_of(&RoleAccessEntry {
                role_guid: String::new(),
                role_description: String::new(),
                first_seen: WinTimestamp::none(),
                last_seen: WinTimestamp::none(),
                source_file: String::new()
            }),
            "RoleGuid,RoleDescription,FirstSeen,LastSeen,SourceFile"
        );
        assert_eq!(
            header_of(&VmEntry {
                serial_number: String::new(),
                creation_time: WinTimestamp::none(),
                last_seen_active: WinTimestamp::none(),
                bios_guid: String::new(),
                vm_guid: String::new(),
                source_file: String::new()
            }),
            "SerialNumber,CreationTime,LastSeenActive,BiosGuid,VmGuid,SourceFile"
        );
    }
}
