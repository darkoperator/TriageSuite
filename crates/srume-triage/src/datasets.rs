//! SRUM dataset record structs and ESE table readers.
//!
//! Each struct's field ORDER matches the public-property DECLARATION ORDER of
//! the corresponding C# entry class in SrumECmd `Srum.cs`, which is the order
//! CsvHelper `AutoMap<T>()` emits columns.
//!
//! Timestamp format used by SrumECmd for the "simple" `Timestamp` column:
//!   `yyyy-MM-dd HH:mm:ss`  (the `--dt` default, passed as the `dt` argument
//!   to each `.Convert(t => t.Value.Timestamp:yyyy-MM-dd HH:mm:ss)` lambda in
//!   Program.cs lines ~468-629).
//! Other DateTimeOffset fields (ConnectStartTime, EventTimestamp, StartTime,
//! EndTime) use the user-supplied `dt` format string (same default).
//!
//! DONE_WITH_CONCERNS:
//! * `Vfuprov.Duration` is a computed C# property (EndTime - StartTime,
//!   returned as a TimeSpan).  CsvHelper AutoMap includes ALL public readable
//!   properties, so it WILL appear as a CSV column.  We serialize it as a
//!   formatted string `"HH:MM:SS"`.  The exact format SrumECmd emits for a
//!   `TimeSpan` via default CsvHelper serialisation is `d.hh:mm:ss` (e.g.
//!   "0.00:01:23") — this will be reconciled against fixtures in Task 9.
//! * `SidType` for each dataset is the C# `SidTypeEnum` variant NAME (e.g.
//!   "UnknownOrUserSid", "LocalSystem").  CsvHelper default ToString() on an
//!   enum is the variant name.
//! * `InterfaceType` is derived from the LUID via `GetInterfaceTypeFromLuid`
//!   (bytes 6-7 of the little-endian LUID).  We emit the enum variant NAME
//!   (e.g. "IF_TYPE_ETHERNET_CSMACD") or "Unknown" when the value has no
//!   known variant.

use std::collections::HashMap;

use serde::Serialize;
use triage_core::timestamp::WinTimestamp;
use triage_ese::Database;

use crate::idmap::{AppInfo, IdMaps};
use crate::sidtype::sid_type;

/// Serialize bool as `"True"` / `"False"` (CsvHelper .NET default).
fn serialize_bool_titlecase<S: serde::Serializer>(v: &bool, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(if *v { "True" } else { "False" })
}

// ---------------------------------------------------------------------------
// Helper: derive InterfaceType name from LUID (port of Srum.cs
// GetInterfaceTypeFromLuid).  The LUID is a little-endian i64; bytes 6-7
// hold the interface-type code.
// ---------------------------------------------------------------------------

