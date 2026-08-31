//! Chromium `cookies`.
//!
//! Two regressions drive this suite. First, the `is_secure`/`secure` rename,
//! which makes a hardcoded query fail outright. Second, and worse because it
//! fails quietly: `encrypted_value` is declared BLOB but SQLite hands many
//! cells back as Text, so matching only `Blob` reported 698 of 1899 encrypted
//! cookies on a real profile as unencrypted.

#![cfg(unix)]

mod support;

use rusqlite::Connection;
use std::path::Path;
use support::{column, profile_dir, read_output, rows, run};
use tempfile::TempDir;

/// Modern schema (`is_secure`, `last_update_utc`), with the value stored as
/// TEXT rather than BLOB — which is what a real Chrome profile does.
fn write_modern(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE cookies(creation_utc INTEGER, host_key TEXT, top_frame_site_key TEXT,
             name TEXT, value TEXT, encrypted_value BLOB, path TEXT, expires_utc INTEGER,
             is_secure INTEGER, is_httponly INTEGER, last_access_utc INTEGER,
             has_expires INTEGER, is_persistent INTEGER, priority INTEGER, samesite INTEGER,
             source_scheme INTEGER, source_port INTEGER, last_update_utc INTEGER);

         INSERT INTO cookies VALUES
           (13344473600000000,'.example.test','','SESSIONID','', 'v10ciphertexthere',
            '/', 13400000000000000, 1,1, 13344473700000000, 1,1, 2, 2, 2, 443,
            13344473800000000),
           (13344473610000000,'.appbound.test','','ABTOKEN','', 'v20ciphertexthere',
            '/', 13400000000000000, 1,1, 13344473710000000, 1,1, 1, 1, 2, 443, 0),
           (13344473620000000,'.session.test','','TEMP','', X'',
            '/', 0, 0,0, 13344473720000000, 0,0, 1, 0, 1, 80, 0);",
    )
    .unwrap();
}

/// Historical schema: `secure`/`httponly`, no `last_update_utc`, no
/// `source_port`.
fn write_legacy(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE cookies(creation_utc INTEGER, host_key TEXT, name TEXT, value TEXT,
             path TEXT, expires_utc INTEGER, secure INTEGER, httponly INTEGER,
             last_access_utc INTEGER, has_expires INTEGER, persistent INTEGER,
             priority INTEGER, encrypted_value BLOB);

         INSERT INTO cookies VALUES
           (13344473600000000,'.old.test','PLAIN','plaintext-value','/',
            13400000000000000, 0,0, 13344473700000000, 1,1, 1, X'');",
    )
    .unwrap();
}

fn setup(writer: fn(&Path)) -> (TempDir, std::path::PathBuf) {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    // Chrome 96+ keeps Cookies under <profile>/Network/.
    let network = dir.join("Network");
    std::fs::create_dir_all(&network).unwrap();
    writer(&network.join("Cookies"));
    let out = td.path().join("out");
    run(td.path(), &out);
    (td, out)
}

#[test]
fn every_cookie_reaches_the_output() {
    let (_td, out) = setup(write_modern);
    let csv_text = read_output(&out, "BrowserTriage_Output_Cookies.csv");
    assert_eq!(rows(&csv_text).len(), 3, "{csv_text}");
}

/// The quiet one: a TEXT-typed `encrypted_value` must still count as encrypted.
#[test]
fn a_text_typed_ciphertext_is_still_recognized_as_encrypted() {
    let (_td, out) = setup(write_modern);
    let csv_text = read_output(&out, "BrowserTriage_Output_Cookies.csv");
    let flags = column(&csv_text, "Value Encrypted");
    assert_eq!(
        flags.iter().filter(|f| *f == "True").count(),
        2,
        "both ciphertexts must be seen despite being stored as TEXT: {csv_text}"
    );
    let schemes = column(&csv_text, "Encryption Scheme");
    assert!(schemes.iter().any(|s| s == "v10"), "{schemes:?}");
    assert!(schemes.iter().any(|s| s == "v20"), "{schemes:?}");
}

