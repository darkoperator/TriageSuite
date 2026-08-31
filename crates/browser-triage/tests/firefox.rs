//! Firefox artifacts.
//!
//! The traps this suite exists for: PRTime microseconds rather than WebKit
//! microseconds, `moz_cookies` mixing seconds and microseconds inside one
//! table, downloads living in an annotation table, and both the username and
//! the password being encrypted in `logins.json`.

#![cfg(unix)]

mod support;

use rusqlite::Connection;
use std::path::Path;
use support::{column, firefox_profile_dir, read_output, rows, run};
use tempfile::TempDir;

/// 1_700_000_000_000_000 is PRTime microseconds for 2023-11-14T22:13:20Z.
fn write_places(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE moz_places(id INTEGER PRIMARY KEY, url TEXT, title TEXT,
             visit_count INTEGER, typed INTEGER, last_visit_date INTEGER, hidden INTEGER,
             frecency INTEGER, guid TEXT);
         CREATE TABLE moz_historyvisits(id INTEGER PRIMARY KEY, from_visit INTEGER,
             place_id INTEGER, visit_date INTEGER, visit_type INTEGER, source INTEGER);
         CREATE TABLE moz_bookmarks(id INTEGER PRIMARY KEY, type INTEGER, fk INTEGER,
             parent INTEGER, position INTEGER, title TEXT, keyword_id INTEGER,
             dateAdded INTEGER, lastModified INTEGER, guid TEXT);
         CREATE TABLE moz_anno_attributes(id INTEGER PRIMARY KEY, name TEXT);
         CREATE TABLE moz_annos(id INTEGER PRIMARY KEY, place_id INTEGER,
             anno_attribute_id INTEGER, content TEXT, dateAdded INTEGER);

         INSERT INTO moz_places VALUES
           (1,'https://mozilla.test/a','Page A',2,1,1700000000000000,0,100,'p1'),
           (2,'https://deleted.test/','Deleted',7,0,1699000000000000,0,50,'p2'),
           (3,'https://dl.test/tool.zip','Download',1,0,1700000000000000,0,10,'p3');

         INSERT INTO moz_historyvisits VALUES
           (10,0,1,1700000000000000,2,0),
           (11,10,1,1700003600000000,1,1),
           (12,0,99,1700007200000000,1,0);

         -- roots plus one folder and one bookmark inside it
         INSERT INTO moz_bookmarks VALUES
           (1,2,NULL,0,0,'',NULL,1700000000000000,1700000000000000,'root________'),
           (2,2,NULL,1,0,'',NULL,1700000000000000,1700000000000000,'toolbar_____'),
           (3,2,NULL,2,0,'Tools',NULL,1700000000000000,1700000100000000,'folder01____'),
           (4,1,1,3,0,'Page A bookmark',NULL,1700000200000000,1700000200000000,'bm0000000001');

         INSERT INTO moz_anno_attributes VALUES
           (1,'downloads/destinationFileURI'), (2,'downloads/metaData');
         INSERT INTO moz_annos VALUES
           (1,3,1,'file:///C:/Users/a/Downloads/tool.zip',1700000000000000),
           (2,3,2,'{\"state\":1,\"endTime\":1700000060000,\"fileSize\":2048}',1700000000000000);",
    )
    .unwrap();
}

fn write_cookies(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE moz_cookies(id INTEGER PRIMARY KEY, host TEXT, name TEXT, value TEXT,
             path TEXT, expiry INTEGER, lastAccessed INTEGER, creationTime INTEGER,
             isSecure INTEGER, isHttpOnly INTEGER, sameSite INTEGER, originAttributes TEXT);
         -- expiry is unix SECONDS while the other two are PRTime MICROseconds.
         INSERT INTO moz_cookies VALUES
           (1,'.mozilla.test','SID','plaintext-value','/',1800000000,
            1700003600000000,1700000000000000,1,1,2,''),
           (2,'.session.test','TEMP','x','/',0,
            1700003600000000,1700000000000000,0,0,0,'');",
    )
    .unwrap();
}

fn write_formhistory(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE moz_formhistory(id INTEGER PRIMARY KEY, fieldname TEXT, value TEXT,
             timesUsed INTEGER, firstUsed INTEGER, lastUsed INTEGER, guid TEXT);
         INSERT INTO moz_formhistory VALUES
           (1,'email','user@mozilla.test',3,1700000000000000,1700003600000000,'g1');",
    )
    .unwrap();
}

