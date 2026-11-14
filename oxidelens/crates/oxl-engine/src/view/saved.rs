//! Saved views and filter presets, kept in the case (`ox.saved_view`).
//!
//! A view is a named document holding what a grid shows: chips, the search
//! box, hosts, grouping, sort, column layout and colour rules, with the
//! pattern of relation names it is offered for. A preset is a view that
//! carries chips only. Saving under a name and kind already in use replaces
//! that view; deleting one leaves a tombstone, and every change has a history
//! row.
//!
//! A view document is untrusted twice over: the app sends one to save, and
//! an imported view file comes from anywhere. Both are read with unknown
//! fields refused and pass the same checks before anything is written: the
//! size, the chips through the grid state's own check, every filter through
//! `check_caps`, the colour rules through their own check.
//!
//! Applied to a dataset, a view is checked against that dataset's columns,
//! not refused for them: a chip naming a column the dataset lacks comes back
//! switched off, and a sort key, group column or colour rule naming one is
//! dropped, each with a warning naming the column, while the rest applies.

use super::rules::check_rules;
use super::{shown, Chip, ColourRule, GridState, SearchState};
use crate::actor::Actor;
use crate::annot::{record, Change};
use crate::case::{Case, CASE_FORMAT};
use crate::catalog;
use crate::error::{EngineError, Result};
use crate::filter::{FilterNode, MARK_COLUMN};
use crate::listing::{check_limit, Listing};
use crate::query::{check_offset, resolve_hosts, SortKey};
use crate::search::{parse_search, SearchOptions};
use crate::time::now_iso;
use duckdb::{params, Connection};
use std::collections::{BTreeMap, BTreeSet};

/// The largest view document, encoded as compact JSON, as it is stored:
/// 256 KiB. Save and import apply the same rule.
pub const MAX_VIEW_BYTES: usize = 256 * 1024;
/// The largest view file an import reads, checked before it is parsed:
/// 1 MiB. Export keeps every file within it (see `export_view`), so any
/// view that saves exports to a file that imports; the document inside is
/// then held to `MAX_VIEW_BYTES` like a save.
pub const MAX_VIEW_FILE_BYTES: usize = 1024 * 1024;
/// Characters in a view's name.
pub const MAX_VIEW_NAME: usize = 128;
/// Characters in a view's relation pattern.
pub const MAX_VIEW_PATTERN: usize = 128;
/// The narrowest and widest column a layout keeps, in pixels.
pub const MIN_COLUMN_WIDTH: u32 = 40;
pub const MAX_COLUMN_WIDTH: u32 = 2000;

/// The version an exported view file carries in `oxidelens_view`.
const VIEW_FILE_VERSION: u32 = 1;

/// How a grid's columns are laid out: their order, widths in pixels, and
/// which are hidden and pinned. Column names are display text, matched
/// against the dataset's when the view is applied.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default, deny_unknown_fields)]
pub struct ColumnLayout {
    pub order: Vec<String>,
    /// `MIN_COLUMN_WIDTH` to `MAX_COLUMN_WIDTH` each.
    pub widths: BTreeMap<String, u32>,
    pub hidden: Vec<String>,
    pub pinned: Vec<String>,
}

/// What a saved view holds. Every part may be left out of the JSON, and is
/// then empty.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default, deny_unknown_fields)]
pub struct ViewDoc {
    pub chips: Vec<Chip>,
    pub search: SearchState,
    pub hosts: Vec<String>,
    pub group_by: Vec<String>,
    pub sort: Vec<SortKey>,
    pub layout: Option<ColumnLayout>,
    pub colour_rules: Vec<ColourRule>,
}

/// A whole view, or a preset: chips only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    View,
    Preset,
}

impl ViewKind {
    /// The text `ox.saved_view.kind` holds.
    fn as_str(self) -> &'static str {
        match self {
            ViewKind::View => "view",
            ViewKind::Preset => "preset",
        }
    }
}

