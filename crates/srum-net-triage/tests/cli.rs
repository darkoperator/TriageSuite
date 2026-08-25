use assert_cmd::Command;

fn cmd() -> Command {
    Command::cargo_bin("SrumNetTriage").unwrap()
}

fn find_output(root: &std::path::Path, suffix: &str) -> Option<std::path::PathBuf> {
    for e in std::fs::read_dir(root).ok()?.flatten() {
        let p = e.path();
        if p.file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with(suffix))
        {
            return Some(p);
        }
    }
    None
}

#[test]
fn rolls_up_network_usage_and_connection_csvs() {
    let tmp = tempfile::tempdir().unwrap();

    let input_dir = tmp.path().join("prior_srumetriage_output");
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::write(
        input_dir.join("SrumETriage_NetworkUsages_Output.csv"),
        "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,UserId,AppId,BytesReceived,BytesSent,InterfaceLuid,InterfaceType,L2ProfileFlags,L2ProfileId,ProfileName\n\
         1,2024-06-29T10:00:00.0000000Z,chrome.exe,,,,,alice,1,1,500,1000,0,Wired80211,0,1,\n\
         2,2024-06-30T02:00:00.0000000Z,beacon.exe,,,,,alice,1,2,0,90000,0,Wired80211,0,1,\n",
    )
    .unwrap();
    std::fs::write(
        input_dir.join("SrumETriage_NetworkConnections_Output.csv"),
        "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,UserId,AppId,ConnectedTime,ConnectStartTime,InterfaceLuid,InterfaceType,L2ProfileFlags,L2ProfileId,ProfileName\n\
         1,2024-06-29T10:00:00.0000000Z,chrome.exe,,,,,alice,1,1,300,,0,Wired80211,0,1,\n",
    )
    .unwrap();

    let out = tmp.path().join("out");
    cmd()
        .arg("-d")
        .arg(&input_dir)
        .arg("--csv")
        .arg(&out)
        .arg("--tz")
        .arg("UTC")
        .arg("-q")
        .assert()
        .code(0);

    let daily =
        find_output(&out, "SrumNetTriage_DailySummary_Output.csv").expect("DailySummary output");
    let daily_content = std::fs::read_to_string(daily).unwrap();
    assert!(daily_content.contains("beacon.exe"), "{daily_content}");
    assert!(daily_content.contains("90000"), "{daily_content}");
    // beacon.exe's 90000-byte day sorts before chrome.exe's 1000-byte day.
    assert!(
        daily_content.find("beacon.exe").unwrap() < daily_content.find("chrome.exe").unwrap(),
        "{daily_content}"
    );

    let hourly = find_output(&out, "SrumNetTriage_HourlyFingerprint_Output.csv")
        .expect("HourlyFingerprint output");
    let hourly_content = std::fs::read_to_string(hourly).unwrap();
    assert!(hourly_content.contains("true"), "{hourly_content}");

    let sessions = find_output(&out, "SrumNetTriage_SessionSummary_Output.csv")
        .expect("SessionSummary output");
    let sessions_content = std::fs::read_to_string(sessions).unwrap();
    assert!(
        sessions_content.contains("chrome.exe"),
        "{sessions_content}"
    );
    assert!(sessions_content.contains("300"), "{sessions_content}");
}

#[test]
fn explicit_tz_overrides_bad_system_hive_and_still_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let input_dir = tmp.path().join("prior_srumetriage_output");
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::write(
        input_dir.join("SrumETriage_NetworkUsages_Output.csv"),
        "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,UserId,AppId,BytesReceived,BytesSent,InterfaceLuid,InterfaceType,L2ProfileFlags,L2ProfileId,ProfileName\n\
         1,2024-06-29T10:00:00.0000000Z,chrome.exe,,,,,alice,1,1,500,1000,0,Wired80211,0,1,\n",
    )
    .unwrap();

    let out = tmp.path().join("out");
    // --tz wins even though --system-hive points nowhere useful.
    cmd()
        .arg("-d")
        .arg(&input_dir)
        .arg("--csv")
        .arg(&out)
        .arg("--tz")
        .arg("+05:00")
        .arg("--system-hive")
        .arg(tmp.path().join("does-not-exist"))
        .arg("-q")
        .assert()
        .code(0);

    let hourly = find_output(&out, "SrumNetTriage_HourlyFingerprint_Output.csv")
        .expect("HourlyFingerprint output");
    let content = std::fs::read_to_string(hourly).unwrap();
    // 10:00 UTC + 5h = hour 15 local.
    assert!(content.contains(",15,"), "{content}");
}

#[test]
fn bogus_system_hive_falls_back_to_utc_with_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let input_dir = tmp.path().join("prior_srumetriage_output");
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::write(
        input_dir.join("SrumETriage_NetworkUsages_Output.csv"),
        "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,UserId,AppId,BytesReceived,BytesSent,InterfaceLuid,InterfaceType,L2ProfileFlags,L2ProfileId,ProfileName\n\
         1,2024-06-29T10:00:00.0000000Z,chrome.exe,,,,,alice,1,1,500,1000,0,Wired80211,0,1,\n",
    )
    .unwrap();

    let out = tmp.path().join("out");
    cmd()
        .arg("-d")
        .arg(&input_dir)
        .arg("--csv")
        .arg(&out)
        .arg("--system-hive")
        .arg(tmp.path().join("does-not-exist"))
        .arg("-q")
        .assert()
        .code(0)
        .stderr(predicates::str::contains("could not auto-detect timezone"));

    let hourly = find_output(&out, "SrumNetTriage_HourlyFingerprint_Output.csv")
        .expect("HourlyFingerprint output");
    let content = std::fs::read_to_string(hourly).unwrap();
    // Falls back to UTC: 10:00 UTC stays hour 10.
    assert!(content.contains(",10,"), "{content}");
}
