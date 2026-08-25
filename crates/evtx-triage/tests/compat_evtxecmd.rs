//! Compatibility gate: EvtxTriage vs EvtxECmd reference fixtures.
//!
//! This is EvtxTriage's oracle gate — the regression safety net every other
//! suite tool has, and whose absence let EvtxTriage's column schema drift.
//!
//! Each fixture in `tests/fixtures/evtxecmd/` is a self-contained PAIR:
//!   <stem>.evtx                    — the source Windows event log
//!   <stem>__EvtxECmd_Output.csv    — EvtxECmd's CSV over that log (the oracle)
//! produced on Windows (EvtxECmd is Windows-only — see
//! `scripts/gen-evtxecmd-fixtures.sh` and `tests/fixtures/evtxecmd/README.md`).
//!
//! The EvtxTriage binary parses the same `.evtx` into a temp `--csv` dir (via
//! assert_cmd, exercising the full runner); the produced CSV is compared
//! row-for-row against the oracle, keyed by `EventRecordId` (unique per record).
//!
//! Timestamp handling is automatic: `compare_csv` normalizes the reference's
//! `yyyy-MM-dd HH:mm:ss.fffffff` TimeCreated to ISO 8601, which is exactly the
//! form EvtxTriage emits — so the intentional ISO-vs-space divergence compares
//! equal with no AcceptedDelta.

use std::path::{Path, PathBuf};

use triage_testkit::{compare_csv, AcceptedDelta};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/evtxecmd")
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

/// Run the real EvtxTriage binary over `evtx` into a leaked temp dir; return root.
fn run_evtxtriage(evtx: &Path) -> PathBuf {
    use assert_cmd::Command;
    let tmp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let out = tmp.path().to_path_buf();
    Command::cargo_bin("EvtxTriage")
        .unwrap()
        .arg("-f")
        .arg(evtx)
        .arg("--csv")
        .arg(&out)
        .assert()
        .success();
    out
}

/// The aggregate output CSV (its name carries the run-stamp prefix).
fn produced(root: &Path) -> Option<PathBuf> {
    walk(root).into_iter().find(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("_EvtxTriage_Output.csv"))
    })
}

fn basename(s: &str) -> &str {
    s.rsplit(['/', '\\']).next().unwrap_or(s)
}

/// SourceFile records the absolute input path, which legitimately differs
/// between the Windows capture EvtxECmd saw and this test's fixture path.
/// Compare by basename so a renamed/relocated log still matches while a wrong
/// source would not.
fn source_file_basename_eq(reference: &str, ours: &str) -> bool {
    basename(reference) == basename(ours)
}

/// EvtxECmd emits a nonzero file offset for the handful of records that carry
/// extra/template data; the high-level `evtx` parser doesn't expose it, so we
/// emit 0. Accept when ours is 0.
fn extra_data_offset_ok(_reference: &str, ours: &str) -> bool {
    ours == "0"
}

const DELTAS: &[AcceptedDelta] = &[
    AcceptedDelta {
        field: "SourceFile",
        reason: "SourceFile is the absolute input path; it differs between the \
                 Windows capture and the test fixture location. Compared by basename.",
        compare: source_file_basename_eq,
        row_guard: None,
    },
    AcceptedDelta {
        field: "ExtraDataOffset",
        reason: "Binary record offset EvtxECmd reports for a few records; not \
                 exposed by the high-level evtx parser, so EvtxTriage emits 0.",
        compare: extra_data_offset_ok,
        row_guard: None,
    },
];

/// Compare every committed EvtxECmd oracle against fresh EvtxTriage output.
///
/// Self-skipping: if no fixtures are committed (or a large `.evtx` source is
/// gitignored), the test logs and returns rather than failing — matching how
/// the other tools' compat gates behave on machines without evidence. Drop a
/// `<stem>.evtx` + `<stem>__EvtxECmd_Output.csv` pair into the fixtures dir to
/// activate it.
#[test]
fn evtxtriage_matches_evtxecmd_fixtures() {
    let dir = fixture_dir();
    if !dir.exists() {
        eprintln!(
            "SKIP: {} absent — no EvtxECmd fixtures committed yet.",
            dir.display()
        );
        return;
    }

    let oracles: Vec<PathBuf> = walk(&dir)
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("__EvtxECmd_Output.csv"))
        })
        .collect();
    if oracles.is_empty() {
        eprintln!(
            "SKIP: no *__EvtxECmd_Output.csv fixtures in {}.",
            dir.display()
        );
        return;
    }

    let mut compared = 0usize;
    for oracle in &oracles {
        let name = oracle.file_name().and_then(|n| n.to_str()).unwrap();
        let stem = name.strip_suffix("__EvtxECmd_Output.csv").unwrap();
        let evtx = dir.join(format!("{stem}.evtx"));
        if !evtx.exists() {
            eprintln!(
                "SKIP {stem}: oracle present but {stem}.evtx absent \
                 (large source logs may be gitignored)."
            );
            continue;
        }

        let root = run_evtxtriage(&evtx);
        let ours =
            produced(&root).unwrap_or_else(|| panic!("EvtxTriage produced no CSV for {stem}.evtx"));
        let diff = compare_csv(oracle, &ours, "EventRecordId", DELTAS);
        assert!(
            diff.is_match(),
            "EvtxECmd mismatch for {stem} (ref rows={}, our rows={}): {:?}",
            diff.reference_rows,
            diff.our_rows,
            diff.mismatches
        );
        compared += 1;
    }

    if compared == 0 {
        eprintln!(
            "NOTE: EvtxECmd oracle CSVs present but no matching .evtx sources; nothing compared."
        );
    }
}