/// One saved view or preset as the case holds it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SavedView {
    pub id: i64,
    pub name: String,
    pub kind: ViewKind,
    /// The relations it is offered for: a relation name in which `*` stands
    /// for any run of characters, matched without regard to ASCII case.
    pub pattern: String,
    pub doc: ViewDoc,
    pub created_by: String,
    pub created_at: String,
    pub updated_by: String,
    pub updated_at: String,
}

impl SavedView {
    /// The view as its history rows record it.
    fn json(&self, deleted: bool) -> serde_json::Value {
        serde_json::json!({
            "id": self.id, "name": self.name, "kind": self.kind.as_str(),
            "pattern": self.pattern, "doc": self.doc,
            "created_by": self.created_by, "created_at": self.created_at,
            "updated_by": self.updated_by, "updated_at": self.updated_at,
            "deleted": deleted,
        })
    }
}

/// A saved view a listing left out because its stored row does not decode
/// (a document or kind the engine never writes: the case was edited by
/// hand, or a later engine wrote it), and why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SkippedView {
    pub id: i64,
    pub reason: String,
}

/// Up to a page of saved views, whether more follow, and the views left out
/// because they do not decode. A skipped view takes no place in the page:
/// `skipped` names every one the listing would have considered, on every
/// page, so a caller can report them whichever page it reads.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ViewListing<T> {
    pub items: Vec<T>,
    /// More items exist past this page.
    pub truncated: bool,
    pub skipped: Vec<SkippedView>,
}

impl<T> ViewListing<T> {
    fn new(page: Listing<T>, skipped: Vec<SkippedView>) -> ViewListing<T> {
        ViewListing {
            items: page.items,
            truncated: page.truncated,
            skipped,
        }
    }
}

/// Part of a view that could not apply to a dataset, and why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ViewWarning {
    /// `chip:<id>`, `sort`, `group_by`, `colour_rule:<id>`, `host` or
    /// `layout`.
    pub part: String,
    /// The column or host the dataset or case lacks; for `layout`, every
    /// such column, comma-separated.
    pub name: String,
    pub message: String,
}

/// A view as it applies to one dataset.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AppliedView {
    /// The chips (switched off where they name a missing column), search,
    /// hosts and sort, over the dataset; every column shown.
    pub state: GridState,
    pub group_by: Vec<String>,
    pub layout: Option<ColumnLayout>,
    /// The view's colour rules, in order; switched off where they name a
    /// missing column.
    pub colour_rules: Vec<ColourRule>,
    pub warnings: Vec<ViewWarning>,
}

/// An exported view file.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewFile {
    oxidelens_view: u32,
    name: String,
    kind: ViewKind,
    pattern: String,
    doc: ViewDoc,
}