const LOGINS: &str = r#"{
  "logins": [
    {"id": 1, "origin": "https://mail.test", "formActionOrigin": "https://mail.test/auth",
     "usernameField": "user", "passwordField": "pass",
     "encryptedUsername": "MDIEEPgAAAAAAAAAAAAAAAAAAAE=",
     "encryptedPassword": "MEIEEPgAAAAAAAAAAAAAAAAAAAEwFAYIKoZIhvcNAwcECHh",
     "guid": "{abc}", "timeCreated": 1700000000000,
     "timeLastUsed": 1700003600000, "timePasswordChanged": 1700000000000, "timesUsed": 5}
  ],
  "disabledHosts": ["https://never-save.test"]
}"#;

const EXTENSIONS: &str = r#"{
  "addons": [
    {"id": "signed@test", "version": "1.0", "type": "extension", "active": true,
     "userDisabled": false, "appDisabled": false, "signedState": 2,
     "location": "app-profile", "sourceURI": "https://addons.mozilla.org/x.xpi",
     "installDate": 1700000000000, "updateDate": 1700003600000,
     "defaultLocale": {"name": "Signed Addon", "description": "A normal one"},
     "userPermissions": {"permissions": ["tabs"], "origins": ["<all_urls>"]}},
    {"id": "unsigned@test", "version": "0.1", "type": "extension", "active": false,
     "userDisabled": true, "appDisabled": false, "signedState": 0,
     "location": "app-profile", "sourceURI": "",
     "installDate": 1700000000000,
     "defaultLocale": {"name": "Unsigned Addon"}}
  ]
}"#;

fn setup() -> (TempDir, std::path::PathBuf) {
    let td = TempDir::new().unwrap();
    let dir = firefox_profile_dir(td.path(), "alice", "ab12cd.default-release");
    write_places(&dir.join("places.sqlite"));
    write_cookies(&dir.join("cookies.sqlite"));
    write_formhistory(&dir.join("formhistory.sqlite"));
    std::fs::write(dir.join("logins.json"), LOGINS).unwrap();
    std::fs::write(dir.join("extensions.json"), EXTENSIONS).unwrap();
    let out = td.path().join("out");
    run(td.path(), &out);
    (td, out)
}

#[test]
fn firefox_is_attributed_with_its_brand_channel_and_profile() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    assert!(column(&csv_text, "Browser").iter().all(|b| b == "Firefox"));
    assert!(column(&csv_text, "Profile")
        .iter()
        .all(|p| p == "ab12cd.default-release"));
}

/// PRTime microseconds, not WebKit microseconds. Reading them as WebKit would
/// put every row in 1601.
#[test]
fn prtime_microseconds_decode_to_the_right_year() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    assert!(
        column(&csv_text, "Visit Time")
            .iter()
            .any(|t| t == "2023-11-14T22:13:20.0000000Z"),
        "{csv_text}"
    );
    assert!(
        !csv_text.contains("1601-"),
        "WebKit epoch applied by mistake"
    );
}

/// Same orphan discipline as Chromium: a place whose visits are gone is the
/// deletion signal and must survive.
#[test]
fn places_with_no_visits_are_kept_as_url_only_rows() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    let types = column(&csv_text, "Record Type");
    // 3 visits + 2 places with no visits (ids 2 and 3).
    assert_eq!(rows(&csv_text).len(), 5, "{csv_text}");
    assert_eq!(types.iter().filter(|t| *t == "Visit").count(), 3);
    assert_eq!(types.iter().filter(|t| *t == "URL Only").count(), 2);
    assert!(csv_text.contains("deleted.test"));
}

#[test]
fn a_visit_with_a_dangling_place_reference_is_emitted_and_noted() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    assert!(column(&csv_text, "Notes")
        .iter()
        .any(|n| n.contains("no matching moz_places row")));
}

#[test]
fn visit_types_and_sources_are_decoded() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    assert!(column(&csv_text, "Visit Type").iter().any(|t| t == "Typed"));
    assert!(column(&csv_text, "Visit Type").iter().any(|t| t == "Link"));
}

/// The unit trap: `expiry` is seconds while `creationTime` is microseconds.
#[test]
fn cookie_expiry_is_seconds_while_creation_is_microseconds() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Cookies.csv");
    assert!(
        column(&csv_text, "Created")
            .iter()
            .any(|t| t == "2023-11-14T22:13:20.0000000Z"),
        "creationTime is PRTime microseconds: {csv_text}"
    );
    assert!(
        column(&csv_text, "Expires")
            .iter()
            .any(|t| t.starts_with("2027-")),
        "expiry 1800000000 is unix seconds and lands in 2027: {csv_text}"
    );
}

