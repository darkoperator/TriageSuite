//! Spec section 10.3 compatibility gate: WxTTriage vs WxTCmd reference fixtures.
//!
//! Each fixture (tests/fixtures/wxtcmd/) was produced by the official WxTCmd net9
//! binary over one ActivitiesCache.db. The WxTTriage binary parses the same
//! database into a temp --csv dir (via assert_cmd, exercising the full runner);
//! each produced CSV is compared row-for-row against its fixture.
//!
//! Row key: `Id` (the GUID identity column) is unique per Activity row. For
//! Activity_PackageId, a single ActivityId can map to several PackageName rows,
//! so those are keyed by the composite (Id, Platform, Name).
//!
//! Timestamp normalization: our WinTimestamp renders 7-digit fractional seconds
//! (`...:49.0000000Z`); WxTCmd renders whole seconds. We pre-normalize OUR CSV
//! (strip the fractional suffix) so timestamp columns compare equal with no
//! AcceptedDeltas — both denote the same instant.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use triage_testkit::{compare_csv, compare_csv_composite, skip_if_missing, AcceptedDelta};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/wxtcmd")
}

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Locate the ActivitiesCache.db for a (collection, user, cdp) tuple.
fn find_db(collection: &str, user: &str, cdp: &str) -> Option<PathBuf> {
    walk(&captures_root()).into_iter().find(|p| {
        let s = p.to_string_lossy();
        s.ends_with("ActivitiesCache.db")
            && s.contains(&format!("/Collection-{collection}"))
            && s.contains(&format!("/Users/{user}/"))
            && s.contains(&format!("/ConnectedDevicesPlatform/{cdp}/"))
    })
}

/// Run the real WxTTriage binary over `db` into a leaked temp dir; return root.
fn run_wxttriage(db: &Path) -> PathBuf {
    use assert_cmd::Command;
    let tmp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let out = tmp.path().to_path_buf();
    Command::cargo_bin("WxTTriage")
        .unwrap()
        .arg("-f")
        .arg(db)
        .arg("--csv")
        .arg(&out)
        .assert()
        .success();
    out
}

fn produced(root: &Path, basename: &str) -> Option<PathBuf> {
    walk(root).into_iter().find(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(basename))
    })
}

/// Strip our 7-digit fractional seconds (`.NNNNNNNZ` -> `Z`) from the given
/// timestamp columns of our CSV; returns a path to a normalized temp copy.
fn normalize_wxt_csv(our_csv: &Path, ts_columns: &[&str]) -> PathBuf {
    let mut rdr = csv::Reader::from_path(our_csv).unwrap();
    let headers: Vec<String> = rdr.headers().unwrap().iter().map(str::to_string).collect();
    let ts_idx: Vec<usize> = ts_columns
        .iter()
        .filter_map(|c| headers.iter().position(|h| h == c))
        .collect();
    let tmp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let out_path = tmp.path().join("wxt_normalized.csv");
    let mut wtr = csv::Writer::from_path(&out_path).unwrap();
    wtr.write_record(&headers).unwrap();
    for rec in rdr.records() {
        let rec = rec.unwrap();
        let mut row: Vec<String> = rec.iter().map(str::to_string).collect();
        for &i in &ts_idx {
            if let Some(cell) = row.get_mut(i) {
                *cell = strip_fractional(cell);
            }
        }
        wtr.write_record(&row).unwrap();
    }
    wtr.flush().unwrap();
    out_path
}

fn strip_fractional(s: &str) -> String {
    if let Some(dot) = s.rfind('.') {
        if s.ends_with('Z') && s[dot + 1..].len() > 1 {
            return format!("{}Z", &s[..dot]);
        }
    }
    s.to_string()
}

const ACT_TS: &[&str] = &[
    "StartTime",
    "EndTime",
    "LastModifiedTime",
    "LastModifiedOnClient",
    "OriginalLastModifiedOnClient",
    "ExpirationTime",
    "CreatedInCloud",
];
const PKG_TS: &[&str] = &["Expires"];

const NO_DELTAS: &[AcceptedDelta] = &[];

/// Compare one Activity fixture.
fn compat_activity(
    collection: &str,
    user: &str,
    cdp: &str,
    fixture_name: &str,
    deltas: &[AcceptedDelta],
) {
    let fixture = fixture_dir().join(fixture_name);
    if skip_if_missing(&fixture, fixture_name) {
        return;
    }
    let Some(db) = find_db(collection, user, cdp) else {
        // No DB found. If the whole capture tree is absent (evidence is gitignored
        // and not present on this machine), skip like every other compat gate.
        // But if captures ARE present, a missing DB for an existing fixture is a
        // real mismatch — fail loudly rather than pass vacuously.
        if skip_if_missing(&captures_root(), "test captures") {
            return;
        }
        panic!("captures present but no ActivitiesCache.db found for {fixture_name} (collection={collection}, user={user}, cdp={cdp})");
    };
    let root = run_wxttriage(&db);
    let ours = produced(&root, "WxTTriage_Activity_Output.csv")
        .unwrap_or_else(|| panic!("no Activity CSV for {fixture_name}"));
    let norm = normalize_wxt_csv(&ours, ACT_TS);
    let diff = compare_csv(&fixture, &norm, "Id", deltas);
    assert!(
        diff.is_match(),
        "Activity mismatch {fixture_name}: {:?}",
        diff.mismatches
    );
}