/// Save `doc` as a view or preset named `name`, offered for the relations
/// `pattern` matches. A view of the same name and kind that is not deleted
/// is replaced (history `view_update`), keeping its id and who saved it
/// first; otherwise a new one is saved (`view_save`).
///
/// `Invalid` for a name that is not 1 to `MAX_VIEW_NAME` characters or holds
/// a control character; a pattern that is not 1 to `MAX_VIEW_PATTERN` of
/// `[A-Za-z0-9_*-]`; a preset with anything but chips; more than
/// `MAX_CHIPS` chips or a bad chip; more than `MAX_RULES` colour rules or a
/// bad rule; a filter over the caps; a search over its caps; a layout width
/// outside `MIN_COLUMN_WIDTH..=MAX_COLUMN_WIDTH`; or a document over
/// `MAX_VIEW_BYTES` encoded. `Refused` for an agent replacing an analyst's
/// view, and for a format-1 case opened read-only. Nothing is written when
/// any check fails.
pub fn save_view(
    case: &mut Case,
    name: &str,
    kind: ViewKind,
    pattern: &str,
    doc: &ViewDoc,
    actor: &Actor,
) -> Result<SavedView> {
    check_name(name)?;
    check_pattern(pattern)?;
    let doc_json = check_doc(kind, doc)?;
    check_writable(case)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let existing = find_by_name(&tx, name, kind)?;
    let (saved, change) = match existing {
        Some(before) => {
            if actor.is_agent() && is_analysts(&before) {
                return Err(EngineError::Refused(format!(
                    "{} {} is an analyst's; an agent may not replace it",
                    kind.as_str(),
                    shown(name)
                )));
            }
            tx.execute(
                "UPDATE ox.saved_view SET pattern = ?, doc_json = ?, updated_by = ?, \
                 updated_at = ? WHERE id = ?",
                params![pattern, doc_json, actor.label(), at, before.id],
            )?;
            let after = SavedView {
                pattern: pattern.to_string(),
                doc: doc.clone(),
                updated_by: actor.label(),
                updated_at: at.clone(),
                ..before.clone()
            };
            let change = change_of(
                after.id,
                "view_update",
                Some(before.json(false)),
                Some(after.json(false)),
            );
            (after, change)
        }
        None => {
            let id: i64 = tx.query_row(
                "INSERT INTO ox.saved_view (id, name, kind, pattern, doc_json, created_by, \
                 created_at, updated_by, updated_at, deleted) \
                 VALUES (nextval('ox.view_seq'), ?, ?, ?, ?, ?, ?, ?, ?, false) RETURNING id",
                params![
                    name,
                    kind.as_str(),
                    pattern,
                    doc_json,
                    actor.label(),
                    at,
                    actor.label(),
                    at
                ],
                |r| r.get(0),
            )?;
            let saved = SavedView {
                id,
                name: name.to_string(),
                kind,
                pattern: pattern.to_string(),
                doc: doc.clone(),
                created_by: actor.label(),
                created_at: at.clone(),
                updated_by: actor.label(),
                updated_at: at.clone(),
            };
            let change = change_of(id, "view_save", None, Some(saved.json(false)));
            (saved, change)
        }
    };
    record(&tx, actor, &at, change)?;
    tx.commit()?;
    Ok(saved)
}

/// Up to `limit` of the views and presets that are not deleted, from
/// `offset`: views first, then presets, each by name. With a relation, only
/// those whose pattern matches it. A view that does not decode is left out
/// and named in `skipped`, whatever the relation (its pattern is unread with
/// the rest of it). A format-1 case has none. `Invalid` for a `limit` of 0
/// or over `MAX_LIMIT`, or an `offset` past `i64::MAX`.
pub fn views(
    case: &Case,
    relation: Option<&str>,
    limit: u32,
    offset: u64,
) -> Result<ViewListing<SavedView>> {
    check_limit(limit)?;
    check_offset(offset)?;
    if case.format() < CASE_FORMAT {
        return Ok(ViewListing::new(
            Listing::from_fetched(Vec::new(), limit),
            Vec::new(),
        ));
    }
    // The pattern is matched here rather than in SQL, where `_` is a
    // wildcard and case folding is not only ASCII's; the page is cut after.
    let (all, skipped) = read_views_skipping(case.conn())?;
    let skip = usize::try_from(offset).unwrap_or(usize::MAX);
    let fetched: Vec<SavedView> = all
        .into_iter()
        .filter(|v| relation.is_none_or(|r| pattern_matches(&v.pattern, r)))
        .skip(skip)
        .take(limit as usize + 1)
        .collect();
    Ok(ViewListing::new(
        Listing::from_fetched(fetched, limit),
        skipped,
    ))
}

/// A saved view with the case's relations its pattern matches.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MatchingView {
    pub view: SavedView,
    /// The case's active relations the view's pattern matches, sorted by
    /// name; never empty.
    pub relations: Vec<String>,
}

