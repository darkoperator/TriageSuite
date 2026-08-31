//! Chromium `Bookmarks` — a JSON tree, not a database.
//!
//! This and `Preferences` are the artifacts a SQL-map engine cannot read at
//! all, so they are a large part of why this crate exists.
//!
//! The tree is walked with an explicit stack rather than recursion: bookmark
//! trees are user-controlled and arbitrarily deep, and a recursive walk over a
//! pathological one would overflow the stack rather than produce output.

use crate::json;
use crate::profile::BrowserId;
use crate::records::BookmarkRecord;
use crate::sql::Notes;
use crate::timeline::{artifact_name, kind, Timeline};
use serde_json::Value;
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;

/// Depth cap. Far beyond any real bookmark tree, and a backstop against a
/// crafted one; hitting it is recorded rather than silently truncating.
const MAX_DEPTH: i64 = 256;

struct Pending {
    node: Value,
    root: String,
    folder_path: String,
    depth: i64,
    position: Option<i64>,
}

pub fn parse(
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    let text = std::fs::read_to_string(path).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    let document: Value = serde_json::from_str(&text).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: format!("Bookmarks: {e}"),
    })?;

    let Some(roots) = document.get("roots").and_then(Value::as_object) else {
        // A valid JSON document that is not a bookmarks file.
        return Ok(0);
    };

    let source = path.display().to_string();
    let mut written = 0u64;
    let mut stack: Vec<Pending> = Vec::new();

    // Reverse so the first root is processed first once popped.
    for (root_name, root_node) in roots.iter().rev() {
        // `roots` also carries scalar bookkeeping keys such as
        // `sync_transaction_version`; only objects are nodes.
        if !root_node.is_object() {
            continue;
        }
        stack.push(Pending {
            node: root_node.clone(),
            root: root_name.clone(),
            folder_path: String::new(),
            depth: 0,
            position: None,
        });
    }

    while let Some(item) = stack.pop() {
        let node = &item.node;
        let title = json::text(node, "name");
        let url = json::text(node, "url");
        let raw_type = json::text(node, "type");
        let node_type = match raw_type.as_str() {
            "url" => "URL",
            "folder" => "Folder",
            "" => {
                if url.is_empty() {
                    "Folder"
                } else {
                    "URL"
                }
            }
            other => other,
        };

        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }
        notes.note_if_lossy("Title", &title);

        let date_added =
            WinTimestamp::from_webkit_micros(json::int(node, "date_added").unwrap_or_default());
        let date_modified =
            WinTimestamp::from_webkit_micros(json::int(node, "date_modified").unwrap_or_default());
        let date_last_used =
            WinTimestamp::from_webkit_micros(json::int(node, "date_last_used").unwrap_or_default());

        if item.depth >= MAX_DEPTH {
            notes.push(format!(
                "bookmark tree deeper than {MAX_DEPTH}; children below this node were not walked"
            ));
        }

        // The root nodes themselves are containers, not bookmarks the user
        // made, so they seed the folder path without producing a row.
        if item.depth > 0 {
            let label = if url.is_empty() {
                format!("{}/{}", item.folder_path, title)
            } else {
                url.clone()
            };

            out.write(
                "bookmarks",
                &BookmarkRecord {
                    browser: id.browser.clone(),
                    channel: id.channel.clone(),
                    profile: id.profile.clone(),
                    node_type: node_type.to_string(),
                    root: item.root.clone(),
                    folder_path: item.folder_path.clone(),
                    title: title.clone(),
                    url: url.clone(),
                    date_added,
                    date_modified,
                    date_last_used,
                    guid: json::text(node, "guid"),
                    bookmark_id: json::int(node, "id"),
                    parent_id: None,
                    position: item.position,
                    depth: Some(item.depth),
                    keyword: String::new(),
                    notes: notes.into_string(),
                    source_file: source.clone(),
                },
            )?;
            written += 1;

            for (timestamp, what) in [
                (date_added, kind::BOOKMARK_ADDED),
                (date_modified, kind::BOOKMARK_MODIFIED),
                (date_last_used, kind::BOOKMARK_LAST_USED),
            ] {
                timeline.push(out, timestamp, what, artifact_name::BOOKMARKS, &label)?;
            }
        }

        if item.depth >= MAX_DEPTH {
            continue;
        }

        let children = json::array(node, "children");
        let child_path = if item.depth == 0 {
            String::new()
        } else if item.folder_path.is_empty() {
            title
        } else {
            format!("{}/{}", item.folder_path, title)
        };
        // Reverse so siblings come out in their stored order.
        for (index, child) in children.iter().enumerate().rev() {
            if !child.is_object() {
                continue;
            }
            stack.push(Pending {
                node: child.clone(),
                root: item.root.clone(),
                folder_path: child_path.clone(),
                depth: item.depth + 1,
                position: Some(index as i64),
            });
        }
    }

    Ok(written)
}