/// The capability difference that justifies the Value Encrypted column.
#[test]
fn firefox_cookie_values_are_plaintext_and_marked_unencrypted() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Cookies.csv");
    assert!(column(&csv_text, "Value")
        .iter()
        .any(|v| v == "plaintext-value"));
    assert!(column(&csv_text, "Value Encrypted")
        .iter()
        .all(|f| f == "False"));
    let session = column(&csv_text, "Session Cookie");
    assert_eq!(session.iter().filter(|s| *s == "True").count(), 1);
}

/// Firefox's autofill epoch is the opposite of Chromium's.
#[test]
fn form_history_uses_prtime_not_unix_seconds() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Autofill.csv");
    assert!(column(&csv_text, "First Used")
        .iter()
        .any(|t| t == "2023-11-14T22:13:20.0000000Z"));
    assert!(column(&csv_text, "Value")
        .iter()
        .any(|v| v == "user@mozilla.test"));
    // Firefox has these, Chromium does not.
    assert!(column(&csv_text, "GUID").iter().any(|g| g == "g1"));
}

/// Downloads come from two annotation rows that must collapse into one record.
#[test]
fn a_download_is_assembled_from_its_two_annotations() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Downloads.csv");
    assert_eq!(rows(&csv_text).len(), 1, "{csv_text}");
    assert!(column(&csv_text, "Target Path")
        .iter()
        .any(|p| p.contains("tool.zip")));
    assert!(column(&csv_text, "Total Bytes").iter().any(|b| b == "2048"));
    assert!(column(&csv_text, "State").iter().any(|s| s == "Complete"));
    // endTime is unix milliseconds inside the metaData JSON.
    assert!(column(&csv_text, "End Time")
        .iter()
        .any(|t| t.starts_with("2023-11-14T22:14:20")));
}

/// The folder path is reconstructed by walking `parent`, and roots are
/// containers rather than bookmarks.
#[test]
fn the_bookmark_folder_path_is_rebuilt_from_the_parent_chain() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Bookmarks.csv");
    // The Tools folder and the bookmark inside it; the two roots are skipped.
    assert_eq!(rows(&csv_text).len(), 2, "{csv_text}");
    let line = csv_text
        .lines()
        .find(|l| l.contains("Page A bookmark"))
        .expect("the bookmark must be present");
    assert!(line.contains("Tools"), "folder path missing: {line}");
    assert!(line.contains("toolbar"), "root name missing: {line}");
}

/// Firefox encrypts the username too, which Chromium does not — this is why
/// the column exists.
#[test]
fn firefox_encrypts_the_username_as_well_as_the_password() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Logins.csv");
    let line = csv_text
        .lines()
        .find(|l| l.contains("mail.test"))
        .expect("the login must be present");
    assert!(line.contains("True"), "{line}");
    assert!(
        !csv_text.contains("MEIEEPgAAAAAAAAAAAAAAAAAAAEwFAYIKoZIhvcNAwcECHh"),
        "no ciphertext may reach the output"
    );
    assert!(
        column(&csv_text, "Username").iter().all(String::is_empty),
        "the username is NSS-encrypted and must not be guessed at"
    );
    assert!(column(&csv_text, "Password Encryption")
        .iter()
        .any(|e| e == "NSS"));
}

/// A site the user told Firefox never to save is evidence of intent.
#[test]
fn a_disabled_host_is_recorded_as_a_blocklist_entry() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Logins.csv");
    let line = csv_text
        .lines()
        .find(|l| l.contains("never-save.test"))
        .expect("the disabledHosts entry must be present");
    assert!(line.contains("True"), "{line}");
    assert!(line.contains("saving was declined"), "{line}");
}

/// An unsigned add-on on a release build is the thing worth spotting.
#[test]
fn addon_signing_state_and_disable_flags_are_reported() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Extensions.csv");
    assert_eq!(rows(&csv_text).len(), 2, "{csv_text}");
    assert!(column(&csv_text, "Signed State")
        .iter()
        .any(|s| s == "Signed"));
    assert!(column(&csv_text, "Signed State")
        .iter()
        .any(|s| s == "Missing"));
    assert!(column(&csv_text, "State")
        .iter()
        .any(|s| s == "User Disabled"));
    assert!(column(&csv_text, "From Store").iter().any(|f| f == "True"));
}