/// Up to `limit` of the views and presets that are not deleted and whose
/// pattern matches at least one active relation of the case, from `offset`,
/// each with the relations it matches (sorted). Ordered as `views` orders
/// them; the page is cut after the matching, so a view matching nothing never
/// uses up a place. A view that does not decode is left out and named in
/// `skipped`, as `views` does. A format-1 case has none. `Invalid` for a
/// `limit` of 0 or over `MAX_LIMIT`, or an `offset` past `i64::MAX`.
pub fn views_matching(case: &Case, limit: u32, offset: u64) -> Result<ViewListing<MatchingView>> {
    check_limit(limit)?;
    check_offset(offset)?;
    if case.format() < CASE_FORMAT {
        return Ok(ViewListing::new(
            Listing::from_fetched(Vec::new(), limit),
            Vec::new(),
        ));
    }
    let relations: Vec<String> = catalog::relations(case)?
        .into_iter()
        .map(|r| r.name)
        .collect();
    let (all, skipped) = read_views_skipping(case.conn())?;
    let skip = usize::try_from(offset).unwrap_or(usize::MAX);
    let fetched: Vec<MatchingView> = all
        .into_iter()
        .filter_map(|view| {
            let matched: Vec<String> = relations
                .iter()
                .filter(|r| pattern_matches(&view.pattern, r))
                .cloned()
                .collect();
            (!matched.is_empty()).then_some(MatchingView {
                view,
                relations: matched,
            })
        })
        .skip(skip)
        .take(limit as usize + 1)
        .collect();
    Ok(ViewListing::new(
        Listing::from_fetched(fetched, limit),
        skipped,
    ))
}

/// Delete view `id`, leaving a tombstone (history `view_delete`).
/// `NotFound` for an unknown id; `Refused` for a view already deleted, an
/// agent deleting an analyst's view, or a format-1 case opened read-only.
pub fn delete_view(case: &mut Case, id: i64, actor: &Actor) -> Result<()> {
    check_writable(case)?;
    let tx = case.conn_mut().transaction()?;
    let (view, deleted) = read_view(&tx, id)?;
    if deleted {
        return Err(EngineError::Refused(format!(
            "saved view {id} was already deleted"
        )));
    }
    if actor.is_agent() && is_analysts(&view) {
        return Err(EngineError::Refused(format!(
            "saved view {id} is an analyst's; an agent may not delete it"
        )));
    }
    tx.execute(
        "UPDATE ox.saved_view SET deleted = true WHERE id = ?",
        params![id],
    )?;
    let change = change_of(
        id,
        "view_delete",
        Some(view.json(false)),
        Some(view.json(true)),
    );
    record(&tx, actor, &now_iso(), change)?;
    tx.commit()?;
    Ok(())
}

/// View `id` as it applies to `relation`. A chip or colour rule whose filter
/// names a column the relation lacks comes back switched off (kept, so the
/// analyst sees it); a sort key or group column naming one is dropped;
/// layout entries naming one are dropped
/// and counted in one `layout` warning; a host the case lacks is dropped, and
/// every host when the relation has no host column. Each has a warning.
/// `NotFound` for a view that is unknown or deleted, and for an unknown
/// relation.
pub fn apply_view(case: &Case, id: i64, relation: &str) -> Result<AppliedView> {
    let view = live_view(case, id)?;
    let info = catalog::relation(case, relation)?;
    let known: BTreeSet<String> = catalog::columns(case, relation)?
        .into_iter()
        .map(|c| c.name)
        .collect();
    let has = |column: &str| known.contains(column);
    let lacks = |column: &str| format!("{relation} has no column {}", shown(column));
    let doc = view.doc;
    let mut warnings = Vec::new();

    let mut chips = doc.chips;
    for chip in &mut chips {
        let mut missing = Vec::new();
        filter_columns(&chip.filter, &mut missing);
        missing.retain(|c| !has(c));
        if !missing.is_empty() {
            chip.enabled = false;
        }
        for column in missing {
            warnings.push(ViewWarning {
                part: format!("chip:{}", chip.id),
                message: format!("{}; the chip is switched off", lacks(&column)),
                name: column,
            });
        }
    }

    let mut sort = doc.sort;
    sort.retain(|key| {
        let keep = has(&key.column);
        if !keep {
            warnings.push(ViewWarning {
                part: "sort".to_string(),
                name: key.column.clone(),
                message: format!("{}; the sort key is dropped", lacks(&key.column)),
            });
        }
        keep
    });

    let mut group_by = doc.group_by;
    group_by.retain(|column| {
        let keep = has(column);
        if !keep {
            warnings.push(ViewWarning {
                part: "group_by".to_string(),
                name: column.clone(),
                message: format!("{}; the grouping by it is dropped", lacks(column)),
            });
        }
        keep
    });

    let mut colour_rules = doc.colour_rules;
    for rule in &mut colour_rules {
        let mut missing = Vec::new();
        filter_columns(&rule.filter, &mut missing);
        missing.retain(|c| !has(c));
        if !missing.is_empty() {
            rule.enabled = false;
        }
        for column in missing {
            warnings.push(ViewWarning {
                part: format!("colour_rule:{}", rule.id),
                message: format!("{}; the colour rule is switched off", lacks(&column)),
                name: column,
            });
        }
    }

    let mut hosts = Vec::new();
    for host in doc.hosts {
        let message = if info.host_column.is_none() {
            Some(format!(
                "{relation} has no host column; the host {} is dropped",
                shown(&host)
            ))
        } else {
            match resolve_hosts(case, std::slice::from_ref(&host)) {
                Ok(_) => None,
                Err(EngineError::NotFound(_)) => Some(format!(
                    "the case has no host {}; it is dropped",
                    shown(&host)
                )),
                Err(other) => return Err(other),
            }
        };
        match message {
            None => hosts.push(host),
            Some(message) => warnings.push(ViewWarning {
                part: "host".to_string(),
                name: host,
                message,
            }),
        }
    }

    let layout = doc.layout.map(|layout| {
        let (layout, dropped, missing) = keep_layout(layout, &has);
        if dropped > 0 {
            warnings.push(ViewWarning {
                part: "layout".to_string(),
                name: missing.into_iter().collect::<Vec<_>>().join(", "),
                message: format!(
                    "{dropped} layout entries name columns {relation} lacks; they are dropped"
                ),
            });
        }
        layout
    });

    Ok(AppliedView {
        state: GridState {
            relation: relation.to_string(),
            chips,
            search: doc.search,
            hosts,
            sort,
            columns: None,
        },
        group_by,
        layout,
        colour_rules,
        warnings,
    })
}