fn interface_type_from_luid(luid: i64) -> String {
    let b = luid.to_le_bytes();
    let code = i16::from_le_bytes([b[6], b[7]]) as i32;
    match code {
        // 0 is not a named member of the InterfaceType enum; C# ToString() returns "0".
        0 => return "0".to_string(),
        1 => "IF_TYPE_OTHER",
        2 => "IF_TYPE_REGULAR_1822",
        3 => "IF_TYPE_HDH_1822",
        4 => "IF_TYPE_DDN_X25",
        5 => "IF_TYPE_RFC877_X25",
        6 => "IF_TYPE_ETHERNET_CSMACD",
        7 => "IF_TYPE_IS088023_CSMACD",
        8 => "IF_TYPE_ISO88024_TOKENBUS",
        9 => "IF_TYPE_ISO88025_TOKENRING",
        10 => "IF_TYPE_ISO88026_MAN",
        11 => "IF_TYPE_STARLAN",
        12 => "IF_TYPE_PROTEON_10MBIT",
        13 => "IF_TYPE_PROTEON_80MBIT",
        14 => "IF_TYPE_HYPERCHANNEL",
        15 => "IF_TYPE_FDDI",
        16 => "IF_TYPE_LAP_B",
        17 => "IF_TYPE_SDLC",
        18 => "IF_TYPE_DS1",
        19 => "IF_TYPE_E1",
        20 => "IF_TYPE_BASIC_ISDN",
        21 => "IF_TYPE_PRIMARY_ISDN",
        22 => "IF_TYPE_PROP_POINT2POINT_SERIAL",
        23 => "IF_TYPE_PPP",
        24 => "IF_TYPE_SOFTWARE_LOOPBACK",
        25 => "IF_TYPE_EON",
        26 => "IF_TYPE_ETHERNET_3MBIT",
        27 => "IF_TYPE_NSIP",
        28 => "IF_TYPE_SLIP",
        29 => "IF_TYPE_ULTRA",
        30 => "IF_TYPE_DS3",
        31 => "IF_TYPE_SIP",
        32 => "IF_TYPE_FRAMERELAY",
        33 => "IF_TYPE_RS232",
        34 => "IF_TYPE_PARA",
        35 => "IF_TYPE_ARCNET",
        36 => "IF_TYPE_ARCNET_PLUS",
        37 => "IF_TYPE_ATM",
        38 => "IF_TYPE_MIO_X25",
        39 => "IF_TYPE_SONET",
        40 => "IF_TYPE_X25_PLE",
        41 => "IF_TYPE_ISO88022_LLC",
        42 => "IF_TYPE_LOCALTALK",
        43 => "IF_TYPE_SMDS_DXI",
        44 => "IF_TYPE_FRAMERELAY_SERVICE",
        45 => "IF_TYPE_V35",
        46 => "IF_TYPE_HSSI",
        47 => "IF_TYPE_HIPPI",
        48 => "IF_TYPE_MODEM",
        49 => "IF_TYPE_AAL5",
        50 => "IF_TYPE_SONET_PATH",
        51 => "IF_TYPE_SONET_VT",
        52 => "IF_TYPE_SMDS_ICIP",
        53 => "IF_TYPE_PROP_VIRTUAL",
        54 => "IF_TYPE_PROP_MULTIPLEXOR",
        55 => "IF_TYPE_IEEE80212",
        56 => "IF_TYPE_FIBRECHANNEL",
        57 => "IF_TYPE_HIPPIINTERFACE",
        58 => "IF_TYPE_FRAMERELAY_INTERCONNECT",
        59 => "IF_TYPE_AFLANE_8023",
        60 => "IF_TYPE_AFLANE_8025",
        61 => "IF_TYPE_CCTEMUL",
        62 => "IF_TYPE_FASTETHER",
        63 => "IF_TYPE_ISDN",
        64 => "IF_TYPE_V11",
        65 => "IF_TYPE_V36",
        66 => "IF_TYPE_G703_64K",
        67 => "IF_TYPE_G703_2MB",
        68 => "IF_TYPE_QLLC",
        69 => "IF_TYPE_FASTETHER_FX",
        70 => "IF_TYPE_CHANNEL",
        71 => "IF_TYPE_IEEE80211",
        72 => "IF_TYPE_IBM370PARCHAN",
        73 => "IF_TYPE_ESCON",
        74 => "IF_TYPE_DLSW",
        75 => "IF_TYPE_ISDN_S",
        76 => "IF_TYPE_ISDN_U",
        77 => "IF_TYPE_LAP_D",
        78 => "IF_TYPE_IPSWITCH",
        79 => "IF_TYPE_RSRB",
        80 => "IF_TYPE_ATM_LOGICAL",
        81 => "IF_TYPE_DS0",
        82 => "IF_TYPE_DS0_BUNDLE",
        83 => "IF_TYPE_BSC",
        84 => "IF_TYPE_ASYNC",
        85 => "IF_TYPE_CNR",
        86 => "IF_TYPE_ISO88025R_DTR",
        87 => "IF_TYPE_EPLRS",
        88 => "IF_TYPE_ARAP",
        89 => "IF_TYPE_PROP_CNLS",
        90 => "IF_TYPE_HOSTPAD",
        91 => "IF_TYPE_TERMPAD",
        92 => "IF_TYPE_FRAMERELAY_MPI",
        93 => "IF_TYPE_X213",
        94 => "IF_TYPE_ADSL",
        95 => "IF_TYPE_RADSL",
        96 => "IF_TYPE_SDSL",
        97 => "IF_TYPE_VDSL",
        98 => "IF_TYPE_ISO88025_CRFPRINT",
        99 => "IF_TYPE_MYRINET",
        100 => "IF_TYPE_VOICE_EM",
        101 => "IF_TYPE_VOICE_FXO",
        102 => "IF_TYPE_VOICE_FXS",
        103 => "IF_TYPE_VOICE_ENCAP",
        104 => "IF_TYPE_VOICE_OVERIP",
        105 => "IF_TYPE_ATM_DXI",
        106 => "IF_TYPE_ATM_FUNI",
        107 => "IF_TYPE_ATM_IMA",
        108 => "IF_TYPE_PPPMULTILINKBUNDLE",
        109 => "IF_TYPE_IPOVER_CDLC",
        110 => "IF_TYPE_IPOVER_CLAW",
        111 => "IF_TYPE_STACKTOSTACK",
        112 => "IF_TYPE_VIRTUALIPADDRESS",
        113 => "IF_TYPE_MPC",
        114 => "IF_TYPE_IPOVER_ATM",
        115 => "IF_TYPE_ISO88025_FIBER",
        116 => "IF_TYPE_TDLC",
        117 => "IF_TYPE_GIGABITETHERNET",
        118 => "IF_TYPE_HDLC",
        119 => "IF_TYPE_LAP_F",
        120 => "IF_TYPE_V37",
        121 => "IF_TYPE_X25_MLP",
        122 => "IF_TYPE_X25_HUNTGROUP",
        123 => "IF_TYPE_TRANSPHDLC",
        124 => "IF_TYPE_INTERLEAVE",
        125 => "IF_TYPE_FAST",
        126 => "IF_TYPE_IP",
        127 => "IF_TYPE_DOCSCABLE_MACLAYER",
        128 => "IF_TYPE_DOCSCABLE_DOWNSTREAM",
        129 => "IF_TYPE_DOCSCABLE_UPSTREAM",
        130 => "IF_TYPE_A12MPPSWITCH",
        131 => "IF_TYPE_TUNNEL",
        132 => "IF_TYPE_COFFEE",
        133 => "IF_TYPE_CES",
        134 => "IF_TYPE_ATM_SUBINTERFACE",
        135 => "IF_TYPE_L2_VLAN",
        136 => "IF_TYPE_L3_IPVLAN",
        137 => "IF_TYPE_L3_IPXVLAN",
        138 => "IF_TYPE_DIGITALPOWERLINE",
        139 => "IF_TYPE_MEDIAMAILOVERIP",
        140 => "IF_TYPE_DTM",
        141 => "IF_TYPE_DCN",
        142 => "IF_TYPE_IPFORWARD",
        143 => "IF_TYPE_MSDSL",
        144 => "IF_TYPE_IEEE1394",
        145 => "IF_TYPE_RECEIVE_ONLY",
        // For any code not in the enum, C# InterfaceType.ToString() returns the
        // integer value as a string (not "Unknown").
        _ => return code.to_string(),
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// Helper: resolve app fields from IdMaps.
// ---------------------------------------------------------------------------

fn app_fields(maps: &IdMaps, app_id: i64) -> (String, String, WinTimestamp) {
    match maps.apps.get(&app_id) {
        Some(AppInfo {
            exe_info,
            exe_info_description,
            exe_timestamp,
        }) => {
            let ts = exe_timestamp
                .as_ref()
                .and_then(|s| {
                    // exe_timestamp is already an ISO-8601 string from idmap.rs
                    // Convert to WinTimestamp via chrono parse.
                    chrono::DateTime::parse_from_rfc3339(s)
                        .ok()
                        .map(|dt| WinTimestamp::from_unix(dt.timestamp()))
                })
                .unwrap_or_else(WinTimestamp::none);
            (exe_info.clone(), exe_info_description.clone(), ts)
        }
        None => (String::new(), String::new(), WinTimestamp::none()),
    }
}

/// Convert a Windows FILETIME (i64 100ns ticks since 1601-01-01) to WinTimestamp.
fn filetime_to_wints(ft: i64) -> WinTimestamp {
    if ft <= 0 {
        WinTimestamp::none()
    } else {
        WinTimestamp::from_filetime(ft as u64)
    }
}

// ---------------------------------------------------------------------------
// 1. NetworkUsage  {973F5D5C-1D90-4944-BE8E-24B94231A174}
//    Property declaration order from Srum.cs NetworkUsage class:
//    Id, Timestamp, ExeInfo, ExeInfoDescription, ExeTimestamp, SidType, Sid,
//    UserName, UserId, AppId, BytesReceived, BytesSent, InterfaceLuid,
//    InterfaceType, L2ProfileFlags, L2ProfileId, ProfileName
// ---------------------------------------------------------------------------

pub const NETWORK_USAGE_TABLE: &str = "{973F5D5C-1D90-4944-BE8E-24B94231A174}";

#[derive(Debug, Serialize)]
pub struct NetworkUsageRecord {
    #[serde(rename = "Id")]
    pub id: i64,
    #[serde(rename = "Timestamp")]
    pub timestamp: WinTimestamp,
    #[serde(rename = "ExeInfo")]
    pub exe_info: String,
    #[serde(rename = "ExeInfoDescription")]
    pub exe_info_description: String,
    #[serde(rename = "ExeTimestamp")]
    pub exe_timestamp: WinTimestamp,
    #[serde(rename = "SidType")]
    pub sid_type: String,
    #[serde(rename = "Sid")]
    pub sid: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "UserId")]
    pub user_id: i64,
    #[serde(rename = "AppId")]
    pub app_id: i64,
    #[serde(rename = "BytesReceived")]
    pub bytes_received: i64,
    #[serde(rename = "BytesSent")]
    pub bytes_sent: i64,
    #[serde(rename = "InterfaceLuid")]
    pub interface_luid: i64,
    #[serde(rename = "InterfaceType")]
    pub interface_type: String,
    #[serde(rename = "L2ProfileFlags")]
    pub l2_profile_flags: i64,
    #[serde(rename = "L2ProfileId")]
    pub l2_profile_id: i64,
    #[serde(rename = "ProfileName")]
    pub profile_name: String,
}

