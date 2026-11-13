//! Timeline Explorer sessions a run carries (`Sessions/*.tle_sess`), kept in
//! the case at load time and mapped to the relations they open; and the
//! case's mark counts.

mod common;

use common::{analyst, copy_tree, n};
use oxl_engine::{
    history, load_run, mark_counts, mark_suspicious, remove_source, sessions, Case, Session,
    SessionRelation,
};
use std::path::Path;

fn relations(session: &Session) -> Vec<(&str, &str)> {
    session
        .relations
        .iter()
        .map(|SessionRelation { relation, host }| (relation.as_str(), host.as_str()))
        .collect()
}

/// The `after` of the case's one `source_load` history row for `source`.
fn load_history(case: &Case, source: i64) -> serde_json::Value {
    history(case, Some("source"), 50, 0)
        .unwrap()
        .items
        .into_iter()
        .find(|h| h.action == "source_load" && h.target_id == source.to_string())
        .and_then(|h| h.after)
        .expect("a source_load history row")
}

const EXPECTED: [(&str, &str); 2] = [("evtxtriage_events", "WS01"), ("petriage_timeline", "WS01")];

#[test]
fn sessions_map_to_relations() {
    let (_d, case) = common::loaded();
    let found = sessions(&case).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    let session = &found[0];
    assert_eq!(
        (session.source_id, session.name.as_str()),
        (1, "Execution_Analysis")
    );
    assert_eq!(relations(session), EXPECTED);
    assert_eq!(session.unmatched, 1);
    let after = load_history(&case, 1);
    assert_eq!(after["sessions"], serde_json::json!(["Execution_Analysis"]));
    assert_eq!(after["sessions_skipped"], serde_json::json!([]));
}

#[test]
fn sessions_survive_a_moved_run() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts_moved(&d.path().join("m"));
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    let found = sessions(&case).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(relations(&found[0]), EXPECTED);
    assert_eq!(found[0].unmatched, 1);
}

/// The suite writes `/`-separated relative paths in its inventory and the
/// session names absolute Windows paths: case and separators do not matter,
/// but a match must be a whole path component.
#[test]
fn entries_match_by_whole_trailing_components_without_regard_to_case() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(&d.path().join("a"));
    oxl_fixtures::write_session(
        &root,
        "Lateral_Movement_Logons",
        &[
            r"D:\Elsewhere\PROCESSED-SRV01-20260101000000\eventlogs\EVTX.CSV",
            "/mnt/x/Processed-SRV01-20260101000000/Execution/pf.csv",
            // Not a component boundary: `xProcessed-…` is another folder.
            r"D:\xProcessed-WS01-20260101000000\Hayabusa\timeline.csv",
            r"D:\run\Processed-WS01-20260101000000\Hayabusa\timeline.csv",
        ],
    );
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    let found = sessions(&case).unwrap();
    let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Execution_Analysis", "Lateral_Movement_Logons"]);
    let lateral = &found[1];
    assert_eq!(
        relations(lateral),
        [
            ("evtxtriage_events", "SRV01"),
            ("ext_hayabusa_csv_timeline", "WS01"),
            ("petriage_timeline", "SRV01"),
        ]
    );
    assert_eq!(lateral.unmatched, 1);
}

/// A session file of `bytes` bytes that parses, naming WS01's prefetch.
fn padded_session(root: &Path, name: &str, bytes: usize) {
    let path = oxl_fixtures::write_session(
        root,
        name,
        &[r"C:\x\Processed-WS01-20260101000000\Execution\pf.csv"],
    );
    let mut text = std::fs::read_to_string(&path).unwrap();
    assert!(text.len() <= bytes);
    text.push_str(&" ".repeat(bytes - text.len()));
    std::fs::write(&path, text).unwrap();
}

#[test]
fn oversized_session_files_are_skipped() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(&d.path().join("a"));
    const MIB: usize = 1024 * 1024;
    padded_session(&root, "Big", MIB + 1);
    padded_session(&root, "Edge", MIB);
    let mut case = oxl_fixtures::new_case(d.path());
    let report = load_run(&mut case, &root, &analyst()).unwrap();
    let names: Vec<String> = sessions(&case)
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["Edge", "Execution_Analysis"]);
    let after = load_history(&case, report.source_id);
    let skipped = after["sessions_skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(skipped[0]["file"], "Big.tle_sess");
    assert!(
        skipped[0]["reason"].as_str().unwrap().contains("1 MiB"),
        "{skipped:?}"
    );
    assert_eq!(
        n(
            &case,
            "SELECT count(*) FROM ox.run_session WHERE name = 'Big'"
        ),
        0
    );
}