/// View `id` as a view file: `{"oxidelens_view": 1, "name", "kind",
/// "pattern", "doc"}`, pretty-printed. `NotFound` for a view that is unknown
/// or deleted.
///
/// Pretty-printing indents every array element on a line of its own, which
/// can multiply a document of long value lists several times over. When the
/// pretty text would pass `MAX_VIEW_FILE_BYTES` the file is written compact
/// instead: the stored document is at most `MAX_VIEW_BYTES` compact and the
/// wrapper (a name of at most 128 characters, a pattern of at most 128 ASCII
/// characters) adds little, so the compact file is always well within the
/// import cap and every export imports.
pub fn export_view(case: &Case, id: i64) -> Result<String> {
    let view = live_view(case, id)?;
    let file = ViewFile {
        oxidelens_view: VIEW_FILE_VERSION,
        name: view.name,
        kind: view.kind,
        pattern: view.pattern,
        doc: view.doc,
    };
    let encode_error = |e: serde_json::Error| {
        EngineError::Internal(format!("saved view {id} does not encode: {e}"))
    };
    let pretty = serde_json::to_string_pretty(&file).map_err(encode_error)?;
    if pretty.len() <= MAX_VIEW_FILE_BYTES {
        return Ok(pretty);
    }
    let compact = serde_json::to_string(&file).map_err(encode_error)?;
    if compact.len() > MAX_VIEW_FILE_BYTES {
        // The bound above says this cannot happen.
        return Err(EngineError::Internal(format!(
            "saved view {id} exports to {} bytes, over the {MAX_VIEW_FILE_BYTES} an import reads",
            compact.len()
        )));
    }
    Ok(compact)
}