pub fn read_network_usage(
    db: &Database,
    maps: &IdMaps,
    users: &HashMap<String, String>,
) -> Vec<NetworkUsageRecord> {
    let Ok(cols) = db.columns(NETWORK_USAGE_TABLE) else {
        return vec![];
    };
    let pos = |n: &str| cols.iter().position(|c| c.name == n);
    let Some(idx_id) = pos("AutoIncId") else {
        return vec![];
    };
    let Some(idx_ts) = pos("TimeStamp") else {
        return vec![];
    };
    let Some(idx_app) = pos("AppId") else {
        return vec![];
    };
    let Some(idx_user) = pos("UserId") else {
        return vec![];
    };
    let Some(idx_br) = pos("BytesRecvd") else {
        return vec![];
    };
    let Some(idx_bs) = pos("BytesSent") else {
        return vec![];
    };
    let Some(idx_luid) = pos("InterfaceLuid") else {
        return vec![];
    };
    let Some(idx_pf) = pos("L2ProfileFlags") else {
        return vec![];
    };
    let Some(idx_pid) = pos("L2ProfileId") else {
        return vec![];
    };

    let Ok(rows) = db.rows(NETWORK_USAGE_TABLE) else {
        return vec![];
    };
    let mut out = Vec::with_capacity(rows.len());

    for row in rows {
        let id = row[idx_id].as_i64().unwrap_or(0);
        let timestamp = row[idx_ts]
            .as_timestamp()
            .copied()
            .unwrap_or_else(WinTimestamp::none);
        let app_id = row[idx_app].as_i64().unwrap_or(0);
        let user_id = row[idx_user].as_i64().unwrap_or(0);
        let bytes_received = row[idx_br].as_i64().unwrap_or(0);
        let bytes_sent = row[idx_bs].as_i64().unwrap_or(0);
        let interface_luid = row[idx_luid].as_i64().unwrap_or(0);
        let l2_profile_flags = row[idx_pf].as_i64().unwrap_or(0);
        let l2_profile_id = row[idx_pid].as_i64().unwrap_or(0);

        let (exe_info, exe_info_description, exe_timestamp) = app_fields(maps, app_id);
        let sid = maps.sids.get(&user_id).cloned().unwrap_or_default();
        let user_name = users.get(&sid).cloned().unwrap_or_default();
        let interface_type = interface_type_from_luid(interface_luid);
        let sid_type_val = sid_type(&sid);

        out.push(NetworkUsageRecord {
            id,
            timestamp,
            exe_info,
            exe_info_description,
            exe_timestamp,
            sid_type: sid_type_val,
            sid,
            user_name,
            user_id,
            app_id,
            bytes_received,
            bytes_sent,
            interface_luid,
            interface_type,
            l2_profile_flags,
            l2_profile_id,
            profile_name: String::new(), // ProfileName from L2ProfileId map (not available without SOFTWARE hive network profiles)
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 2. NetworkConnection  {DD6636C4-8929-4683-974E-22C046A43763}
//    Property declaration order:
//    Id, Timestamp, ExeInfo, ExeInfoDescription, ExeTimestamp, SidType, Sid,
//    UserName, UserId, AppId, ConnectedTime, ConnectStartTime, InterfaceLuid,
//    InterfaceType, L2ProfileFlags, L2ProfileId, ProfileName
// ---------------------------------------------------------------------------

pub const NETWORK_CONNECTION_TABLE: &str = "{DD6636C4-8929-4683-974E-22C046A43763}";

#[derive(Debug, Serialize)]
pub struct NetworkConnectionRecord {
    #[serde(rename = "Id")]
    pub id: i64,
    #[serde(rename = "Timestamp")]
    pub timestamp: WinTimestamp,
    #[serde(rename = "ExeInfo")]
    pub exe_info: String,
    #[serde(rename = "ExeInfoDescription")]
    pub exe_info_description: String,
    #[serde(rename = "ExeTimestamp")]
    pub exe_timestamp: WinTimestamp,
    #[serde(rename = "SidType")]
    pub sid_type: String,
    #[serde(rename = "Sid")]
    pub sid: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "UserId")]
    pub user_id: i64,
    #[serde(rename = "AppId")]
    pub app_id: i64,
    #[serde(rename = "ConnectedTime")]
    pub connected_time: i64,
    #[serde(rename = "ConnectStartTime")]
    pub connect_start_time: WinTimestamp,
    #[serde(rename = "InterfaceLuid")]
    pub interface_luid: i64,
    #[serde(rename = "InterfaceType")]
    pub interface_type: String,
    #[serde(rename = "L2ProfileFlags")]
    pub l2_profile_flags: i64,
    #[serde(rename = "L2ProfileId")]
    pub l2_profile_id: i64,
    #[serde(rename = "ProfileName")]
    pub profile_name: String,
}

pub fn read_network_connection(
    db: &Database,
    maps: &IdMaps,
    users: &HashMap<String, String>,
) -> Vec<NetworkConnectionRecord> {
    let Ok(cols) = db.columns(NETWORK_CONNECTION_TABLE) else {
        return vec![];
    };
    let pos = |n: &str| cols.iter().position(|c| c.name == n);
    let Some(idx_id) = pos("AutoIncId") else {
        return vec![];
    };
    let Some(idx_ts) = pos("TimeStamp") else {
        return vec![];
    };
    let Some(idx_app) = pos("AppId") else {
        return vec![];
    };
    let Some(idx_user) = pos("UserId") else {
        return vec![];
    };
    let Some(idx_ct) = pos("ConnectedTime") else {
        return vec![];
    };
    let Some(idx_cst) = pos("ConnectStartTime") else {
        return vec![];
    };
    let Some(idx_luid) = pos("InterfaceLuid") else {
        return vec![];
    };
    let Some(idx_pf) = pos("L2ProfileFlags") else {
        return vec![];
    };
    let Some(idx_pid) = pos("L2ProfileId") else {
        return vec![];
    };

    let Ok(rows) = db.rows(NETWORK_CONNECTION_TABLE) else {
        return vec![];
    };
    let mut out = Vec::with_capacity(rows.len());

    for row in rows {
        let id = row[idx_id].as_i64().unwrap_or(0);
        let timestamp = row[idx_ts]
            .as_timestamp()
            .copied()
            .unwrap_or_else(WinTimestamp::none);
        let app_id = row[idx_app].as_i64().unwrap_or(0);
        let user_id = row[idx_user].as_i64().unwrap_or(0);
        let connected_time = row[idx_ct].as_i64().unwrap_or(0);
        let connect_start_filetime = row[idx_cst].as_i64().unwrap_or(0);
        let interface_luid = row[idx_luid].as_i64().unwrap_or(0);
        let l2_profile_flags = row[idx_pf].as_i64().unwrap_or(0);
        let l2_profile_id = row[idx_pid].as_i64().unwrap_or(0);

        let (exe_info, exe_info_description, exe_timestamp) = app_fields(maps, app_id);
        let sid = maps.sids.get(&user_id).cloned().unwrap_or_default();
        let user_name = users.get(&sid).cloned().unwrap_or_default();
        let interface_type = interface_type_from_luid(interface_luid);
        let connect_start_time = filetime_to_wints(connect_start_filetime);
        let sid_type_val = sid_type(&sid);

        out.push(NetworkConnectionRecord {
            id,
            timestamp,
            exe_info,
            exe_info_description,
            exe_timestamp,
            sid_type: sid_type_val,
            sid,
            user_name,
            user_id,
            app_id,
            connected_time,
            connect_start_time,
            interface_luid,
            interface_type,
            l2_profile_flags,
            l2_profile_id,
            profile_name: String::new(),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 3. AppResourceUseInfo  {D10CA2FE-6FCF-4F6D-848E-B2E99266FA89}
//    Property declaration order:
//    Id, Timestamp, ExeInfo, ExeInfoDescription, ExeTimestamp, SidType, Sid,
//    UserName, UserId, AppId,
//    BackgroundBytesRead, BackgroundBytesWritten, BackgroundContextSwitches,
//    BackgroundCycleTime, BackgroundNumberOfFlushes, BackgroundNumReadOperations,
//    BackgroundNumWriteOperations, FaceTime, ForegroundBytesRead,
//    ForegroundBytesWritten, ForegroundContextSwitches, ForegroundCycleTime,
//    ForegroundNumberOfFlushes, ForegroundNumReadOperations,
//    ForegroundNumWriteOperations
// ---------------------------------------------------------------------------

pub const APP_RESOURCE_USE_TABLE: &str = "{D10CA2FE-6FCF-4F6D-848E-B2E99266FA89}";

#[derive(Debug, Serialize)]
pub struct AppResourceUseRecord {
    #[serde(rename = "Id")]
    pub id: i64,
    #[serde(rename = "Timestamp")]
    pub timestamp: WinTimestamp,
    #[serde(rename = "ExeInfo")]
    pub exe_info: String,
    #[serde(rename = "ExeInfoDescription")]
    pub exe_info_description: String,
    #[serde(rename = "ExeTimestamp")]
    pub exe_timestamp: WinTimestamp,
    #[serde(rename = "SidType")]
    pub sid_type: String,
    #[serde(rename = "Sid")]
    pub sid: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "UserId")]
    pub user_id: i64,
    #[serde(rename = "AppId")]
    pub app_id: i64,
    #[serde(rename = "BackgroundBytesRead")]
    pub background_bytes_read: i64,
    #[serde(rename = "BackgroundBytesWritten")]
    pub background_bytes_written: i64,
    #[serde(rename = "BackgroundContextSwitches")]
    pub background_context_switches: i64,
    #[serde(rename = "BackgroundCycleTime")]
    pub background_cycle_time: i64,
    #[serde(rename = "BackgroundNumberOfFlushes")]
    pub background_number_of_flushes: i64,
    #[serde(rename = "BackgroundNumReadOperations")]
    pub background_num_read_operations: i64,
    #[serde(rename = "BackgroundNumWriteOperations")]
    pub background_num_write_operations: i64,
    #[serde(rename = "FaceTime")]
    pub face_time: i64,
    #[serde(rename = "ForegroundBytesRead")]
    pub foreground_bytes_read: i64,
    #[serde(rename = "ForegroundBytesWritten")]
    pub foreground_bytes_written: i64,
    #[serde(rename = "ForegroundContextSwitches")]
    pub foreground_context_switches: i64,
    #[serde(rename = "ForegroundCycleTime")]
    pub foreground_cycle_time: i64,
    #[serde(rename = "ForegroundNumberOfFlushes")]
    pub foreground_number_of_flushes: i64,
    #[serde(rename = "ForegroundNumReadOperations")]
    pub foreground_num_read_operations: i64,
    #[serde(rename = "ForegroundNumWriteOperations")]
    pub foreground_num_write_operations: i64,
}

pub fn read_app_resource_use(
    db: &Database,
    maps: &IdMaps,
    users: &HashMap<String, String>,
) -> Vec<AppResourceUseRecord> {
    let Ok(cols) = db.columns(APP_RESOURCE_USE_TABLE) else {
        return vec![];
    };
    let pos = |n: &str| cols.iter().position(|c| c.name == n);
    let Some(idx_id) = pos("AutoIncId") else {
        return vec![];
    };
    let Some(idx_ts) = pos("TimeStamp") else {
        return vec![];
    };
    let Some(idx_app) = pos("AppId") else {
        return vec![];
    };
    let Some(idx_user) = pos("UserId") else {
        return vec![];
    };
    let Some(idx_bbr) = pos("BackgroundBytesRead") else {
        return vec![];
    };
    let Some(idx_bbw) = pos("BackgroundBytesWritten") else {
        return vec![];
    };
    let Some(idx_bcs) = pos("BackgroundContextSwitches") else {
        return vec![];
    };
    let Some(idx_bct) = pos("BackgroundCycleTime") else {
        return vec![];
    };
    let Some(idx_bnf) = pos("BackgroundNumberOfFlushes") else {
        return vec![];
    };
    let Some(idx_bro) = pos("BackgroundNumReadOperations") else {
        return vec![];
    };
    let Some(idx_bwo) = pos("BackgroundNumWriteOperations") else {
        return vec![];
    };
    let Some(idx_ft) = pos("FaceTime") else {
        return vec![];
    };
    let Some(idx_fbr) = pos("ForegroundBytesRead") else {
        return vec![];
    };
    let Some(idx_fbw) = pos("ForegroundBytesWritten") else {
        return vec![];
    };
    let Some(idx_fcs) = pos("ForegroundContextSwitches") else {
        return vec![];
    };
    let Some(idx_fct) = pos("ForegroundCycleTime") else {
        return vec![];
    };
    let Some(idx_fnf) = pos("ForegroundNumberOfFlushes") else {
        return vec![];
    };
    let Some(idx_fro) = pos("ForegroundNumReadOperations") else {
        return vec![];
    };
    let Some(idx_fwo) = pos("ForegroundNumWriteOperations") else {
        return vec![];
    };

    let Ok(rows) = db.rows(APP_RESOURCE_USE_TABLE) else {
        return vec![];
    };
    let mut out = Vec::with_capacity(rows.len());

    for row in rows {
        let id = row[idx_id].as_i64().unwrap_or(0);
        let timestamp = row[idx_ts]
            .as_timestamp()
            .copied()
            .unwrap_or_else(WinTimestamp::none);
        let app_id = row[idx_app].as_i64().unwrap_or(0);
        let user_id = row[idx_user].as_i64().unwrap_or(0);

        let (exe_info, exe_info_description, exe_timestamp) = app_fields(maps, app_id);
        let sid = maps.sids.get(&user_id).cloned().unwrap_or_default();
        let user_name = users.get(&sid).cloned().unwrap_or_default();
        let sid_type_val = sid_type(&sid);

        out.push(AppResourceUseRecord {
            id,
            timestamp,
            exe_info,
            exe_info_description,
            exe_timestamp,
            sid_type: sid_type_val,
            sid,
            user_name,
            user_id,
            app_id,
            background_bytes_read: row[idx_bbr].as_i64().unwrap_or(0),
            background_bytes_written: row[idx_bbw].as_i64().unwrap_or(0),
            background_context_switches: row[idx_bcs].as_i64().unwrap_or(0),
            background_cycle_time: row[idx_bct].as_i64().unwrap_or(0),
            background_number_of_flushes: row[idx_bnf].as_i64().unwrap_or(0),
            background_num_read_operations: row[idx_bro].as_i64().unwrap_or(0),
            background_num_write_operations: row[idx_bwo].as_i64().unwrap_or(0),
            face_time: row[idx_ft].as_i64().unwrap_or(0),
            foreground_bytes_read: row[idx_fbr].as_i64().unwrap_or(0),
            foreground_bytes_written: row[idx_fbw].as_i64().unwrap_or(0),
            foreground_context_switches: row[idx_fcs].as_i64().unwrap_or(0),
            foreground_cycle_time: row[idx_fct].as_i64().unwrap_or(0),
            foreground_number_of_flushes: row[idx_fnf].as_i64().unwrap_or(0),
            foreground_num_read_operations: row[idx_fro].as_i64().unwrap_or(0),
            foreground_num_write_operations: row[idx_fwo].as_i64().unwrap_or(0),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 4. PushNotification  {D10CA2FE-6FCF-4F6D-848E-B2E99266FA86}
//    Property declaration order:
//    Id, Timestamp, ExeInfo, ExeInfoDescription, ExeTimestamp, SidType, Sid,
//    UserName, UserId, AppId, NetworkType, NotificationType, PayloadSize
// ---------------------------------------------------------------------------

pub const PUSH_NOTIFICATION_TABLE: &str = "{D10CA2FE-6FCF-4F6D-848E-B2E99266FA86}";

#[derive(Debug, Serialize)]
pub struct PushNotificationRecord {
    #[serde(rename = "Id")]
    pub id: i64,
    #[serde(rename = "Timestamp")]
    pub timestamp: WinTimestamp,
    #[serde(rename = "ExeInfo")]
    pub exe_info: String,
    #[serde(rename = "ExeInfoDescription")]
    pub exe_info_description: String,
    #[serde(rename = "ExeTimestamp")]
    pub exe_timestamp: WinTimestamp,
    #[serde(rename = "SidType")]
    pub sid_type: String,
    #[serde(rename = "Sid")]
    pub sid: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "UserId")]
    pub user_id: i64,
    #[serde(rename = "AppId")]
    pub app_id: i64,
    #[serde(rename = "NetworkType")]
    pub network_type: i64,
    #[serde(rename = "NotificationType")]
    pub notification_type: i64,
    #[serde(rename = "PayloadSize")]
    pub payload_size: i64,
}

pub fn read_push_notification(
    db: &Database,
    maps: &IdMaps,
    users: &HashMap<String, String>,
) -> Vec<PushNotificationRecord> {
    let Ok(cols) = db.columns(PUSH_NOTIFICATION_TABLE) else {
        return vec![];
    };
    let pos = |n: &str| cols.iter().position(|c| c.name == n);
    let Some(idx_id) = pos("AutoIncId") else {
        return vec![];
    };
    let Some(idx_ts) = pos("TimeStamp") else {
        return vec![];
    };
    let Some(idx_app) = pos("AppId") else {
        return vec![];
    };
    let Some(idx_user) = pos("UserId") else {
        return vec![];
    };
    let Some(idx_nt) = pos("NetworkType") else {
        return vec![];
    };
    let Some(idx_not) = pos("NotificationType") else {
        return vec![];
    };
    let Some(idx_ps) = pos("PayloadSize") else {
        return vec![];
    };

    let Ok(rows) = db.rows(PUSH_NOTIFICATION_TABLE) else {
        return vec![];
    };
    let mut out = Vec::with_capacity(rows.len());

    for row in rows {
        let id = row[idx_id].as_i64().unwrap_or(0);
        let timestamp = row[idx_ts]
            .as_timestamp()
            .copied()
            .unwrap_or_else(WinTimestamp::none);
        let app_id = row[idx_app].as_i64().unwrap_or(0);
        let user_id = row[idx_user].as_i64().unwrap_or(0);

        let (exe_info, exe_info_description, exe_timestamp) = app_fields(maps, app_id);
        let sid = maps.sids.get(&user_id).cloned().unwrap_or_default();
        let user_name = users.get(&sid).cloned().unwrap_or_default();
        let sid_type_val = sid_type(&sid);

        out.push(PushNotificationRecord {
            id,
            timestamp,
            exe_info,
            exe_info_description,
            exe_timestamp,
            sid_type: sid_type_val,
            sid,
            user_name,
            user_id,
            app_id,
            network_type: row[idx_nt].as_i64().unwrap_or(0),
            notification_type: row[idx_not].as_i64().unwrap_or(0),
            payload_size: row[idx_ps].as_i64().unwrap_or(0),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 5. EnergyUsage  {FEE4E14F-02A9-4550-B5CE-5FA2DA202E37} (base) +
//                 {FEE4E14F-02A9-4550-B5CE-5FA2DA202E37}LT
//    Property declaration order:
//    Id, Timestamp, ExeInfo, ExeInfoDescription, ExeTimestamp, SidType, Sid,
//    UserName, UserId, AppId, IsLt, ConfigurationHash, EventTimestamp,
//    StateTransition, ChargeLevel, CycleCount, DesignedCapacity,
//    FullChargedCapacity, ActiveAcTime, ActiveDcTime, ActiveDischargeTime,
//    ActiveEnergy, CsAcTime, CsDcTime, CsDischargeTime, CsEnergy
//
//    LT table lacks: EventTimestamp, StateTransition, ChargeLevel (→ -1)
//    Base table lacks: CsAcTime, CsDcTime, CsDischargeTime, CsEnergy (→ -1)
// ---------------------------------------------------------------------------

pub const ENERGY_USAGE_TABLE: &str = "{FEE4E14F-02A9-4550-B5CE-5FA2DA202E37}";
pub const ENERGY_USAGE_LT_TABLE: &str = "{FEE4E14F-02A9-4550-B5CE-5FA2DA202E37}LT";

#[derive(Debug, Serialize)]
pub struct EnergyUsageRecord {
    #[serde(rename = "Id")]
    pub id: i64,
    #[serde(rename = "Timestamp")]
    pub timestamp: WinTimestamp,
    #[serde(rename = "ExeInfo")]
    pub exe_info: String,
    #[serde(rename = "ExeInfoDescription")]
    pub exe_info_description: String,
    #[serde(rename = "ExeTimestamp")]
    pub exe_timestamp: WinTimestamp,
    #[serde(rename = "SidType")]
    pub sid_type: String,
    #[serde(rename = "Sid")]
    pub sid: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "UserId")]
    pub user_id: i64,
    #[serde(rename = "AppId")]
    pub app_id: i64,
    #[serde(rename = "IsLt", serialize_with = "serialize_bool_titlecase")]
    pub is_lt: bool,
    #[serde(rename = "ConfigurationHash")]
    pub configuration_hash: i64,
    #[serde(rename = "EventTimestamp")]
    pub event_timestamp: WinTimestamp,
    #[serde(rename = "StateTransition")]
    pub state_transition: i64,
    #[serde(rename = "ChargeLevel")]
    pub charge_level: i64,
    #[serde(rename = "CycleCount")]
    pub cycle_count: i64,
    #[serde(rename = "DesignedCapacity")]
    pub designed_capacity: i64,
    #[serde(rename = "FullChargedCapacity")]
    pub full_charged_capacity: i64,
    #[serde(rename = "ActiveAcTime")]
    pub active_ac_time: i64,
    #[serde(rename = "ActiveDcTime")]
    pub active_dc_time: i64,
    #[serde(rename = "ActiveDischargeTime")]
    pub active_discharge_time: i64,
    #[serde(rename = "ActiveEnergy")]
    pub active_energy: i64,
    #[serde(rename = "CsAcTime")]
    pub cs_ac_time: i64,
    #[serde(rename = "CsDcTime")]
    pub cs_dc_time: i64,
    #[serde(rename = "CsDischargeTime")]
    pub cs_discharge_time: i64,
    #[serde(rename = "CsEnergy")]
    pub cs_energy: i64,
}

pub fn read_energy_usage(
    db: &Database,
    maps: &IdMaps,
    users: &HashMap<String, String>,
) -> Vec<EnergyUsageRecord> {
    let mut out = Vec::new();

    // --- LT table (isLt = true) ---
    if let Ok(cols) = db.columns(ENERGY_USAGE_LT_TABLE) {
        let pos = |n: &str| cols.iter().position(|c| c.name == n);
        if let (
            Some(idx_id),
            Some(idx_ts),
            Some(idx_app),
            Some(idx_user),
            Some(idx_cfg),
            Some(idx_cc),
            Some(idx_dc),
            Some(idx_fcc),
            Some(idx_aat),
            Some(idx_adt),
            Some(idx_adct),
            Some(idx_ae),
            Some(idx_csat),
            Some(idx_csdt),
            Some(idx_csdct),
            Some(idx_cse),
        ) = (
            pos("AutoIncId"),
            pos("TimeStamp"),
            pos("AppId"),
            pos("UserId"),
            pos("ConfigurationHash"),
            pos("CycleCount"),
            pos("DesignedCapacity"),
            pos("FullChargedCapacity"),
            pos("ActiveAcTime"),
            pos("ActiveDcTime"),
            pos("ActiveDischargeTime"),
            pos("ActiveEnergy"),
            pos("CsAcTime"),
            pos("CsDcTime"),
            pos("CsDischargeTime"),
            pos("CsEnergy"),
        ) {
            if let Ok(rows) = db.rows(ENERGY_USAGE_LT_TABLE) {
                for row in rows {
                    let id = row[idx_id].as_i64().unwrap_or(0);
                    let timestamp = row[idx_ts]
                        .as_timestamp()
                        .copied()
                        .unwrap_or_else(WinTimestamp::none);
                    let app_id = row[idx_app].as_i64().unwrap_or(0);
                    let user_id = row[idx_user].as_i64().unwrap_or(0);
                    let (exe_info, exe_info_description, exe_timestamp) = app_fields(maps, app_id);
                    let sid = maps.sids.get(&user_id).cloned().unwrap_or_default();
                    let user_name = users.get(&sid).cloned().unwrap_or_default();
                    let sid_type_val = sid_type(&sid);

                    out.push(EnergyUsageRecord {
                        id,
                        timestamp,
                        exe_info,
                        exe_info_description,
                        exe_timestamp,
                        sid_type: sid_type_val,
                        sid,
                        user_name,
                        user_id,
                        app_id,
                        is_lt: true,
                        configuration_hash: row[idx_cfg].as_i64().unwrap_or(0),
                        event_timestamp: WinTimestamp::none(), // absent in LT table
                        state_transition: -1,
                        charge_level: -1,
                        cycle_count: row[idx_cc].as_i64().unwrap_or(0),
                        designed_capacity: row[idx_dc].as_i64().unwrap_or(0),
                        full_charged_capacity: row[idx_fcc].as_i64().unwrap_or(0),
                        active_ac_time: row[idx_aat].as_i64().unwrap_or(0),
                        active_dc_time: row[idx_adt].as_i64().unwrap_or(0),
                        active_discharge_time: row[idx_adct].as_i64().unwrap_or(0),
                        active_energy: row[idx_ae].as_i64().unwrap_or(0),
                        cs_ac_time: row[idx_csat].as_i64().unwrap_or(0),
                        cs_dc_time: row[idx_csdt].as_i64().unwrap_or(0),
                        cs_discharge_time: row[idx_csdct].as_i64().unwrap_or(0),
                        cs_energy: row[idx_cse].as_i64().unwrap_or(0),
                    });
                }
            }
        }
    }

    // Track IDs already used by LT rows for collision avoidance (mirrors Srum.cs).
    let mut used_ids: std::collections::HashSet<i64> = out.iter().map(|r| r.id).collect();
    let mut last_seen: i64 = out.iter().map(|r| r.id).max().unwrap_or(0);

    // --- Base table (isLt = false) ---
    if let Ok(cols) = db.columns(ENERGY_USAGE_TABLE) {
        let pos = |n: &str| cols.iter().position(|c| c.name == n);
        if let (
            Some(idx_id),
            Some(idx_ts),
            Some(idx_app),
            Some(idx_user),
            Some(idx_cfg),
            Some(idx_et),
            Some(idx_st),
            Some(idx_cl),
            Some(idx_cc),
            Some(idx_dc),
            Some(idx_fcc),
        ) = (
            pos("AutoIncId"),
            pos("TimeStamp"),
            pos("AppId"),
            pos("UserId"),
            pos("ConfigurationHash"),
            pos("EventTimestamp"),
            pos("StateTransition"),
            pos("ChargeLevel"),
            pos("CycleCount"),
            pos("DesignedCapacity"),
            pos("FullChargedCapacity"),
        ) {
            if let Ok(rows) = db.rows(ENERGY_USAGE_TABLE) {
                for row in rows {
                    let mut id = row[idx_id].as_i64().unwrap_or(0);
                    // Collision: increment last_seen until a free ID is found.
                    if used_ids.contains(&id) {
                        while used_ids.contains(&last_seen) {
                            last_seen += 1;
                        }
                        id += last_seen;
                    }
                    used_ids.insert(id);

                    let timestamp = row[idx_ts]
                        .as_timestamp()
                        .copied()
                        .unwrap_or_else(WinTimestamp::none);
                    let app_id = row[idx_app].as_i64().unwrap_or(0);
                    let user_id = row[idx_user].as_i64().unwrap_or(0);
                    let event_filetime = row[idx_et].as_i64().unwrap_or(-1);
                    let event_timestamp = if event_filetime > 0 {
                        filetime_to_wints(event_filetime)
                    } else {
                        WinTimestamp::none()
                    };

                    let (exe_info, exe_info_description, exe_timestamp) = app_fields(maps, app_id);
                    let sid = maps.sids.get(&user_id).cloned().unwrap_or_default();
                    let user_name = users.get(&sid).cloned().unwrap_or_default();
                    let sid_type_val = sid_type(&sid);

                    out.push(EnergyUsageRecord {
                        id,
                        timestamp,
                        exe_info,
                        exe_info_description,
                        exe_timestamp,
                        sid_type: sid_type_val,
                        sid,
                        user_name,
                        user_id,
                        app_id,
                        is_lt: false,
                        configuration_hash: row[idx_cfg].as_i64().unwrap_or(0),
                        event_timestamp,
                        state_transition: row[idx_st].as_i64().unwrap_or(-1),
                        charge_level: row[idx_cl].as_i64().unwrap_or(-1),
                        cycle_count: row[idx_cc].as_i64().unwrap_or(0),
                        designed_capacity: row[idx_dc].as_i64().unwrap_or(0),
                        full_charged_capacity: row[idx_fcc].as_i64().unwrap_or(0),
                        active_ac_time: -1,
                        active_dc_time: -1,
                        active_discharge_time: -1,
                        active_energy: -1,
                        cs_ac_time: -1,
                        cs_dc_time: -1,
                        cs_discharge_time: -1,
                        cs_energy: -1,
                    });
                }
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// 6. TimelineProvider  {5C8CF1C7-7257-4F13-B223-970EF5939312}
//    Property declaration order:
//    Id, Timestamp, ExeInfo, ExeInfoDescription, ExeTimestamp, SidType, Sid,
//    UserName, UserId, AppId, EndTime, DurationMs
// ---------------------------------------------------------------------------

pub const TIMELINE_PROVIDER_TABLE: &str = "{5C8CF1C7-7257-4F13-B223-970EF5939312}";

#[derive(Debug, Serialize)]
pub struct TimelineProviderRecord {
    #[serde(rename = "Id")]
    pub id: i64,
    #[serde(rename = "Timestamp")]
    pub timestamp: WinTimestamp,
    #[serde(rename = "ExeInfo")]
    pub exe_info: String,
    #[serde(rename = "ExeInfoDescription")]
    pub exe_info_description: String,
    #[serde(rename = "ExeTimestamp")]
    pub exe_timestamp: WinTimestamp,
    #[serde(rename = "SidType")]
    pub sid_type: String,
    #[serde(rename = "Sid")]
    pub sid: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "UserId")]
    pub user_id: i64,
    #[serde(rename = "AppId")]
    pub app_id: i64,
    #[serde(rename = "EndTime")]
    pub end_time: WinTimestamp,
    #[serde(rename = "DurationMs")]
    pub duration_ms: i64,
}

pub fn read_timeline_provider(
    db: &Database,
    maps: &IdMaps,
    users: &HashMap<String, String>,
) -> Vec<TimelineProviderRecord> {
    let Ok(cols) = db.columns(TIMELINE_PROVIDER_TABLE) else {
        return vec![];
    };
    let pos = |n: &str| cols.iter().position(|c| c.name == n);
    let Some(idx_id) = pos("AutoIncId") else {
        return vec![];
    };
    let Some(idx_ts) = pos("TimeStamp") else {
        return vec![];
    };
    let Some(idx_app) = pos("AppId") else {
        return vec![];
    };
    let Some(idx_user) = pos("UserId") else {
        return vec![];
    };
    let Some(idx_dur) = pos("DurationMS") else {
        return vec![];
    };
    let Some(idx_et) = pos("EndTime") else {
        return vec![];
    };

    let Ok(rows) = db.rows(TIMELINE_PROVIDER_TABLE) else {
        return vec![];
    };
    let mut out = Vec::with_capacity(rows.len());

    for row in rows {
        let id = row[idx_id].as_i64().unwrap_or(0);
        let timestamp = row[idx_ts]
            .as_timestamp()
            .copied()
            .unwrap_or_else(WinTimestamp::none);
        let app_id = row[idx_app].as_i64().unwrap_or(0);
        let user_id = row[idx_user].as_i64().unwrap_or(0);
        let duration_ms = row[idx_dur].as_i64().unwrap_or(0);
        let end_filetime = row[idx_et].as_i64().unwrap_or(0);
        let end_time = filetime_to_wints(end_filetime);

        let (exe_info, exe_info_description, exe_timestamp) = app_fields(maps, app_id);
        let sid = maps.sids.get(&user_id).cloned().unwrap_or_default();
        let user_name = users.get(&sid).cloned().unwrap_or_default();
        let sid_type_val = sid_type(&sid);

        out.push(TimelineProviderRecord {
            id,
            timestamp,
            exe_info,
            exe_info_description,
            exe_timestamp,
            sid_type: sid_type_val,
            sid,
            user_name,
            user_id,
            app_id,
            end_time,
            duration_ms,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 7. Vfuprov  {7ACBBAA3-D029-4BE4-9A7A-0885927F1D8F}
//    Property declaration order (Srum.cs Vfuprov class):
//    Id, Timestamp, UserId, AppId,
//    ExeInfo, ExeInfoDescription, ExeTimestamp, SidType, Sid, UserName,
//    StartTime, EndTime, Flags, Duration
//
//    NOTE: `Duration` is a computed property (EndTime - StartTime as TimeSpan).
//    CsvHelper AutoMap includes all public readable properties.
//    DONE_WITH_CONCERNS: .NET TimeSpan default serialisation format is not
//    well-specified; we emit "d.HH:mm:ss" (e.g. "0.00:01:23") to approximate
//    it.  Task 9 fixtures will reconcile.
// ---------------------------------------------------------------------------

pub const VFUPROV_TABLE: &str = "{7ACBBAA3-D029-4BE4-9A7A-0885927F1D8F}";

#[derive(Debug, Serialize)]
pub struct VfuprovRecord {
    #[serde(rename = "Id")]
    pub id: i64,
    #[serde(rename = "Timestamp")]
    pub timestamp: WinTimestamp,
    #[serde(rename = "UserId")]
    pub user_id: i64,
    #[serde(rename = "AppId")]
    pub app_id: i64,
    #[serde(rename = "ExeInfo")]
    pub exe_info: String,
    #[serde(rename = "ExeInfoDescription")]
    pub exe_info_description: String,
    #[serde(rename = "ExeTimestamp")]
    pub exe_timestamp: WinTimestamp,
    #[serde(rename = "SidType")]
    pub sid_type: String,
    #[serde(rename = "Sid")]
    pub sid: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "StartTime")]
    pub start_time: WinTimestamp,
    #[serde(rename = "EndTime")]
    pub end_time: WinTimestamp,
    #[serde(rename = "Flags")]
    pub flags: i64,
    /// Computed: EndTime - StartTime, formatted as "d.HH:mm:ss".
    /// DONE_WITH_CONCERNS: exact .NET TimeSpan serialisation format TBD in Task 9.
    #[serde(rename = "Duration")]
    pub duration: String,
}

pub fn read_vfuprov(
    db: &Database,
    maps: &IdMaps,
    users: &HashMap<String, String>,
) -> Vec<VfuprovRecord> {
    let Ok(cols) = db.columns(VFUPROV_TABLE) else {
        return vec![];
    };
    let pos = |n: &str| cols.iter().position(|c| c.name == n);
    let Some(idx_id) = pos("AutoIncId") else {
        return vec![];
    };
    let Some(idx_ts) = pos("TimeStamp") else {
        return vec![];
    };
    let Some(idx_app) = pos("AppId") else {
        return vec![];
    };
    let Some(idx_user) = pos("UserId") else {
        return vec![];
    };
    let Some(idx_flags) = pos("Flags") else {
        return vec![];
    };
    let Some(idx_start) = pos("StartTime") else {
        return vec![];
    };
    let Some(idx_end) = pos("EndTime") else {
        return vec![];
    };

    let Ok(rows) = db.rows(VFUPROV_TABLE) else {
        return vec![];
    };
    let mut out = Vec::with_capacity(rows.len());

    for row in rows {
        let id = row[idx_id].as_i64().unwrap_or(0);
        let timestamp = row[idx_ts]
            .as_timestamp()
            .copied()
            .unwrap_or_else(WinTimestamp::none);
        let app_id = row[idx_app].as_i64().unwrap_or(0);
        let user_id = row[idx_user].as_i64().unwrap_or(0);
        let flags = row[idx_flags].as_i64().unwrap_or(0);
        let start_filetime = row[idx_start].as_i64().unwrap_or(0);
        let end_filetime = row[idx_end].as_i64().unwrap_or(0);
        let start_time = filetime_to_wints(start_filetime);
        let end_time = filetime_to_wints(end_filetime);

        // Compute duration from filetimes (100ns ticks; 1 tick = 100ns).
        let duration = compute_duration(start_filetime, end_filetime);

        let (exe_info, exe_info_description, exe_timestamp) = app_fields(maps, app_id);
        let sid = maps.sids.get(&user_id).cloned().unwrap_or_default();
        let user_name = users.get(&sid).cloned().unwrap_or_default();
        let sid_type_val = sid_type(&sid);

        out.push(VfuprovRecord {
            id,
            timestamp,
            user_id,
            app_id,
            exe_info,
            exe_info_description,
            exe_timestamp,
            sid_type: sid_type_val,
            sid,
            user_name,
            start_time,
            end_time,
            flags,
            duration,
        });
    }
    out
}

/// Format a duration from two Windows FILETIME i64 values (100ns ticks) as
/// .NET `TimeSpan` constant ("c") format:
///   - No days segment when days == 0: `[‑]hh:mm:ss` or `[‑]hh:mm:ss.fffffff`
///   - Days segment when |days| > 0: `[‑]d.hh:mm:ss[.fffffff]`
///   - 7-digit fractional seconds only when the sub-second part is non-zero.
///   - Negative TimeSpan: prefix with `-` (when end < start).
fn compute_duration(start_ft: i64, end_ft: i64) -> String {
    if start_ft <= 0 || end_ft <= 0 {
        return String::new();
    }
    let raw_ticks = end_ft - start_ft; // 100ns ticks, may be negative
    let (negative, ticks) = if raw_ticks < 0 {
        (true, -raw_ticks)
    } else {
        (false, raw_ticks)
    };
    let sign = if negative { "-" } else { "" };
    let frac_ticks = ticks % 10_000_000; // sub-second 100ns ticks (0..9_999_999)
    let total_secs = ticks / 10_000_000;
    let days = total_secs / 86400;
    let rem = total_secs % 86400;
    let hours = rem / 3600;
    let mins = (rem % 3600) / 60;
    let secs = rem % 60;
    if frac_ticks == 0 {
        if days == 0 {
            format!("{sign}{hours:02}:{mins:02}:{secs:02}")
        } else {
            format!("{sign}{days}.{hours:02}:{mins:02}:{secs:02}")
        }
    } else if days == 0 {
        format!("{sign}{hours:02}:{mins:02}:{secs:02}.{frac_ticks:07}")
    } else {
        format!("{sign}{days}.{hours:02}:{mins:02}:{secs:02}.{frac_ticks:07}")
    }
}

// ---------------------------------------------------------------------------
// Tests: assert serde-serialized CSV header equals declaration-order header.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    fn headers<T: Serialize>(rec: &T) -> String {
        let mut wtr = csv::Writer::from_writer(vec![]);
        wtr.serialize(rec).unwrap();
        let data = String::from_utf8(wtr.into_inner().unwrap()).unwrap();
        data.lines().next().unwrap().to_string()
    }

    fn dummy_ts() -> WinTimestamp {
        WinTimestamp::none()
    }

    #[test]
    fn network_usage_headers() {
        let r = NetworkUsageRecord {
            id: 0,
            timestamp: dummy_ts(),
            exe_info: String::new(),
            exe_info_description: String::new(),
            exe_timestamp: dummy_ts(),
            sid_type: String::new(),
            sid: String::new(),
            user_name: String::new(),
            user_id: 0,
            app_id: 0,
            bytes_received: 0,
            bytes_sent: 0,
            interface_luid: 0,
            interface_type: String::new(),
            l2_profile_flags: 0,
            l2_profile_id: 0,
            profile_name: String::new(),
        };
        assert_eq!(
            headers(&r),
            "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,\
UserId,AppId,BytesReceived,BytesSent,InterfaceLuid,InterfaceType,L2ProfileFlags,\
L2ProfileId,ProfileName"
        );
    }

    #[test]
    fn network_connection_headers() {
        let r = NetworkConnectionRecord {
            id: 0,
            timestamp: dummy_ts(),
            exe_info: String::new(),
            exe_info_description: String::new(),
            exe_timestamp: dummy_ts(),
            sid_type: String::new(),
            sid: String::new(),
            user_name: String::new(),
            user_id: 0,
            app_id: 0,
            connected_time: 0,
            connect_start_time: dummy_ts(),
            interface_luid: 0,
            interface_type: String::new(),
            l2_profile_flags: 0,
            l2_profile_id: 0,
            profile_name: String::new(),
        };
        assert_eq!(
            headers(&r),
            "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,\
UserId,AppId,ConnectedTime,ConnectStartTime,InterfaceLuid,InterfaceType,L2ProfileFlags,\
L2ProfileId,ProfileName"
        );
    }

    #[test]
    fn app_resource_use_headers() {
        let r = AppResourceUseRecord {
            id: 0,
            timestamp: dummy_ts(),
            exe_info: String::new(),
            exe_info_description: String::new(),
            exe_timestamp: dummy_ts(),
            sid_type: String::new(),
            sid: String::new(),
            user_name: String::new(),
            user_id: 0,
            app_id: 0,
            background_bytes_read: 0,
            background_bytes_written: 0,
            background_context_switches: 0,
            background_cycle_time: 0,
            background_number_of_flushes: 0,
            background_num_read_operations: 0,
            background_num_write_operations: 0,
            face_time: 0,
            foreground_bytes_read: 0,
            foreground_bytes_written: 0,
            foreground_context_switches: 0,
            foreground_cycle_time: 0,
            foreground_number_of_flushes: 0,
            foreground_num_read_operations: 0,
            foreground_num_write_operations: 0,
        };
        assert_eq!(
            headers(&r),
            "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,\
UserId,AppId,BackgroundBytesRead,BackgroundBytesWritten,BackgroundContextSwitches,\
BackgroundCycleTime,BackgroundNumberOfFlushes,BackgroundNumReadOperations,\
BackgroundNumWriteOperations,FaceTime,ForegroundBytesRead,ForegroundBytesWritten,\
ForegroundContextSwitches,ForegroundCycleTime,ForegroundNumberOfFlushes,\
ForegroundNumReadOperations,ForegroundNumWriteOperations"
        );
    }

    #[test]
    fn push_notification_headers() {
        let r = PushNotificationRecord {
            id: 0,
            timestamp: dummy_ts(),
            exe_info: String::new(),
            exe_info_description: String::new(),
            exe_timestamp: dummy_ts(),
            sid_type: String::new(),
            sid: String::new(),
            user_name: String::new(),
            user_id: 0,
            app_id: 0,
            network_type: 0,
            notification_type: 0,
            payload_size: 0,
        };
        assert_eq!(
            headers(&r),
            "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,\
UserId,AppId,NetworkType,NotificationType,PayloadSize"
        );
    }

    #[test]
    fn energy_usage_headers() {
        let r = EnergyUsageRecord {
            id: 0,
            timestamp: dummy_ts(),
            exe_info: String::new(),
            exe_info_description: String::new(),
            exe_timestamp: dummy_ts(),
            sid_type: String::new(),
            sid: String::new(),
            user_name: String::new(),
            user_id: 0,
            app_id: 0,
            is_lt: false,
            configuration_hash: 0,
            event_timestamp: dummy_ts(),
            state_transition: 0,
            charge_level: 0,
            cycle_count: 0,
            designed_capacity: 0,
            full_charged_capacity: 0,
            active_ac_time: 0,
            active_dc_time: 0,
            active_discharge_time: 0,
            active_energy: 0,
            cs_ac_time: 0,
            cs_dc_time: 0,
            cs_discharge_time: 0,
            cs_energy: 0,
        };
        assert_eq!(
            headers(&r),
            "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,\
UserId,AppId,IsLt,ConfigurationHash,EventTimestamp,StateTransition,ChargeLevel,\
CycleCount,DesignedCapacity,FullChargedCapacity,ActiveAcTime,ActiveDcTime,\
ActiveDischargeTime,ActiveEnergy,CsAcTime,CsDcTime,CsDischargeTime,CsEnergy"
        );
    }

    #[test]
    fn timeline_provider_headers() {
        let r = TimelineProviderRecord {
            id: 0,
            timestamp: dummy_ts(),
            exe_info: String::new(),
            exe_info_description: String::new(),
            exe_timestamp: dummy_ts(),
            sid_type: String::new(),
            sid: String::new(),
            user_name: String::new(),
            user_id: 0,
            app_id: 0,
            end_time: dummy_ts(),
            duration_ms: 0,
        };
        assert_eq!(
            headers(&r),
            "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,\
UserId,AppId,EndTime,DurationMs"
        );
    }

    #[test]
    fn vfuprov_headers() {
        let r = VfuprovRecord {
            id: 0,
            timestamp: dummy_ts(),
            user_id: 0,
            app_id: 0,
            exe_info: String::new(),
            exe_info_description: String::new(),
            exe_timestamp: dummy_ts(),
            sid_type: String::new(),
            sid: String::new(),
            user_name: String::new(),
            start_time: dummy_ts(),
            end_time: dummy_ts(),
            flags: 0,
            duration: String::new(),
        };
        assert_eq!(
            headers(&r),
            "Id,Timestamp,UserId,AppId,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,\
Sid,UserName,StartTime,EndTime,Flags,Duration"
        );
    }
}
