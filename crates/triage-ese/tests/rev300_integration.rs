//! M8-B: prove the patched ese_parser_lib + triage-ese reads rev-300 ESE files.
//! Cross-checks the production engine against the spike-proven ground truth
//! (itself bit-for-bit validated vs SumECmd). Gated on local evidence.

use std::path::{Path, PathBuf};
use triage_ese::db::Database;

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

/// Find a file by exact name anywhere under the captures whose path contains `marker`.
fn find_named(name: &str, marker: &str) -> Option<PathBuf> {
    let mut stack = vec![captures_root()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().and_then(|n| n.to_str()) == Some(name)
                && p.to_string_lossy().contains(marker)
            {
                return Some(p);
            }
        }
    }
    None
}

fn col_index(db: &Database, table: &str, col: &str) -> Option<usize> {
    db.columns(table).ok()?.iter().position(|c| c.name == col)
}

#[test]
fn reads_rev300_system_identity() {
    let Some(p) = find_named("SystemIdentity.mdb", "Collection-STDC1") else {
        return;
    };
    let db = Database::open(&p).expect("rev-300 SystemIdentity.mdb must open after the mask");

    let tables = db.tables().unwrap();
    assert!(
        tables.iter().any(|t| t == "SYSTEM_IDENTITY"),
        "tables: {tables:?}"
    );
    assert!(tables.iter().any(|t| t == "ROLE_IDS"), "tables: {tables:?}");

    let rows = db.rows("SYSTEM_IDENTITY").unwrap();
    let build_i = col_index(&db, "SYSTEM_IDENTITY", "OSBuildNumber").unwrap();
    let host_i = col_index(&db, "SYSTEM_IDENTITY", "SystemDNSHostName").unwrap();
    assert!(
        rows.iter().any(|r| r[build_i].as_i64() == Some(26100)),
        "expected an OSBuildNumber=26100 row"
    );
    assert!(
        rows.iter().any(|r| r[host_i].as_text() == Some("STDC1")),
        "expected SystemDNSHostName=STDC1"
    );

    let role_rows = db.rows("ROLE_IDS").unwrap();
    // Exactly 13 roles — the spike matched SumECmd's RoleInfos output 13/13 (ground truth).
    assert_eq!(role_rows.len(), 13, "ROLE_IDS row count");
    let name_i = col_index(&db, "ROLE_IDS", "RoleName").unwrap();
    assert!(
        role_rows
            .iter()
            .any(|r| r[name_i].as_text() == Some("Active Directory Certificate Services")),
        "expected the ADCS role name"
    );
}

#[test]
fn reads_rev300_guid_mdb_ual_tables() {
    let Some(p) = find_named(
        "{0A5818F9-9C96-4299-9EC3-FC84CFAAC55A}.mdb",
        "Collection-STDC1",
    ) else {
        return;
    };
    let db = Database::open(&p).expect("rev-300 GUID .mdb must open after the mask");
    let tables = db.tables().unwrap();
    for want in ["CLIENTS", "DNS", "ROLE_ACCESS"] {
        assert!(
            tables.iter().any(|t| t == want),
            "GUID mdb missing {want}: {tables:?}"
        );
    }
    let cols = db.columns("CLIENTS").unwrap();
    assert!(
        cols.len() > 300,
        "CLIENTS should have the Day1..Day366 columns: {}",
        cols.len()
    );
}
