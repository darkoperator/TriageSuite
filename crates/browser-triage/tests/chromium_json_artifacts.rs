//! Chromium `Bookmarks` and `Preferences` — the JSON artifacts.
//!
//! These are the two a SQL-map engine cannot read at all, so they are a large
//! part of what this crate adds over SQLETriage.

#![cfg(unix)]

mod support;

use support::{column, output_exists, profile_dir, read_output, rows, run};
use tempfile::TempDir;

/// Chromium writes `date_added` as WebKit microseconds in a decimal *string*,
/// which a number-only accessor would silently drop.
const BOOKMARKS: &str = r#"{
  "roots": {
    "bookmark_bar": {
      "type": "folder", "name": "Bookmarks bar", "id": "1",
      "date_added": "13344473600000000",
      "children": [
        {"type": "url", "name": "Example", "url": "https://example.test/",
         "id": "5", "guid": "aaaa", "date_added": "13344473600000000"},
        {"type": "folder", "name": "Tools", "id": "6",
         "date_added": "13344473610000000", "date_modified": "13344473620000000",
         "children": [
           {"type": "url", "name": "Nested", "url": "https://nested.test/",
            "id": "7", "date_added": "13344473630000000"}
         ]}
      ]
    },
    "other": {
      "type": "folder", "name": "Other bookmarks", "id": "2",
      "children": [
        {"type": "url", "name": "Elsewhere", "url": "https://elsewhere.test/",
         "id": "8", "date_added": "13344473640000000"}
      ]
    },
    "sync_transaction_version": "1"
  },
  "version": 1
}"#;

const PREFERENCES: &str = r#"{
  "extensions": {
    "settings": {
      "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": {
        "state": 1, "location": 4, "from_webstore": false,
        "path": "C:\\tools\\evil-extension",
        "install_time": "13344473600000000",
        "last_update_time": "13344473700000000",
        "manifest": {"name": "Sideloaded Helper", "version": "1.2",
                     "manifest_version": 3,
                     "permissions": ["tabs"],
                     "host_permissions": ["<all_urls>"]},
        "active_permissions": {"api": ["cookies"], "explicit_host": ["https://*/*"]}
      },
      "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb": {
        "state": 0, "location": 1, "disable_reasons": 513,
        "manifest": {"name": "Disabled Thing", "version": "0.1"}
      },
      "cccccccccccccccccccccccccccccccc": {
        "state": 0, "disable_reasons": 1, "active_permissions": {"api": []}
      }
    }
  }
}"#;

fn setup(bookmarks: Option<&str>, preferences: Option<&str>) -> (TempDir, std::path::PathBuf) {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    if let Some(body) = bookmarks {
        std::fs::write(dir.join("Bookmarks"), body).unwrap();
    }
    if let Some(body) = preferences {
        std::fs::write(dir.join("Preferences"), body).unwrap();
    }
    let out = td.path().join("out");
    run(td.path(), &out);
    (td, out)
}

#[test]
fn every_bookmark_node_including_folders_reaches_the_output() {
    let (_td, out) = setup(Some(BOOKMARKS), None);
    let csv_text = read_output(&out, "BrowserTriage_Output_Bookmarks.csv");
    // Example, Nested and Elsewhere, plus the Tools folder. The two root
    // containers seed the folder path but are not bookmarks the user made.
    assert_eq!(rows(&csv_text).len(), 4, "{csv_text}");
    let types = column(&csv_text, "Type");
    assert_eq!(types.iter().filter(|t| *t == "URL").count(), 3);
    assert_eq!(
        types.iter().filter(|t| *t == "Folder").count(),
        1,
        "a folder is evidence too and must not be dropped"
    );
}

/// A number-only JSON accessor would lose every bookmark timestamp, because
/// Chromium stores them as decimal strings.
#[test]
fn the_string_encoded_webkit_timestamp_is_decoded() {
    let (_td, out) = setup(Some(BOOKMARKS), None);
    let csv_text = read_output(&out, "BrowserTriage_Output_Bookmarks.csv");
    assert!(
        column(&csv_text, "Date Added")
            .iter()
            .any(|d| d == "2023-11-14T22:13:20.0000000Z"),
        "{csv_text}"
    );
}

