//! Shared helpers for the end-to-end suites: build a realistic capture, run the
//! real binary over it, and read the CSV back.
//!
//! Each integration test binary compiles this module in full but uses only the
//! helpers it needs, so unused ones here are expected rather than dead.

#![allow(dead_code)]

use assert_cmd::Command;
use std::path::{Path, PathBuf};

/// A Velociraptor-shaped profile directory, so attribution and percent-decoding
/// are exercised by every test rather than only the ones that target them.
pub fn profile_dir(root: &Path, user: &str, vendor: &str, profile: &str) -> PathBuf {
    let dir = root
        .join("uploads/auto/C%3A/Users")
        .join(user)
        .join("AppData/Local")
        .join(vendor)
        .join("User Data")
        .join(profile);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A Firefox profile directory, which anchors on `Profiles` instead.
pub fn firefox_profile_dir(root: &Path, user: &str, profile: &str) -> PathBuf {
    let dir = root
        .join("uploads/auto/C%3A/Users")
        .join(user)
        .join("AppData/Roaming/Mozilla/Firefox/Profiles")
        .join(profile);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn run(capture: &Path, out: &Path) {
    Command::cargo_bin("BrowserTriage")
        .unwrap()
        .args([
            "-d",
            capture.to_str().unwrap(),
            "--csv",
            out.to_str().unwrap(),
            "-q",
        ])
        .assert()
        .success();
}

/// Flat layout folds the identity into the filename and prefixes a run stamp,
/// so outputs are found by suffix rather than by an exact name.
pub fn read_output(out: &Path, suffix: &str) -> String {
    let entry = std::fs::read_dir(out)
        .unwrap_or_else(|e| panic!("no output directory {}: {e}", out.display()))
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix))
        })
        .unwrap_or_else(|| panic!("no output file ending in {suffix} under {}", out.display()));
    std::fs::read_to_string(entry).unwrap()
}

pub fn output_exists(out: &Path, suffix: &str) -> bool {
    std::fs::read_dir(out)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_str().is_some_and(|n| n.ends_with(suffix)))
        })
        .unwrap_or(false)
}

pub fn rows(csv_text: &str) -> Vec<Vec<String>> {
    let mut reader = csv::Reader::from_reader(csv_text.as_bytes());
    reader
        .records()
        .map(|r| r.unwrap().iter().map(str::to_string).collect())
        .collect()
}

pub fn column(csv_text: &str, name: &str) -> Vec<String> {
    let mut reader = csv::Reader::from_reader(csv_text.as_bytes());
    let index = reader
        .headers()
        .unwrap()
        .iter()
        .position(|h| h == name)
        .unwrap_or_else(|| panic!("no column {name}"));
    reader
        .records()
        .map(|r| r.unwrap()[index].to_string())
        .collect()
}
