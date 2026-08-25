//! MFTriage integration tests (ported from the standalone project).
//!
//! - $Boot: deterministic, uses the committed 8 KiB sample fixture.
//! - $MFT / $J: gated on `test captures/` presence (suite convention); run against
//!   the smallest real $MFT (DESKTOP capture) and its sibling $J.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
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

fn run(input: &Path, extra: &[&str]) -> PathBuf {
    let tmp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let out = tmp.path().to_path_buf();
    let mut cmd = Command::cargo_bin("MFTriage").unwrap();
    cmd.arg("-f").arg(input).arg("--csv").arg(&out);
    for e in extra {
        cmd.arg(e);
    }
    cmd.assert().success();
    out
}

fn read_produced(root: &Path, basename: &str) -> String {
    let p = walk(root)
        .into_iter()
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(basename))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| panic!("no {basename} produced"));
    std::fs::read_to_string(p).unwrap()
}

// The committed $Boot fixture referenced by the doc comment above is a real
// NTFS boot sector pulled from a test-capture host, so it isn't included in
// this public repo. Skip rather than fail on the missing file.
#[test]
#[ignore = "requires tests/fixtures/$Boot, a real captured boot sector not distributed publicly"]
fn boot_csv_matches_header() {
    let boot = fixtures().join("$Boot");
    let out = run(&boot, &[]);
    let csv = read_produced(&out, "MFTriage_$Boot_Output.csv");
    let header = std::fs::read_to_string(fixtures().join("boot_expected_header.txt")).unwrap();
    assert!(
        csv.starts_with(header.trim_end()),
        "boot header mismatch:\n{}",
        csv.lines().next().unwrap()
    );
}

fn find_capture_artifact(name_suffix: &str) -> Option<PathBuf> {
    let root = captures_root();
    if !root.exists() {
        return None;
    }
    let mut hits: Vec<PathBuf> = walk(&root)
        .into_iter()
        .filter(|p| {
            p.to_string_lossy().contains("ntfs")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(name_suffix))
                    .unwrap_or(false)
        })
        .collect();
    // Prefer the DESKTOP capture (smallest $MFT).
    hits.sort_by_key(|p| !p.to_string_lossy().contains("Collection-DESKTOP"));
    hits.into_iter().next()
}

#[test]
fn mft_csv_matches_header_when_evidence_present() {
    let Some(mft) = find_capture_artifact("$MFT") else {
        return;
    };
    let out = run(&mft, &["--fl"]);
    let csv = read_produced(&out, "MFTriage_$MFT_Output.csv");
    let header = std::fs::read_to_string(fixtures().join("mft_expected_header.txt")).unwrap();
    assert!(csv.starts_with(header.trim_end()), "mft header mismatch");
    assert!(
        csv.contains(",$MFT,"),
        "expected the $MFT self entry in output"
    );
    let listing = read_produced(&out, "MFTriage_$MFT_Output_FileListing.csv");
    let lheader = std::fs::read_to_string(fixtures().join("mft_file_listing_header.txt")).unwrap();
    assert!(
        listing.starts_with(lheader.trim_end()),
        "file-listing header mismatch"
    );
}

#[test]
fn usn_csv_matches_header_and_resolves_parent_paths_when_evidence_present() {
    let Some(j) = find_capture_artifact("UsnJrnl%3A$J") else {
        return;
    };
    let out = run(&j, &[]);
    let csv = read_produced(&out, "MFTriage_$J_Output.csv");
    let header = std::fs::read_to_string(fixtures().join("usn_expected_header.txt")).unwrap();
    assert!(csv.starts_with(header.trim_end()), "usn header mismatch");
    let any_parent = csv.lines().skip(1).any(|line| {
        line.split(',')
            .nth(6)
            .map(|c| !c.is_empty())
            .unwrap_or(false)
    });
    assert!(
        any_parent,
        "expected $J ParentPath populated from the sibling $MFT"
    );
}
