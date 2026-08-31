//! Firefox `places.sqlite` -> `moz_bookmarks` joined to `moz_places`.
//!
//! Unlike Chromium's JSON tree the structure is relational, so the folder path
//! is reconstructed by walking `parent` upward. That walk carries a visited-set
//! guard: a corrupted database can contain a parent cycle, and following one
//! would hang the run.

use crate::profile::BrowserId;
use crate::records::BookmarkRecord;
use crate::sql::{self, Notes};
use crate::timeline::{artifact_name, kind, Timeline};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_sqlite::Database;

/// One row's parent-chain context, resolved once up front.
struct Node {
    parent: Option<i64>,
    title: String,
    guid: String,
}

/// Walk `parent` upward to build the `/`-joined folder path and name the root.
///
/// Returns the path, the root's friendly name, the depth, and whether a cycle
/// was hit. A cycle is reported rather than silently truncating the path.
///
/// Firefox's hierarchy is `root________` containing `toolbar`, `menu`,
/// `unfiled`, `mobile` and `tags`, so the *useful* root is the one below the
/// top — reporting "root" for everything would say nothing. The outermost
/// non-`root` named ancestor wins, falling back to "root" only when a bookmark
/// really does hang directly off the top.
fn ancestry(nodes: &HashMap<i64, Node>, start: Option<i64>) -> (String, String, i64, bool) {
    let mut titles: Vec<String> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    let mut root = String::new();
    let mut cursor = start;
    let mut depth = 0i64;
    let mut cycle = false;

    while let Some(current) = cursor {
        if !seen.insert(current) {
            cycle = true;
            break;
        }
        let Some(node) = nodes.get(&current) else {
            break;
        };
        depth += 1;

        let named = super::root_name(&node.guid);
        if !named.is_empty() && (named != "root" || root.is_empty()) {
            root = named.to_string();
        }
        // A root's own title is empty and is never part of the path.
        if !node.title.is_empty() && named.is_empty() {
            titles.push(node.title.clone());
        }
        cursor = node.parent;
    }

    (
        titles.into_iter().rev().collect::<Vec<_>>().join("/"),
        root,
        depth,
        cycle,
    )
}

pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    if !db.table_exists("moz_bookmarks").unwrap_or(false) {
        return Ok(0);
    }
    let cols = sql::columns(db, "moz_bookmarks");
    let source = path.display().to_string();
    let mut written = 0u64;

    // Every node first, so the parent chain can be resolved without a query
    // per row.
    let mut nodes: HashMap<i64, Node> = HashMap::new();
    if let Ok(rows) = db.query("SELECT id, parent, title, guid FROM moz_bookmarks") {
        for row in &rows {
            if let Some(node_id) = sql::int(sql::cell(row, 0)) {
                nodes.insert(
                    node_id,
                    Node {
                        parent: sql::int(sql::cell(row, 1)).filter(|p| *p != 0),
                        title: sql::text(sql::cell(row, 2)),
                        guid: sql::text(sql::cell(row, 3)),
                    },
                );
            }
        }
    }

    let has_keywords = db.table_exists("moz_keywords").unwrap_or(false);
    let keyword_join = if has_keywords {
        "LEFT JOIN moz_keywords k ON k.id = b.keyword_id"
    } else {
        ""
    };
    let keyword_column = if has_keywords { "k.keyword" } else { "NULL" };

    let sql_text = format!(
        "SELECT b.id, b.parent, b.type, b.title, b.position, {date_added}, {last_modified}, \
                b.guid, p.url, {keyword_column} \
         FROM moz_bookmarks b LEFT JOIN moz_places p ON p.id = b.fk {keyword_join} \
         ORDER BY b.id",
        date_added = sql::alternatives(&cols, &["dateAdded"], Some("b")),
        last_modified = sql::alternatives(&cols, &["lastModified"], Some("b")),
    );
    let rows = db.query(&sql_text).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: format!("moz_bookmarks: {e}"),
    })?;

    for row in &rows {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }

        let node_id = sql::int(sql::cell(row, 0));
        let parent_id = sql::int(sql::cell(row, 1)).filter(|p| *p != 0);
        let title = sql::text(sql::cell(row, 3));
        let url = sql::text(sql::cell(row, 8));
        notes.note_if_lossy("Title", &title);

        let guid = sql::text(sql::cell(row, 7));
        // A root is a container, not a bookmark the user made.
        if !super::root_name(&guid).is_empty() {
            continue;
        }

        let (folder_path, root, depth, cycle) = ancestry(&nodes, parent_id);
        if cycle {
            notes.push(format!(
                "bookmark parent cycle detected at id {}",
                node_id.unwrap_or_default()
            ));
        }

        let date_added =
            WinTimestamp::from_unix_micros(sql::int(sql::cell(row, 5)).unwrap_or_default());
        let date_modified =
            WinTimestamp::from_unix_micros(sql::int(sql::cell(row, 6)).unwrap_or_default());

        let label = if url.is_empty() {
            format!("{folder_path}/{title}")
        } else {
            url.clone()
        };

        out.write(
            "bookmarks",
            &BookmarkRecord {
                browser: id.browser.clone(),
                channel: id.channel.clone(),
                profile: id.profile.clone(),
                node_type: super::bookmark_type(sql::int(sql::cell(row, 2))),
                root,
                folder_path,
                title,
                url,
                date_added,
                date_modified,
                date_last_used: WinTimestamp::none(),
                guid,
                bookmark_id: node_id,
                parent_id,
                position: sql::int(sql::cell(row, 4)),
                depth: Some(depth),
                keyword: sql::text(sql::cell(row, 9)),
                notes: notes.into_string(),
                source_file: source.clone(),
            },
        )?;
        written += 1;

        for (timestamp, what) in [
            (date_added, kind::BOOKMARK_ADDED),
            (date_modified, kind::BOOKMARK_MODIFIED),
        ] {
            timeline.push(out, timestamp, what, artifact_name::BOOKMARKS, &label)?;
        }
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(parent: Option<i64>, title: &str, guid: &str) -> Node {
        Node {
            parent,
            title: title.to_string(),
            guid: guid.to_string(),
        }
    }

    #[test]
    fn the_folder_path_is_built_from_the_parent_chain() {
        let mut nodes = HashMap::new();
        nodes.insert(1, node(None, "", "root________"));
        nodes.insert(2, node(Some(1), "", "toolbar_____"));
        nodes.insert(3, node(Some(2), "Tools", "aaaa"));
        let (path, root, depth, cycle) = ancestry(&nodes, Some(3));
        assert_eq!(path, "Tools");
        assert_eq!(
            root, "toolbar",
            "the useful root is the one below root________, not root itself"
        );
        assert_eq!(depth, 3);
        assert!(!cycle);
    }

    /// A bookmark hanging directly off the top still gets a root name.
    #[test]
    fn a_bookmark_directly_under_the_top_root_reports_root() {
        let mut nodes = HashMap::new();
        nodes.insert(1, node(None, "", "root________"));
        let (_, root, _, _) = ancestry(&nodes, Some(1));
        assert_eq!(root, "root");
    }

    /// A corrupted database can contain a parent cycle; following one would
    /// hang the run rather than produce output.
    #[test]
    fn a_parent_cycle_terminates_and_is_reported() {
        let mut nodes = HashMap::new();
        nodes.insert(1, node(Some(2), "A", "aaaa"));
        nodes.insert(2, node(Some(1), "B", "bbbb"));
        let (_, _, _, cycle) = ancestry(&nodes, Some(1));
        assert!(cycle, "the cycle must be detected, not followed");
    }

    #[test]
    fn an_unknown_parent_stops_the_walk_without_failing() {
        let nodes = HashMap::new();
        let (path, _, depth, cycle) = ancestry(&nodes, Some(42));
        assert_eq!(path, "");
        assert_eq!(depth, 0);
        assert!(!cycle);
    }
}
