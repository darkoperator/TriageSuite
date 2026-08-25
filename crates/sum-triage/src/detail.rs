//! Read one Current/chained .mdb: CLIENTS (+ day expansion), DNS, ROLE_ACCESS,
//! VIRTUALMACHINES. Joins RoleGuid->RoleName from the identity map; stamps the
//! bare filename as SourceFile.

use std::collections::HashMap;

use triage_core::timestamp::WinTimestamp;
use triage_ese::{Database, EseValue};

use crate::datasets::{
    ClientDayDetail, ClientEntry, DnsEntry, RoleAccessEntry, VmEntry, CLIENTS_TABLE, DNS_TABLE,
    ROLE_ACCESS_TABLE, VIRTUALMACHINES_TABLE,
};
use crate::helpers::{bytes_to_ip, day_to_date, filetime_to_wints, format_guid};

pub const UNKNOWN_ROLE: &str = "(Unknown Role Guid)";

/// Read a timestamp cell robustly: a DateTime coltyp arrives as
/// `EseValue::DateTime`; an Int64 FILETIME arrives as `EseValue::Int`.
fn read_ts(v: &EseValue) -> WinTimestamp {
    if let Some(ts) = v.as_timestamp() {
        *ts
    } else {
        filetime_to_wints(v.as_i64().unwrap_or(0))
    }
}

/// All DETAIL rows from one database.
#[derive(Default)]
pub struct Detail {
    pub clients: Vec<ClientEntry>,
    pub clients_detailed: Vec<ClientDayDetail>,
    pub dns: Vec<DnsEntry>,
    pub role_accesses: Vec<RoleAccessEntry>,
    pub vms: Vec<VmEntry>,
}

fn indexer(db: &Database, table: &str) -> Option<HashMap<String, usize>> {
    let cols = db.columns(table).ok()?;
    Some(
        cols.into_iter()
            .enumerate()
            .map(|(i, c)| (c.name, i))
            .collect(),
    )
}

fn role_desc(role_map: &HashMap<String, String>, guid: &str) -> String {
    role_map
        .get(guid)
        .cloned()
        .unwrap_or_else(|| UNKNOWN_ROLE.to_string())
}

/// `year` is the calendar year for CLIENTS Day{n}->Date expansion (current year
/// for Current.mdb; CHAINED_DATABASES.Year for a chained DB). `source_file` is
/// the bare filename.
pub fn read(
    db: &Database,
    role_map: &HashMap<String, String>,
    year: i32,
    source_file: &str,
) -> Detail {
    let mut out = Detail::default();
    read_clients(db, role_map, year, source_file, &mut out);
    read_dns(db, source_file, &mut out);
    read_role_accesses(db, role_map, source_file, &mut out);
    read_vms(db, source_file, &mut out);
    out
}