/// Save the view in `json`, a view file from anywhere, as `save_view`
/// would, replacing one of the same name and kind. `Invalid` for a file over
/// `MAX_VIEW_FILE_BYTES`, refused before it is parsed; one that is not a
/// view file of version 1 (an unknown field anywhere, an unknown operator);
/// and for everything `save_view` refuses as `Invalid`, its document over
/// `MAX_VIEW_BYTES` compact among them. Nothing is written when any check
/// fails.
pub fn import_view(case: &mut Case, json: &str, actor: &Actor) -> Result<SavedView> {
    if json.len() > MAX_VIEW_FILE_BYTES {
        return Err(EngineError::Invalid(format!(
            "a view file is at most {MAX_VIEW_FILE_BYTES} bytes, not {}",
            json.len()
        )));
    }
    let file: ViewFile = serde_json::from_str(json)
        .map_err(|e| EngineError::Invalid(format!("not a view file: {e}")))?;
    if file.oxidelens_view != VIEW_FILE_VERSION {
        return Err(EngineError::Invalid(format!(
            "a view file of version {}; this build reads version {VIEW_FILE_VERSION}",
            file.oxidelens_view
        )));
    }
    save_view(case, &file.name, file.kind, &file.pattern, &file.doc, actor)
}

/// `Refused` for a format-1 case, which has no saved views and is opened
/// only read-only.
fn check_writable(case: &Case) -> Result<()> {
    if case.format() < CASE_FORMAT {
        return Err(EngineError::Refused(format!(
            "the case is format {}, opened read-only; open it to change it, which \
             upgrades it to format {CASE_FORMAT}",
            case.format()
        )));
    }
    Ok(())
}

