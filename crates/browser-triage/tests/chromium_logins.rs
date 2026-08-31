//! Chromium `Login Data` -> `logins`.
//!
//! The load-bearing test here is the negative one: no password material may
//! reach the output in any encoding. Everything else is ordinary metadata.

#![cfg(unix)]

mod support;

use rusqlite::Connection;
use std::path::Path;
use support::{column, profile_dir, read_output, rows, run};
use tempfile::TempDir;

/// A recognizable needle to grep the whole output for.
const SECRET: &str = "SUPERSECRETPASSWORD";

fn write_login_data(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(&format!(
        "CREATE TABLE logins(origin_url TEXT, action_url TEXT, username_element TEXT,
             username_value TEXT, password_element TEXT, password_value BLOB,
             signon_realm TEXT, date_created INTEGER, blacklisted_by_user INTEGER,
             scheme INTEGER, times_used INTEGER, display_name TEXT, federation_url TEXT,
             id INTEGER PRIMARY KEY, date_last_used INTEGER,
             date_password_modified INTEGER, date_received INTEGER);

         INSERT INTO logins VALUES
           ('https://mail.test/login','https://mail.test/auth','user',
            'alice@mail.test','pass','v10{SECRET}','https://mail.test/',
            13344473600000000, 0, 0, 7, 'Alice', '', 1,
            13344473700000000, 13344473650000000, 0),
           -- A never-save entry: no credential at all.
           ('https://blocked.test/','','','','', X'', 'https://blocked.test/',
            13344473800000000, 1, 0, 0, '', '', 2, 0, 0, 0),
           -- A pre-M55 profile writing date_created as a time_t.
           ('https://old.test/','','','bob','', X'', 'https://old.test/',
            1700000000, 0, 0, 1, '', '', 3, 0, 0, 0);"
    ))
    .unwrap();
}

fn setup() -> (TempDir, std::path::PathBuf) {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    write_login_data(&dir.join("Login Data"));
    let out = td.path().join("out");
    run(td.path(), &out);
    (td, out)
}

/// The one that matters. Every file the run produced is checked for the
/// password in plain, hex and base64 form.
#[test]
fn no_password_material_appears_anywhere_in_the_output() {
    let (_td, out) = setup();

    let hex: String = SECRET.bytes().map(|b| format!("{b:02x}")).collect();
    // Base64 of the needle, computed inline to avoid a dependency.
    let base64 = {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes = SECRET.as_bytes();
        let mut encoded = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    encoded.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3F) as usize] as char);
                }
            }
        }
        encoded
    };

    for entry in std::fs::read_dir(&out).unwrap().flatten() {
        let body = std::fs::read_to_string(entry.path()).unwrap_or_default();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(!body.contains(SECRET), "plaintext password in {name}");
        assert!(!body.contains(&hex), "hex-encoded password in {name}");
        assert!(!body.contains(&base64), "base64 password in {name}");
    }
}

#[test]
fn every_login_reaches_the_output() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Logins.csv");
    assert_eq!(rows(&csv_text).len(), 3, "{csv_text}");
}

/// The username is ordinary attribution evidence and is emitted; the password
/// is described but never shown.
#[test]
fn the_username_is_emitted_while_the_password_is_only_described() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Logins.csv");
    assert!(column(&csv_text, "Username")
        .iter()
        .any(|u| u == "alice@mail.test"));
    assert!(column(&csv_text, "Password Present")
        .iter()
        .any(|p| p == "True"));
    assert!(column(&csv_text, "Password Encryption")
        .iter()
        .any(|e| e == "v10"));
    assert!(column(&csv_text, "Password Length")
        .iter()
        .any(|l| l == "22"));
    // Chromium keeps usernames in the clear, unlike Firefox.
    assert!(column(&csv_text, "Username Encrypted")
        .iter()
        .all(|f| f == "False"));
}

/// A "never save for this site" entry carries no credential, and saying so is
/// different from saying the password was empty.
#[test]
fn a_blocklisted_entry_is_marked_rather_than_dropped() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Logins.csv");
    let line = csv_text
        .lines()
        .find(|l| l.contains("blocked.test"))
        .expect("the blocklist entry must be present");
    assert!(line.contains("True"), "Blocklisted should be True: {line}");
}

/// Pre-M55 profiles wrote a time_t here. Reading it as WebKit would put the row
/// in 1601, so the fallback fires and says that it did.
#[test]
fn a_legacy_time_t_date_created_is_converted_and_noted() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Logins.csv");
    let line = csv_text
        .lines()
        .find(|l| l.contains("old.test"))
        .expect("the legacy row must be present");
    assert!(line.contains("2023-11-14T22:13:20"), "{line}");
    assert!(line.contains("legacy time_t"), "{line}");
}

#[test]
fn the_timeline_fans_out_one_row_per_login_timestamp() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Timeline.csv");
    let kinds = column(&csv_text, "Timestamp Type");
    assert_eq!(kinds.iter().filter(|k| *k == "Login Created").count(), 3);
    assert_eq!(kinds.iter().filter(|k| *k == "Login Last Used").count(), 1);
    assert_eq!(kinds.iter().filter(|k| *k == "Password Changed").count(), 1);
    assert!(column(&csv_text, "Value")
        .iter()
        .any(|v| v == "https://mail.test/login (alice@mail.test)"));
}
