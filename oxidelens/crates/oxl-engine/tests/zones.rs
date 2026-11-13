//! Each host's own Windows time zone, read from RETriage's
//! `retriage_timezoneinfo_system`, for the app's zone picker.

mod common;

use common::{analyst, loaded};
use oxl_engine::{host_zones, load_file, load_run, remove_source, FileOptions, HostZone};

fn zone(host: &str, windows_name: &str, iana: Option<&str>) -> HostZone {
    HostZone {
        host: host.into(),
        windows_name: windows_name.into(),
        iana: iana.map(str::to_string),
    }
}

#[test]
fn host_zones_from_the_registry() {
    let (d, mut case) = loaded();
    let root = oxl_fixtures::host_zones_run(d.path());
    let report = load_run(&mut case, &root, &analyst()).unwrap();
    // Only the `TimeZoneKeyName` rows: WS01's `Bias` row is not a zone.
    assert_eq!(
        host_zones(&case).unwrap(),
        vec![
            zone("SRV01", "UTC", Some("Etc/UTC")),
            zone("WS01", "Eastern Standard Time", Some("America/New_York")),
        ]
    );

    // A removed source's rows are no longer the case's.
    remove_source(&mut case, report.source_id, true, &analyst()).unwrap();
    assert_eq!(host_zones(&case).unwrap(), Vec::new());
}

#[test]
fn host_zones_without_the_relation_is_empty() {
    let (_d, case) = loaded();
    assert_eq!(host_zones(&case).unwrap(), Vec::new());
}

#[test]
fn an_unknown_windows_zone_has_no_iana_name() {
    // A zone name no CLDR mapping has is reported as written, never guessed.
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let file = d.path().join("tz.csv");
    std::fs::write(
        &file,
        "ValueName,ValueData\nTimeZoneKeyName,Mars Standard Time\nBias,0\n",
    )
    .unwrap();
    load_file(
        &mut case,
        &file,
        FileOptions {
            name: Some("retriage_timezoneinfo_system".into()),
            host: Some("WS01".into()),
        },
        &analyst(),
    )
    .unwrap();
    assert_eq!(
        host_zones(&case).unwrap(),
        vec![zone("WS01", "Mars Standard Time", None)]
    );
}