#[test]
fn at_most_64_session_files_are_kept() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(&d.path().join("a"));
    for i in 0..64 {
        oxl_fixtures::write_session(&root, &format!("S{i:02}"), &[]);
    }
    let mut case = oxl_fixtures::new_case(d.path());
    let report = load_run(&mut case, &root, &analyst()).unwrap();
    assert_eq!(n(&case, "SELECT count(*) FROM ox.run_session"), 64);
    // Taken in name order: the last one is left out.
    let skipped = &load_history(&case, report.source_id)["sessions_skipped"];
    assert_eq!(skipped.as_array().unwrap().len(), 1, "{skipped}");
    assert_eq!(skipped[0]["file"], "S63.tle_sess");
}

#[test]
fn a_session_file_that_does_not_parse_is_skipped_and_the_run_loads() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(&d.path().join("a"));
    let sessions_dir = root.join("Sessions");
    std::fs::write(sessions_dir.join("Broken.tle_sess"), "{not json").unwrap();
    std::fs::write(
        sessions_dir.join("Shapeless.tle_sess"),
        r#"{"SessionFiles": [1]}"#,
    )
    .unwrap();
    // Not a session file at all: ignored, not skipped.
    std::fs::write(sessions_dir.join("readme.txt"), "hello").unwrap();
    std::fs::create_dir(sessions_dir.join("Folder.tle_sess")).unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let report = load_run(&mut case, &root, &analyst()).unwrap();
    let names: Vec<String> = sessions(&case)
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["Execution_Analysis"]);
    let skipped = &load_history(&case, report.source_id)["sessions_skipped"];
    let files: Vec<&str> = skipped
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["file"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["Broken.tle_sess", "Shapeless.tle_sess"]);
}

#[test]
fn a_run_without_sessions_has_none() {
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let report = load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_prefetch(d.path()),
        &analyst(),
    )
    .unwrap();
    assert!(sessions(&case).unwrap().is_empty());
    assert_eq!(
        load_history(&case, report.source_id)["sessions_skipped"],
        serde_json::json!([])
    );
}

/// A run whose every dataset is empty has no rows in any view, so it
/// offers no session, though the session is kept with it.
#[test]
fn a_run_with_no_rows_offers_no_sessions() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::empty_dataset(&d.path().join("a"));
    oxl_fixtures::write_session(
        &root,
        "File_And_Folder_Access",
        &[r"C:\x\Processed-WS01-20260103000000\FileSystem\recyclebin.csv"],
    );
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    assert_eq!(
        n(&case, "SELECT count(*) FROM ox.source_dataset WHERE active"),
        1
    );
    assert_eq!(n(&case, "SELECT count(*) FROM ox.run_session"), 1);
    assert!(sessions(&case).unwrap().is_empty());
}

/// The skipped entries of the load of `source`, as (file, reason).
fn skipped_of(case: &Case, source: i64) -> Vec<(String, String)> {
    load_history(case, source)["sessions_skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["file"].as_str().unwrap().to_string(),
                s["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// A session file outside the run, which a link in it might point at.
fn outside_session(dir: &Path) -> std::path::PathBuf {
    let outside = dir.join("outside");
    oxl_fixtures::write_session(&outside, "Outside", &[r"C:\secret\x.csv"])
}

/// A `.tle_sess` that is a link is never followed out of the run folder.
#[test]
fn a_linked_session_file_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(&d.path().join("a"));
    let target = outside_session(d.path());
    let link = root.join("Sessions").join("Linked.tle_sess");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(&target, &link);
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&target, &link);
    if let Err(e) = made {
        println!("note: skipped, this system refused to make a symbolic link: {e}");
        return;
    }
    let mut case = oxl_fixtures::new_case(d.path());
    let report = load_run(&mut case, &root, &analyst()).unwrap();
    let names: Vec<String> = sessions(&case)
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["Execution_Analysis"]);
    assert_eq!(
        skipped_of(&case, report.source_id),
        [("Linked.tle_sess".to_string(), "link".to_string())]
    );
    assert_eq!(
        n(
            &case,
            "SELECT count(*) FROM ox.run_session WHERE name IN ('Linked', 'Outside')"
        ),
        0
    );
}

