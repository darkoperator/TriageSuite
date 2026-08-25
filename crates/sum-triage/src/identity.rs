//! Read SystemIdentity.mdb: the 3 SUMMARY datasets plus the cross-file
//! RoleGuid->RoleName map and the chained-database list.

use std::collections::HashMap;

use triage_ese::{Database, EseError};

use crate::datasets::{
    ChainedDbInfo, RoleInfo, SystemIdentInfo, CHAINED_DATABASES_TABLE, ROLE_IDS_TABLE,
    SYSTEM_IDENTITY_TABLE,
};
use crate::helpers::{filetime_to_wints, format_guid};

/// Everything the detail pass needs from SystemIdentity.mdb.
pub struct Identity {
    pub system_idents: Vec<SystemIdentInfo>,
    pub role_infos: Vec<RoleInfo>,
    pub chained: Vec<ChainedDbInfo>,
    /// RoleGuid (lowercase) -> RoleName, for DETAIL role-name joins.
    pub role_map: HashMap<String, String>,
}

/// Column index lookup by name.
fn indexer(db: &Database, table: &str) -> Option<HashMap<String, usize>> {
    let cols = db.columns(table).ok()?;
    Some(
        cols.into_iter()
            .enumerate()
            .map(|(i, c)| (c.name, i))
            .collect(),
    )
}

pub fn read(db: &Database) -> Result<Identity, EseError> {
    let mut system_idents = Vec::new();
    if db.table_exists(SYSTEM_IDENTITY_TABLE) {
        if let Some(ix) = indexer(db, SYSTEM_IDENTITY_TABLE) {
            let g = |n: &str| ix.get(n).copied();
            let (Some(ct), Some(maj), Some(min), Some(bld)) = (
                g("CreationTime"),
                g("OSMajor"),
                g("OSMinor"),
                g("OSBuildNumber"),
            ) else {
                return Err(EseError::Parse(
                    "SYSTEM_IDENTITY missing expected columns".into(),
                ));
            };
            for row in db.rows(SYSTEM_IDENTITY_TABLE)? {
                // CreationTime is stored as an ESE DateTime column (coltyp 12), so
                // the ESE layer already decodes it to a WinTimestamp. Older / other
                // SUM builds may store it as an Int64 FILETIME — fall back to that.
                let creation_time = match row[ct].as_timestamp() {
                    Some(ts) => *ts,
                    None => filetime_to_wints(row[ct].as_i64().unwrap_or(0)),
                };
                system_idents.push(SystemIdentInfo {
                    creation_time,
                    os_major: row[maj].as_i64().unwrap_or(0),
                    os_minor: row[min].as_i64().unwrap_or(0),
                    os_build: row[bld].as_i64().unwrap_or(0),
                });
            }
        }
    }

    let mut role_infos = Vec::new();
    let mut role_map = HashMap::new();
    if db.table_exists(ROLE_IDS_TABLE) {
        if let Some(ix) = indexer(db, ROLE_IDS_TABLE) {
            let g = |n: &str| ix.get(n).copied();
            if let (Some(rg), Some(rn), Some(pn)) = (g("RoleGuid"), g("RoleName"), g("ProductName"))
            {
                for row in db.rows(ROLE_IDS_TABLE)? {
                    let role_guid = format_guid(row[rg].as_bytes().unwrap_or_default());
                    let role_name = row[rn].as_text().unwrap_or_default().to_string();
                    let product_name = row[pn].as_text().unwrap_or_default().to_string();
                    if !role_guid.is_empty() {
                        role_map.insert(role_guid.clone(), role_name.clone());
                    }
                    role_infos.push(RoleInfo {
                        role_guid,
                        role_name,
                        product_name,
                    });
                }
            }
        }
    }

    let mut chained = Vec::new();
    if db.table_exists(CHAINED_DATABASES_TABLE) {
        if let Some(ix) = indexer(db, CHAINED_DATABASES_TABLE) {
            let g = |n: &str| ix.get(n).copied();
            if let (Some(yr), Some(fnm)) = (g("Year"), g("FileName")) {
                for row in db.rows(CHAINED_DATABASES_TABLE)? {
                    chained.push(ChainedDbInfo {
                        year: row[yr].as_i64().unwrap_or(0),
                        file_name: row[fnm].as_text().unwrap_or_default().to_string(),
                    });
                }
            }
        }
    }

    Ok(Identity {
        system_idents,
        role_infos,
        chained,
        role_map,
    })
}