fn check_name(name: &str) -> Result<()> {
    let length = name.chars().count();
    if !(1..=MAX_VIEW_NAME).contains(&length) {
        return Err(EngineError::Invalid(format!(
            "a view name is 1 to {MAX_VIEW_NAME} characters, not {length}"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(EngineError::Invalid(format!(
            "view name {} holds a control character",
            shown(name)
        )));
    }
    Ok(())
}

fn check_pattern(pattern: &str) -> Result<()> {
    let valid = (1..=MAX_VIEW_PATTERN).contains(&pattern.len())
        && pattern
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'*' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(EngineError::Invalid(format!(
            "view pattern {} is not 1 to {MAX_VIEW_PATTERN} of letters, digits, '_', '-' \
             and '*'",
            shown(pattern)
        )))
    }
}

/// Every check a document passes before it is saved, by the checks the grid
/// already runs on the same parts; the document encoded, as it is stored.
fn check_doc(kind: ViewKind, doc: &ViewDoc) -> Result<String> {
    if kind == ViewKind::Preset {
        let chips_only = doc.search == SearchState::default()
            && doc.hosts.is_empty()
            && doc.group_by.is_empty()
            && doc.sort.is_empty()
            && doc.layout.is_none()
            && doc.colour_rules.is_empty();
        if !chips_only {
            return Err(EngineError::Invalid(
                "a preset carries chips only: no search, hosts, grouping, sort, layout \
                 or colour rules"
                    .to_string(),
            ));
        }
    }
    let state = GridState {
        relation: String::new(),
        chips: doc.chips.clone(),
        search: doc.search.clone(),
        hosts: doc.hosts.clone(),
        sort: doc.sort.clone(),
        columns: None,
    };
    // The grid's own check: the count, ids, labels, and every chip's filter
    // on its own, switched on or off (caps and value shapes); then the
    // enabled chips together, as the grid compiles them.
    state.query()?;
    check_rules(&doc.colour_rules)?;
    let opts = SearchOptions {
        regex: doc.search.regex,
        case_sensitive: doc.search.case_sensitive,
    };
    parse_search(&doc.search.text, opts, &[])?;
    if let Some(layout) = &doc.layout {
        if let Some((column, width)) = layout
            .widths
            .iter()
            .find(|(_, w)| !(MIN_COLUMN_WIDTH..=MAX_COLUMN_WIDTH).contains(*w))
        {
            return Err(EngineError::Invalid(format!(
                "column {} is {width} px wide; a width is {MIN_COLUMN_WIDTH} to \
                 {MAX_COLUMN_WIDTH}",
                shown(column)
            )));
        }
    }
    // The filters passed the caps above, so encoding them is bounded.
    let json = serde_json::to_string(doc)
        .map_err(|e| EngineError::Internal(format!("a view document does not encode: {e}")))?;
    if json.len() > MAX_VIEW_BYTES {
        return Err(EngineError::Invalid(format!(
            "a view document is at most {MAX_VIEW_BYTES} bytes encoded, not {}",
            json.len()
        )));
    }
    Ok(json)
}

/// Whether an analyst saved `view`, first or last: then it is an analyst's
/// work, which an agent neither replaces nor deletes.
fn is_analysts(view: &SavedView) -> bool {
    let by_analyst = |label: &str| !label.starts_with("agent:");
    by_analyst(&view.created_by) || by_analyst(&view.updated_by)
}

/// Whether `pattern` matches all of `relation`, `*` standing for any run of
/// characters (none too) and every other character for itself without
/// regard to ASCII case. Greedy with one backtrack point, so linear in
/// practice and never exponential.
fn pattern_matches(pattern: &str, relation: &str) -> bool {
    let (p, r) = (pattern.as_bytes(), relation.as_bytes());
    let (mut pi, mut ri) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ri < r.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some((pi, ri));
            pi += 1;
        } else if pi < p.len() && p[pi].eq_ignore_ascii_case(&r[ri]) {
            pi += 1;
            ri += 1;
        } else if let Some((sp, sr)) = star {
            pi = sp + 1;
            ri = sr + 1;
            star = Some((sp, sr + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&b| b == b'*')
}

/// The columns `filter`'s conditions name, in the order first named, the
/// `_mark` pseudo-column left out. The filter has passed `check_caps`, at
/// save, which bounds this recursion.
fn filter_columns(filter: &FilterNode, out: &mut Vec<String>) {
    match filter {
        FilterNode::And(children) | FilterNode::Or(children) => {
            children.iter().for_each(|c| filter_columns(c, out));
        }
        FilterNode::Not(inner) => filter_columns(inner, out),
        FilterNode::Leaf(leaf) => {
            if leaf.column != MARK_COLUMN && !out.contains(&leaf.column) {
                out.push(leaf.column.clone());
            }
        }
    }
}

/// `layout` without its entries for columns `has` denies; how many entries
/// were dropped, and the columns they named.
fn keep_layout(
    layout: ColumnLayout,
    has: &impl Fn(&str) -> bool,
) -> (ColumnLayout, usize, BTreeSet<String>) {
    let mut dropped = 0;
    let mut missing = BTreeSet::new();
    let mut keep = |column: &String| {
        let kept = has(column);
        if !kept {
            dropped += 1;
            missing.insert(column.clone());
        }
        kept
    };
    let ColumnLayout {
        order,
        widths,
        hidden,
        pinned,
    } = layout;
    let order = order.into_iter().filter(|c| keep(c)).collect();
    let widths = widths.into_iter().filter(|(c, _)| keep(c)).collect();
    let hidden = hidden.into_iter().filter(|c| keep(c)).collect();
    let pinned = pinned.into_iter().filter(|c| keep(c)).collect();
    (
        ColumnLayout {
            order,
            widths,
            hidden,
            pinned,
        },
        dropped,
        missing,
    )
}

/// The history entry for a change to view `id`.
fn change_of(
    id: i64,
    action: &str,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
) -> Change<'_> {
    Change {
        target_kind: "saved_view",
        target_id: id.to_string(),
        action,
        before,
        after,
    }
}

const VIEW_COLUMNS: &str = "id, name, kind, pattern, doc_json, created_by, created_at, \
                            updated_by, updated_at, deleted";

/// The views `condition` selects, views before presets, then by name.
fn read_views(
    conn: &Connection,
    condition: &str,
    values: &[&dyn duckdb::ToSql],
) -> Result<Vec<SavedView>> {
    read_raw(conn, condition, values)?
        .into_iter()
        .map(RawView::decode)
        .collect()
}

/// The views that are not deleted, ordered as `read_views` orders them, with
/// each one that does not decode left out and named instead. Only the
/// listings read this way: a call about one view still answers `Internal`
/// for it, since that is the view it was asked about.
fn read_views_skipping(conn: &Connection) -> Result<(Vec<SavedView>, Vec<SkippedView>)> {
    let (mut views, mut skipped) = (Vec::new(), Vec::new());
    for raw in read_raw(conn, "NOT deleted", &[])? {
        let id = raw.id;
        match raw.decode() {
            Ok(view) => views.push(view),
            Err(e) => skipped.push(SkippedView {
                id,
                reason: e.to_string(),
            }),
        }
    }
    Ok((views, skipped))
}

/// The rows `condition` selects, undecoded, in `read_views`' order.
fn read_raw(
    conn: &Connection,
    condition: &str,
    values: &[&dyn duckdb::ToSql],
) -> Result<Vec<RawView>> {
    let rows: Vec<(RawView, bool)> = conn
        .prepare(&format!(
            "SELECT {VIEW_COLUMNS} FROM ox.saved_view WHERE {condition} \
             ORDER BY kind = 'preset', name, id"
        ))?
        .query_map(values, raw_row)?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows.into_iter().map(|(raw, _)| raw).collect())
}

/// View `id`, deleted or not. `NotFound` for an unknown id.
fn read_view(conn: &Connection, id: i64) -> Result<(SavedView, bool)> {
    let (raw, deleted) = conn
        .query_row(
            &format!("SELECT {VIEW_COLUMNS} FROM ox.saved_view WHERE id = ?"),
            params![id],
            raw_row,
        )
        .map_err(|e| match e {
            duckdb::Error::QueryReturnedNoRows => no_view(id),
            other => other.into(),
        })?;
    Ok((raw.decode()?, deleted))
}

/// View `id` if it is there and not deleted. `NotFound` otherwise, and on a
/// format-1 case, which has none.
fn live_view(case: &Case, id: i64) -> Result<SavedView> {
    if case.format() < CASE_FORMAT {
        return Err(no_view(id));
    }
    match read_view(case.conn(), id)? {
        (view, false) => Ok(view),
        (_, true) => Err(no_view(id)),
    }
}

/// The live view named `name` of `kind`, if there is one.
fn find_by_name(conn: &Connection, name: &str, kind: ViewKind) -> Result<Option<SavedView>> {
    let mut found = read_views(
        conn,
        "NOT deleted AND name = ? AND kind = ?",
        &[&name, &kind.as_str()],
    )?;
    Ok(found.pop())
}

fn no_view(id: i64) -> EngineError {
    EngineError::NotFound(format!("no saved view {id}"))
}

/// A row of `ox.saved_view` before its kind and document are decoded.
struct RawView {
    id: i64,
    name: String,
    kind: String,
    pattern: String,
    doc_json: String,
    created_by: String,
    created_at: String,
    updated_by: String,
    updated_at: String,
}

fn raw_row(r: &duckdb::Row<'_>) -> duckdb::Result<(RawView, bool)> {
    Ok((
        RawView {
            id: r.get(0)?,
            name: r.get(1)?,
            kind: r.get(2)?,
            pattern: r.get(3)?,
            doc_json: r.get(4)?,
            created_by: r.get(5)?,
            created_at: r.get(6)?,
            updated_by: r.get(7)?,
            updated_at: r.get(8)?,
        },
        r.get(9)?,
    ))
}

impl RawView {
    /// The engine writes every row's kind and document itself, so one that
    /// does not decode is `Internal`.
    fn decode(self) -> Result<SavedView> {
        let id = self.id;
        let kind = match self.kind.as_str() {
            "view" => ViewKind::View,
            "preset" => ViewKind::Preset,
            other => {
                return Err(EngineError::Internal(format!(
                    "saved view {id} has kind {}, which the engine never writes",
                    shown(other)
                )))
            }
        };
        let doc: ViewDoc = serde_json::from_str(&self.doc_json).map_err(|e| {
            EngineError::Internal(format!(
                "saved view {id} holds a document that does not decode: {e}"
            ))
        })?;
        Ok(SavedView {
            id,
            name: self.name,
            kind,
            pattern: self.pattern,
            doc,
            created_by: self.created_by,
            created_at: self.created_at,
            updated_by: self.updated_by,
            updated_at: self.updated_at,
        })
    }
}