/// Compare one PackageIDs fixture.
///
/// Row key: `(Id, Platform, Name)` composite — a given ActivityId can have
/// multiple rows per platform (e.g. one for System and one for SystemX86),
/// so neither `Id` alone nor `(Id, Platform)` is unique. WxTCmd emits all
/// rows; `(Id, Platform, Name)` is unique per row in both the fixture and our
/// output.
fn pkg_key(row: &BTreeMap<String, String>, _: &[&str]) -> String {
    let id = row.get("Id").map(|s| s.as_str()).unwrap_or("");
    let platform = row.get("Platform").map(|s| s.as_str()).unwrap_or("");
    let name = row.get("Name").map(|s| s.as_str()).unwrap_or("");
    format!("{id}|{platform}|{name}")
}

fn compat_pkg(
    collection: &str,
    user: &str,
    cdp: &str,
    fixture_name: &str,
    deltas: &[AcceptedDelta],
) {
    let fixture = fixture_dir().join(fixture_name);
    if skip_if_missing(&fixture, fixture_name) {
        return;
    }
    let Some(db) = find_db(collection, user, cdp) else {
        // No DB found. If the whole capture tree is absent (evidence is gitignored
        // and not present on this machine), skip like every other compat gate.
        // But if captures ARE present, a missing DB for an existing fixture is a
        // real mismatch — fail loudly rather than pass vacuously.
        if skip_if_missing(&captures_root(), "test captures") {
            return;
        }
        panic!("captures present but no ActivitiesCache.db found for {fixture_name} (collection={collection}, user={user}, cdp={cdp})");
    };
    let root = run_wxttriage(&db);
    let ours = produced(&root, "WxTTriage_Activity_PackageId_Output.csv")
        .unwrap_or_else(|| panic!("no PackageId CSV for {fixture_name}"));
    let norm = normalize_wxt_csv(&ours, PKG_TS);
    let diff = compare_csv_composite(&fixture, &norm, &[], pkg_key, deltas);
    assert!(
        diff.is_match(),
        "PackageId mismatch {fixture_name}: {:?}",
        diff.mismatches
    );
}

#[test]
fn desktop_localadmin_activity() {
    compat_activity(
        "DESKTOP",
        "localadmin",
        "L.localadmin",
        "DESKTOP__localadmin__L.localadmin__Activity.csv",
        NO_DELTAS,
    );
}
#[test]
fn desktop_localadmin_pkg() {
    compat_pkg(
        "DESKTOP",
        "localadmin",
        "L.localadmin",
        "DESKTOP__localadmin__L.localadmin__PackageIDs.csv",
        NO_DELTAS,
    );
}
#[test]
fn stcl1_admin_aad_activity() {
    compat_activity(
        "STCL1",
        "administrator",
        "AAD.87ac7114-9a08-446f-89d6-3bd199340701",
        "STCL1__administrator__AAD.87ac7114-9a08-446f-89d6-3bd199340701__Activity.csv",
        NO_DELTAS,
    );
}
#[test]
fn stcl1_admin_aad_pkg() {
    compat_pkg(
        "STCL1",
        "administrator",
        "AAD.87ac7114-9a08-446f-89d6-3bd199340701",
        "STCL1__administrator__AAD.87ac7114-9a08-446f-89d6-3bd199340701__PackageIDs.csv",
        NO_DELTAS,
    );
}
#[test]
fn stcl1_admin_local_activity() {
    compat_activity(
        "STCL1",
        "administrator",
        "L.administrator",
        "STCL1__administrator__L.administrator__Activity.csv",
        NO_DELTAS,
    );
}
#[test]
fn stcl1_admin_local_pkg() {
    compat_pkg(
        "STCL1",
        "administrator",
        "L.administrator",
        "STCL1__administrator__L.administrator__PackageIDs.csv",
        NO_DELTAS,
    );
}
#[test]
fn stcl1_cperez_local_activity() {
    compat_activity(
        "STCL1",
        "cperez",
        "L.cperez",
        "STCL1__cperez__L.cperez__Activity.csv",
        NO_DELTAS,
    );
}
#[test]
fn stcl1_cperez_local_pkg() {
    compat_pkg(
        "STCL1",
        "cperez",
        "L.cperez",
        "STCL1__cperez__L.cperez__PackageIDs.csv",
        NO_DELTAS,
    );
}
#[test]
fn stcl1_localadmin_activity() {
    compat_activity(
        "STCL1",
        "localadmin",
        "L.localadmin",
        "STCL1__localadmin__L.localadmin__Activity.csv",
        NO_DELTAS,
    );
}
#[test]
fn stcl1_localadmin_pkg() {
    compat_pkg(
        "STCL1",
        "localadmin",
        "L.localadmin",
        "STCL1__localadmin__L.localadmin__PackageIDs.csv",
        NO_DELTAS,
    );
}
