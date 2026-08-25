use assert_cmd::Command;

fn cmd() -> Command {
    Command::cargo_bin("LolTriage").unwrap()
}

#[test]
fn hash_and_filename_matches_across_two_sources() {
    let tmp = tempfile::tempdir().unwrap();

    let refs_dir = tmp.path().join("refs");
    std::fs::create_dir_all(&refs_dir).unwrap();
    std::fs::write(
        refs_dir.join("loldrivers_refs.json"),
        r#"[{"id":"2a6a38ca-f2e6-456e-9ccf-db59d8c80c9e","category":"vulnerable driver","mitre_id":"T1068","tags":["nvflash.sys"],"md5":"","sha1":"b9c3f4dcc7463cbec84b808d880194bbc304ccd0","sha256":""}]"#,
    )
    .unwrap();
    std::fs::write(
        refs_dir.join("lolrmm_refs.json"),
        r#"[{"name":"KiTTY","category":"RAT","install_basenames":["kitty.exe"],"sha256_hashes":[]}]"#,
    )
    .unwrap();

    let input_dir = tmp.path().join("prior_run_output");
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::write(
        input_dir.join("AmcacheTriage_UnassociatedFileEntries_Output.csv"),
        "ApplicationName,ProgramId,FileKeyLastWriteTimestamp,SHA1,IsOsComponent,FullPath,Name,FileExtension,LinkDate,ProductName,Size,Version,ProductVersion,LongPathHash,BinaryType,IsPeFile,BinFileVersion,BinProductVersion,Usn,Language,Description\n\
         Unassociated,prog-1,2024-01-01T00:00:00.0000000Z,b9c3f4dcc7463cbec84b808d880194bbc304ccd0,False,C:\\Windows\\System32\\drivers\\nvflash.sys,nvflash.sys,.sys,,,1024,,,,,False,,,0,,\n",
    )
    .unwrap();
    std::fs::write(
        input_dir.join("AppCompatTriage_AppCompatCache_Output.csv"),
        "ControlSet,CacheEntryPosition,Path,LastModifiedTimeUTC,Executed,Duplicate,SourceFile\n\
         1,0,C:\\Tools\\kitty.exe,2024-01-01T00:00:00.0000000Z,Yes,False,C:\\triage\\SYSTEM\n",
    )
    .unwrap();

    let out = tmp.path().join("out");
    cmd()
        .arg("-d")
        .arg(&input_dir)
        .arg("--csv")
        .arg(&out)
        .arg("--refs")
        .arg(&refs_dir)
        .arg("-q")
        .assert()
        .code(0);

    let csv_path = find_output(&out).expect("LolTriage_Output.csv");
    let content = std::fs::read_to_string(csv_path).unwrap();
    assert!(content.contains("LOLDrivers"), "{content}");
    assert!(content.contains("High"), "{content}");
    assert!(content.contains("LOLRMM"), "{content}");
    assert!(content.contains("KiTTY"), "{content}");
}

fn find_output(root: &std::path::Path) -> Option<std::path::PathBuf> {
    for e in std::fs::read_dir(root).ok()?.flatten() {
        let p = e.path();
        if p.file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with("LolTriage_Output.csv"))
        {
            return Some(p);
        }
    }
    None
}