/// No decryption is attempted, and no ciphertext reaches the output in any
/// form — the value column stays empty and only its length is reported.
#[test]
fn no_ciphertext_is_ever_emitted() {
    let (_td, out) = setup(write_modern);
    let csv_text = read_output(&out, "BrowserTriage_Output_Cookies.csv");
    assert!(
        !csv_text.contains("ciphertexthere"),
        "ciphertext must never appear in output: {csv_text}"
    );
    assert!(column(&csv_text, "Value Length").iter().any(|l| l == "17"));
}

/// The legacy spelling must not make the whole statement fail.
#[test]
fn the_legacy_secure_and_httponly_columns_are_read() {
    let (_td, out) = setup(write_legacy);
    let csv_text = read_output(&out, "BrowserTriage_Output_Cookies.csv");
    assert_eq!(rows(&csv_text).len(), 1, "{csv_text}");
    assert!(column(&csv_text, "Host").iter().any(|h| h == ".old.test"));
    // Firefox-style plaintext does come through when there is no ciphertext.
    assert!(column(&csv_text, "Value")
        .iter()
        .any(|v| v == "plaintext-value"));
    assert!(column(&csv_text, "Value Encrypted")
        .iter()
        .all(|f| f == "False"));
}

/// Columns a legacy schema lacks are empty, not a reason to fail the table.
#[test]
fn columns_absent_from_an_old_schema_are_simply_empty() {
    let (_td, out) = setup(write_legacy);
    let csv_text = read_output(&out, "BrowserTriage_Output_Cookies.csv");
    assert!(column(&csv_text, "Last Updated")
        .iter()
        .all(String::is_empty));
    assert!(column(&csv_text, "Source Port")
        .iter()
        .all(String::is_empty));
}

#[test]
fn enum_columns_are_decoded() {
    let (_td, out) = setup(write_modern);
    let csv_text = read_output(&out, "BrowserTriage_Output_Cookies.csv");
    assert!(column(&csv_text, "SameSite").iter().any(|s| s == "Strict"));
    assert!(column(&csv_text, "Priority").iter().any(|p| p == "High"));
    assert!(column(&csv_text, "Source Scheme")
        .iter()
        .any(|s| s == "Secure"));
}

#[test]
fn a_session_cookie_is_distinguished_from_a_persistent_one() {
    let (_td, out) = setup(write_modern);
    let csv_text = read_output(&out, "BrowserTriage_Output_Cookies.csv");
    let session = column(&csv_text, "Session Cookie");
    assert_eq!(session.iter().filter(|s| *s == "True").count(), 1);
    assert_eq!(session.iter().filter(|s| *s == "False").count(), 2);
}

/// Chrome 96+ moved Cookies into <profile>/Network/; the profile attribution
/// must be identical either way or one profile splits into two identities.
#[test]
fn the_network_subdirectory_does_not_change_the_profile() {
    let (_td, out) = setup(write_modern);
    let csv_text = read_output(&out, "BrowserTriage_Output_Cookies.csv");
    assert!(column(&csv_text, "Profile").iter().all(|p| p == "Default"));
}

/// Four instants per cookie, minus the ones that are unset.
#[test]
fn the_timeline_fans_out_one_row_per_cookie_timestamp() {
    let (_td, out) = setup(write_modern);
    let csv_text = read_output(&out, "BrowserTriage_Output_Timeline.csv");
    let kinds = column(&csv_text, "Timestamp Type");
    assert_eq!(kinds.iter().filter(|k| *k == "Cookie Created").count(), 3);
    assert_eq!(
        kinds
            .iter()
            .filter(|k| *k == "Cookie Last Accessed")
            .count(),
        3
    );
    assert_eq!(
        kinds.iter().filter(|k| *k == "Cookie Expires").count(),
        2,
        "the session cookie has no expiry and contributes no row"
    );
    assert_eq!(
        kinds.iter().filter(|k| *k == "Cookie Last Updated").count(),
        1,
        "only one row carries last_update_utc"
    );
}