/// Make `link` a directory link to `target`: a symbolic link, or on
/// Windows, where one may need a privilege, a junction, which needs none.
fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return Ok(());
        }
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "mklink /J exited with {status}"
            )))
        }
    }
}

/// A `Sessions` folder that is a link (on Windows, a junction when no
/// symbolic link can be made) is not followed either.
#[test]
fn a_linked_sessions_folder_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(&d.path().join("a"));
    let target = outside_session(d.path());
    let sessions_dir = root.join("Sessions");
    std::fs::remove_dir_all(&sessions_dir).unwrap();
    if let Err(e) = link_dir(target.parent().unwrap(), &sessions_dir) {
        println!("note: skipped, this system refused to make a directory link: {e}");
        return;
    }
    // The link works: through it the outside session file is readable.
    assert!(sessions_dir.join("Outside.tle_sess").is_file());
    let mut case = oxl_fixtures::new_case(d.path());
    let report = load_run(&mut case, &root, &analyst()).unwrap();
    assert!(sessions(&case).unwrap().is_empty());
    assert_eq!(n(&case, "SELECT count(*) FROM ox.run_session"), 0);
    assert_eq!(
        skipped_of(&case, report.source_id),
        [("Sessions".to_string(), "link".to_string())]
    );
}

/// A junction is what a Windows user can make without a privilege, so the
/// check is pinned on one there whatever symbolic links the system allows.
#[cfg(windows)]
#[test]
fn a_sessions_junction_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(&d.path().join("a"));
    let target = outside_session(d.path());
    let sessions_dir = root.join("Sessions");
    std::fs::remove_dir_all(&sessions_dir).unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&sessions_dir)
        .arg(target.parent().unwrap())
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J exited with {status}");
    let meta = std::fs::symlink_metadata(&sessions_dir).unwrap();
    println!(
        "a junction: is_symlink {}, is_dir {}",
        meta.file_type().is_symlink(),
        meta.is_dir()
    );
    let mut case = oxl_fixtures::new_case(d.path());
    let report = load_run(&mut case, &root, &analyst()).unwrap();
    assert!(sessions(&case).unwrap().is_empty());
    assert_eq!(
        skipped_of(&case, report.source_id),
        [("Sessions".to_string(), "link".to_string())]
    );
}

#[test]
fn a_removed_run_offers_no_sessions() {
    let (_d, mut case) = common::loaded();
    assert_eq!(sessions(&case).unwrap().len(), 1);
    remove_source(&mut case, 1, true, &analyst()).unwrap();
    assert!(sessions(&case).unwrap().is_empty());
}

/// The run's files are read, never written: loading leaves the session
/// files as they were.
#[test]
fn loading_leaves_the_session_files_unchanged() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(&d.path().join("a"));
    let file = root.join("Sessions").join("Execution_Analysis.tle_sess");
    let before = std::fs::read(&file).unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), before);
}

#[test]
fn mark_counts_count_active_rows_only() {
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    assert_eq!(mark_counts(&case).unwrap().suspicious, 0);
    let chrome = n(
        &case,
        "SELECT _ox_row FROM petriage_timeline \
         WHERE ExecutableName = 'CHROME.EXE' AND _triage_host = 'WS01'",
    );
    let event = n(
        &case,
        "SELECT _ox_row FROM evtxtriage_events WHERE PayloadData1 = 'rclone.exe'",
    );
    for row in [chrome, event] {
        assert!(mark_suspicious(&mut case, &[row], &analyst()).unwrap()[0].done);
    }
    assert_eq!(mark_counts(&case).unwrap().suspicious, 2);

    // A later run of WS01's prefetch in which CHROME.EXE's row differs: its
    // mark has no identical row to move to and stays on the superseded one.
    let rerun = oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b"));
    let changed = d.path().join("c");
    copy_tree(&rerun, &changed);
    let csv = changed.join("Processed-WS01-20260102000000/Execution/pf.csv");
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(text.contains("CHROME.EXE,412"));
    std::fs::write(&csv, text.replace("CHROME.EXE,412", "CHROME.EXE,413")).unwrap();
    let report = load_run(&mut case, &changed, &analyst()).unwrap();
    assert_eq!(report.superseded[0].left_behind, 1, "{report:?}");
    assert_eq!(n(&case, "SELECT count(*) FROM ox.row_mark"), 2);
    assert_eq!(mark_counts(&case).unwrap().suspicious, 1);
}