#[test]
fn the_folder_path_and_root_give_a_bookmark_its_context() {
    let (_td, out) = setup(Some(BOOKMARKS), None);
    let csv_text = read_output(&out, "BrowserTriage_Output_Bookmarks.csv");
    let line = csv_text
        .lines()
        .find(|l| l.contains("nested.test"))
        .expect("the nested bookmark must be present");
    assert!(line.contains("Tools"), "folder path missing: {line}");
    assert!(line.contains("bookmark_bar"), "root missing: {line}");

    assert!(column(&csv_text, "Root").iter().any(|r| r == "other"));
}

/// `roots` also carries scalar bookkeeping keys, which are not nodes.
#[test]
fn scalar_keys_under_roots_are_not_mistaken_for_bookmarks() {
    let (_td, out) = setup(Some(BOOKMARKS), None);
    let csv_text = read_output(&out, "BrowserTriage_Output_Bookmarks.csv");
    assert!(!csv_text.contains("sync_transaction_version"), "{csv_text}");
}

#[test]
fn every_extension_entry_reaches_the_output() {
    let (_td, out) = setup(None, Some(PREFERENCES));
    let csv_text = read_output(&out, "BrowserTriage_Output_Extensions.csv");
    assert_eq!(rows(&csv_text).len(), 3, "{csv_text}");
}

/// The column an examiner looks at first.
#[test]
fn a_sideloaded_extension_is_named_as_unpacked() {
    let (_td, out) = setup(None, Some(PREFERENCES));
    let csv_text = read_output(&out, "BrowserTriage_Output_Extensions.csv");
    let line = csv_text
        .lines()
        .find(|l| l.contains("Sideloaded Helper"))
        .expect("the sideloaded extension must be present");
    assert!(line.contains("Unpacked (Load)"), "{line}");
    assert!(line.contains("C:\\tools\\evil-extension"), "{line}");
}

/// Declared and granted permissions each understate the extension on their own.
#[test]
fn declared_and_granted_permissions_are_merged() {
    let (_td, out) = setup(None, Some(PREFERENCES));
    let csv_text = read_output(&out, "BrowserTriage_Output_Extensions.csv");
    assert!(column(&csv_text, "Permissions")
        .iter()
        .any(|p| p == "tabs|cookies"));
    assert!(column(&csv_text, "Host Permissions")
        .iter()
        .any(|p| p == "<all_urls>|https://*/*"));
}

#[test]
fn disable_reason_bits_are_decoded() {
    let (_td, out) = setup(None, Some(PREFERENCES));
    let csv_text = read_output(&out, "BrowserTriage_Output_Extensions.csv");
    assert!(column(&csv_text, "Disable Reasons")
        .iter()
        .any(|r| r == "User Action|Corrupted"));
}

/// A sparse entry is a real record and is emitted, but a row of blanks needs
/// to explain itself rather than looking like a parse failure.
#[test]
fn a_settings_stub_is_emitted_and_labelled() {
    let (_td, out) = setup(None, Some(PREFERENCES));
    let csv_text = read_output(&out, "BrowserTriage_Output_Extensions.csv");
    let line = csv_text
        .lines()
        .find(|l| l.contains("cccccccccccccccccccccccccccccccc"))
        .expect("the stub entry must still be emitted");
    assert!(line.contains("settings stub"), "{line}");
}

/// A `Preferences` belonging to unrelated software passes quietly rather than
/// being reported as corrupt evidence.
#[test]
fn a_preferences_file_without_extensions_produces_nothing_and_no_error() {
    let (_td, out) = setup(None, Some(r#"{"some_other_app": {"setting": 1}}"#));
    assert!(!output_exists(&out, "BrowserTriage_Output_Extensions.csv"));
}
