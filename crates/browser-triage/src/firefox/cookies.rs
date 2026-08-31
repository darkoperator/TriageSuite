//! Firefox `cookies.sqlite` -> `moz_cookies`.
//!
//! The value is stored in the clear here, which is the sharpest capability
//! difference between the two families and the reason the `Value Encrypted`
//! column exists at all.
//!
//! One trap: `moz_cookies` mixes units inside a single table. `creationTime`
//! and `lastAccessed` are PRTime **microseconds**, while `expiry` is unix
//! **seconds**. Reading `expiry` as microseconds would put every expiry in
//! 1970.

use crate::profile::BrowserId;
use crate::records::CookieRecord;
use crate::sql::{self, Notes};
use crate::timeline::{artifact_name, kind, Timeline};
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_sqlite::Database;

const SAME_SITE: &[(i64, &str)] = &[(0, "None"), (1, "Lax"), (2, "Strict")];

pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    if !db.table_exists("moz_cookies").unwrap_or(false) {
        return Ok(0);
    }
    let cols = sql::columns(db, "moz_cookies");
    let source = path.display().to_string();
    let mut written = 0u64;

    let sql_text = format!(
        "SELECT host, name, path, value, creationTime, lastAccessed, expiry, \
                {secure}, {http_only}, {same_site}, {origin_attributes} \
         FROM moz_cookies ORDER BY creationTime",
        secure = sql::alternatives(&cols, &["isSecure"], None),
        http_only = sql::alternatives(&cols, &["isHttpOnly"], None),
        same_site = sql::alternatives(&cols, &["sameSite"], None),
        origin_attributes = sql::alternatives(&cols, &["originAttributes"], None),
    );
    let rows = db.query(&sql_text).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: format!("moz_cookies: {e}"),
    })?;

    for row in &rows {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }

        let host = sql::text(sql::cell(row, 0));
        let name = sql::text(sql::cell(row, 1));
        let value = sql::text(sql::cell(row, 3));
        notes.note_if_lossy("Host", &host);
        notes.note_if_lossy("Value", &value);

        let created =
            WinTimestamp::from_unix_micros(sql::int(sql::cell(row, 4)).unwrap_or_default());
        let last_accessed =
            WinTimestamp::from_unix_micros(sql::int(sql::cell(row, 5)).unwrap_or_default());
        // Seconds, not microseconds. See the module docs.
        let expiry_secs = sql::int(sql::cell(row, 6)).unwrap_or_default();
        let expires = if expiry_secs == 0 {
            WinTimestamp::none()
        } else {
            WinTimestamp::from_unix(expiry_secs)
        };

        let label = format!("{host} {name}");
        out.write(
            "cookies",
            &CookieRecord {
                browser: id.browser.clone(),
                channel: id.channel.clone(),
                profile: id.profile.clone(),
                host: host.clone(),
                name: name.clone(),
                path: sql::text(sql::cell(row, 2)),
                value_length: Some(value.len() as i64),
                value,
                // Firefox stores cookie values in the clear.
                value_encrypted: "False".to_string(),
                encryption_scheme: String::new(),
                created,
                last_accessed,
                last_updated: WinTimestamp::none(),
                expires,
                // Firefox marks a session cookie with expiry = 0.
                session_cookie: if expiry_secs == 0 { "True" } else { "False" }.to_string(),
                secure: sql::bool_str(sql::cell(row, 7)),
                http_only: sql::bool_str(sql::cell(row, 8)),
                same_site: super::decode(SAME_SITE, sql::int(sql::cell(row, 9))),
                priority: String::new(),
                source_scheme: String::new(),
                source_port: None,
                top_frame_site: String::new(),
                origin_attributes: sql::text(sql::cell(row, 10)),
                notes: notes.into_string(),
                source_file: source.clone(),
            },
        )?;
        written += 1;

        for (timestamp, what) in [
            (created, kind::COOKIE_CREATED),
            (last_accessed, kind::COOKIE_LAST_ACCESSED),
            (expires, kind::COOKIE_EXPIRES),
        ] {
            timeline.push(out, timestamp, what, artifact_name::COOKIES, &label)?;
        }
    }

    Ok(written)
}