fn read_clients(
    db: &Database,
    role_map: &HashMap<String, String>,
    year: i32,
    source_file: &str,
    out: &mut Detail,
) {
    if !db.table_exists(CLIENTS_TABLE) {
        return;
    }
    let Some(ix) = indexer(db, CLIENTS_TABLE) else {
        return;
    };
    let g = |n: &str| ix.get(n).copied();
    let Ok(rows) = db.rows(CLIENTS_TABLE) else {
        return;
    };

    // Pre-resolve Day1..Day366 indices once.
    let day_idx: Vec<(i64, usize)> = (1..=366)
        .filter_map(|n| g(&format!("Day{n}")).map(|i| (n, i)))
        .collect();

    for row in rows {
        let cell = |opt: Option<usize>| opt.map(|i| &row[i]);
        let role_guid = cell(g("RoleGuid"))
            .and_then(|v| v.as_bytes())
            .map(format_guid)
            .unwrap_or_default();
        let role_description = role_desc(role_map, &role_guid);
        let authenticated_user_name = cell(g("AuthenticatedUserName"))
            .and_then(|v| v.as_text())
            .unwrap_or_default()
            .to_string();
        let total_accesses = cell(g("TotalAccesses"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let insert_date = cell(g("InsertDate"))
            .map(read_ts)
            .unwrap_or_else(WinTimestamp::none);
        let last_access = cell(g("LastAccess"))
            .map(read_ts)
            .unwrap_or_else(WinTimestamp::none);
        let ip_address = cell(g("Address"))
            .and_then(|v| v.as_bytes())
            .map(bytes_to_ip)
            .unwrap_or_default();
        let client_name = cell(g("ClientName"))
            .and_then(|v| v.as_text())
            .unwrap_or_default()
            .to_string();
        let tenant_id = cell(g("TenantId"))
            .and_then(|v| v.as_bytes())
            .map(format_guid)
            .unwrap_or_default();

        out.clients.push(ClientEntry {
            role_guid: role_guid.clone(),
            role_description: role_description.clone(),
            authenticated_user_name: authenticated_user_name.clone(),
            total_accesses,
            insert_date,
            last_access,
            ip_address: ip_address.clone(),
            client_name: client_name.clone(),
            tenant_id: tenant_id.clone(),
            source_file: source_file.to_string(),
        });

        // Day expansion: one ClientsDetailed row per day column present (sparse
        // tagged column; absent days are Null/None and skipped).
        for (n, idx) in &day_idx {
            if let Some(count) = row[*idx].as_i64() {
                out.clients_detailed.push(ClientDayDetail {
                    date: day_to_date(year, *n),
                    count,
                    day_number: *n,
                    role_guid: role_guid.clone(),
                    role_description: role_description.clone(),
                    authenticated_user_name: authenticated_user_name.clone(),
                    total_accesses,
                    insert_date,
                    last_access,
                    ip_address: ip_address.clone(),
                    client_name: client_name.clone(),
                    tenant_id: tenant_id.clone(),
                    source_file: source_file.to_string(),
                });
            }
        }
    }
}

fn read_dns(db: &Database, source_file: &str, out: &mut Detail) {
    if !db.table_exists(DNS_TABLE) {
        return;
    }
    let Some(ix) = indexer(db, DNS_TABLE) else {
        return;
    };
    let g = |n: &str| ix.get(n).copied();
    let Ok(rows) = db.rows(DNS_TABLE) else { return };
    for row in rows {
        let cell = |opt: Option<usize>| opt.map(|i| &row[i]);
        out.dns.push(DnsEntry {
            host_name: cell(g("HostName"))
                .and_then(|v| v.as_text())
                .unwrap_or_default()
                .to_string(),
            address: cell(g("Address"))
                .and_then(|v| v.as_text())
                .unwrap_or_default()
                .to_string(),
            last_seen: cell(g("LastSeen"))
                .map(read_ts)
                .unwrap_or_else(WinTimestamp::none),
            source_file: source_file.to_string(),
        });
    }
}

fn read_role_accesses(
    db: &Database,
    role_map: &HashMap<String, String>,
    source_file: &str,
    out: &mut Detail,
) {
    if !db.table_exists(ROLE_ACCESS_TABLE) {
        return;
    }
    let Some(ix) = indexer(db, ROLE_ACCESS_TABLE) else {
        return;
    };
    let g = |n: &str| ix.get(n).copied();
    let Ok(rows) = db.rows(ROLE_ACCESS_TABLE) else {
        return;
    };
    for row in rows {
        let cell = |opt: Option<usize>| opt.map(|i| &row[i]);
        let role_guid = cell(g("RoleGuid"))
            .and_then(|v| v.as_bytes())
            .map(format_guid)
            .unwrap_or_default();
        out.role_accesses.push(RoleAccessEntry {
            role_description: role_desc(role_map, &role_guid),
            role_guid,
            first_seen: cell(g("FirstSeen"))
                .map(read_ts)
                .unwrap_or_else(WinTimestamp::none),
            last_seen: cell(g("LastSeen"))
                .map(read_ts)
                .unwrap_or_else(WinTimestamp::none),
            source_file: source_file.to_string(),
        });
    }
}

fn read_vms(db: &Database, source_file: &str, out: &mut Detail) {
    if !db.table_exists(VIRTUALMACHINES_TABLE) {
        return;
    }
    let Some(ix) = indexer(db, VIRTUALMACHINES_TABLE) else {
        return;
    };
    let g = |n: &str| ix.get(n).copied();
    let Ok(rows) = db.rows(VIRTUALMACHINES_TABLE) else {
        return;
    };
    for row in rows {
        let cell = |opt: Option<usize>| opt.map(|i| &row[i]);
        out.vms.push(VmEntry {
            serial_number: cell(g("SerialNumber"))
                .and_then(|v| v.as_text())
                .unwrap_or_default()
                .to_string(),
            creation_time: cell(g("CreationTime"))
                .map(read_ts)
                .unwrap_or_else(WinTimestamp::none),
            last_seen_active: cell(g("LastSeenActive"))
                .map(read_ts)
                .unwrap_or_else(WinTimestamp::none),
            bios_guid: cell(g("BIOSGuid"))
                .and_then(|v| v.as_bytes())
                .map(format_guid)
                .unwrap_or_default(),
            vm_guid: cell(g("VmGuid"))
                .and_then(|v| v.as_bytes())
                .map(format_guid)
                .unwrap_or_default(),
            source_file: source_file.to_string(),
        });
    }
}
