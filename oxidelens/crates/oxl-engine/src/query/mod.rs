//! What the grid asks for, and the scope every query over it starts from.
//!
//! A `GridQuery` names a relation and narrows it: a filter document, the
//! search box, a set of hosts. `scope` turns it into a `FROM` clause and a
//! `WHERE` clause with their parameters, which pages, counts, groups, the
//! histogram, stacks and find all build on, so every one of them sees the
//! same rows for the same query.
//!
//! The `FROM` clause is a projection of the relation's view, aliased
//! `ox_r`, holding exactly the physical columns the catalog names (each
//! column's text and typed value, and the host column) plus `_ox_row`. A
//! catalogued column that no active source has (one only a superseded or
//! removed source brought) is projected as NULL, so a query naming it reads
//! empty cells instead of failing to bind. Only one `_ox_row` is in scope,
//! which is what the `_mark` filter's correlated subquery relies on.

mod around;
mod fan;
mod find;
mod group;
mod histogram;
mod page;
mod sql;
mod stack;

pub use around::{anchor_of, around, merged_rows, AroundCount, MergedRow, MergedSource};
pub use find::{find, FindHit, FindMode, FindRequest, FindResult, MAX_VALUE};
pub(crate) use group::{as_count, group_key_sql, group_spans, primary_time_semantics};
pub use group::{group_filter, groups, GroupRow};
pub use histogram::{histogram, Bucket, Histogram, BUCKET_LADDER};
pub(crate) use page::clip_text;
pub use page::{count, page, Cell, MarkState, Page, CELL_CAP, DEFAULT_LIMIT, MAX_LIMIT};
pub(crate) use page::{mark_sql, read_mark, MARK_SUSPICIOUS};
pub use sql::{
    describe, query_sql, query_sql_within, Described, DescribedColumn, SqlResult, SQL_MEMORY_LIMIT,
    SQL_TEXT_MAX, SQL_TIME_LIMIT, SQL_TIME_LIMIT_MAX,
};
pub use stack::{stack, Stack, StackOrder, StackRow};

use crate::case::Case;
use crate::catalog::{self, ColumnInfo, RelationInfo};
use crate::error::{EngineError, Result};
use crate::filter::{compile_filter, FilterNode, Op, SqlFragment};
use crate::load::stage::view_columns;
use crate::search::{compile_search, parse_search, Search, SearchOptions};
use duckdb::types::Value as SqlValue;
use duckdb::{params, Connection};
use std::collections::BTreeSet;
use triage_core::output::duckdb::sql::{quote_ident, quote_literal};

/// One grid request: a relation and what narrows, orders and shows it.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GridQuery {
    pub relation: String,
    #[serde(default)]
    pub filter: Option<FilterNode>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub search_regex: bool,
    #[serde(default)]
    pub search_case_sensitive: bool,
    /// Empty means every host.
    #[serde(default)]
    pub hosts: Vec<String>,
    /// Empty means the primary time ascending when there is one, then `_ox_row`.
    #[serde(default)]
    pub sort: Vec<SortKey>,
    /// `None` means every catalogued column.
    #[serde(default)]
    pub columns: Option<Vec<String>>,
}

/// One sort key. It arrives untrusted inside a grid state or a saved view,
/// so unknown fields are refused.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SortKey {
    pub column: String,
    #[serde(default)]
    pub descending: bool,
}

/// The rows a `GridQuery` selects, as SQL: `SELECT … FROM {from_sql} WHERE
/// {where_sql}`, binding `params` in order. Every `?` is in `where_sql`.
///
/// `from_sql` must be the only FROM item: `where_sql` names columns
/// unqualified, and the `_mark` filter's correlated subquery reads the one
/// `_ox_row` in scope, so a table joined beside it would capture or shadow
/// those names. A query that needs another table wraps the scope as a
/// subquery (`SELECT … FROM (SELECT … FROM {from_sql} WHERE {where_sql})
/// JOIN …`) instead.
pub(crate) struct Scope {
    pub from_sql: String,
    pub where_sql: String,
    pub params: Vec<SqlValue>,
    /// The columns the query shows, in order: the ones it named, or every
    /// catalogued column.
    pub columns: Vec<ColumnInfo>,
    /// Every catalogued column of the relation, in display order, shown or
    /// not: what a sort, group or stack column is looked up in.
    pub all: Vec<ColumnInfo>,
    pub relation: RelationInfo,
}

/// The offset rule every paged call shares: DuckDB's `OFFSET` is a signed
/// 64-bit value, so an offset past `i64::MAX` is `Invalid` here rather than
/// a conversion error from the statement that binds it.
pub(crate) fn check_offset(offset: u64) -> Result<()> {
    if i64::try_from(offset).is_err() {
        return Err(EngineError::Invalid(format!(
            "an offset is at most {}, not {offset}",
            i64::MAX
        )));
    }
    Ok(())
}

/// `Invalid` unless `zone` is an IANA zone name this build knows, spelled
/// exactly. A display zone is never guessed or corrected.
pub(crate) fn check_zone(zone: &str) -> Result<()> {
    if triage_core::timestamp::zone::is_zone(zone) {
        return Ok(());
    }
    let cut: String = zone.chars().take(64).collect();
    Err(EngineError::Invalid(format!(
        "{cut:?}{} is not a time zone this build knows (an IANA name such as \
         America/New_York, or UTC)",
        if cut.len() < zone.len() { "…" } else { "" }
    )))
}

/// UTC text of a timestamp, six fractional digits (ruling R1), as a
/// `strftime` format. A constant, never a value.
pub(crate) const ISO_FORMAT: &str = "%Y-%m-%dT%H:%M:%S.%fZ";

/// True for a row in scope that carries a mark. The mark's spelling is the
/// engine's own constant, never outside input, so it is a quoted literal.
pub(crate) fn marked_sql() -> String {
    format!(
        "EXISTS (SELECT 1 FROM ox.row_mark AS ox_gm \
         WHERE ox_gm.row_id = {ROW_ALIAS}._ox_row AND ox_gm.mark = {})",
        quote_literal(MARK_SUSPICIOUS)
    )
}

/// The alias of the projection `Scope::from_sql` reads from.
pub(crate) const ROW_ALIAS: &str = "ox_r";

/// What a scope is built over: a relation, every column the catalog has for
/// it, and the physical columns its view has.
pub(crate) struct Subject {
    pub relation: RelationInfo,
    all: Vec<ColumnInfo>,
    /// Lowercased, as DuckDB compares names; empty when there is no view.
    present: BTreeSet<String>,
}

/// Every relation of the case as a subject, in name order. A pivot covers
/// them all, so they are read together, in a handful of statements however
/// many there are, instead of each as `scope` reads its one.
pub(crate) fn subjects(case: &Case) -> Result<Vec<Subject>> {
    let conn = case.conn();
    let mut columns = catalog::all_columns(conn)?;
    let mut present = view_columns(conn)?;
    Ok(catalog::relations(case)?
        .into_iter()
        .map(|relation| Subject {
            all: columns.remove(&relation.name).unwrap_or_default(),
            present: present.remove(&relation.name).unwrap_or_default(),
            relation,
        })
        .collect())
}

/// Build the scope of `q`. `NotFound` for an unknown relation, a named
/// column the relation lacks or a host the case lacks (`resolve_hosts`);
/// `Invalid` for a bad filter or search, hosts on a relation without a host
/// column, or a regex DuckDB will not compile.
pub(crate) fn scope(case: &Case, q: &GridQuery) -> Result<Scope> {
    let relation = catalog::relation(case, &q.relation)?;
    let subject = Subject {
        all: catalog::columns(case, &q.relation)?,
        present: described_columns(case.conn(), &relation)?,
        relation,
    };
    let q = GridQuery {
        hosts: resolve_hosts(case, &q.hosts)?,
        ..q.clone()
    };
    scope_of(case, &subject, &q)
}

/// Hosts of the case a caller named, as the case spells them. A host name
/// is matched without regard to case, as Windows host names are: the case's
/// own spelling when one is exactly the name, else every host of the case
/// that differs from it only in case. `NotFound`, naming the case's hosts,
/// for a name no host of the case matches; a host the case has but one
/// relation lacks is not an error, only no rows there.
pub(crate) fn resolve_hosts(case: &Case, hosts: &[String]) -> Result<Vec<String>> {
    if hosts.is_empty() {
        return Ok(Vec::new());
    }
    let known: Vec<String> = case
        .conn()
        .prepare(
            "SELECT DISTINCT sd.host FROM ox.source_dataset sd \
             JOIN ox.source s ON s.id = sd.source_id \
             WHERE sd.active AND NOT s.removed AND sd.host IS NOT NULL ORDER BY sd.host",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut out: Vec<String> = Vec::new();
    for name in hosts {
        let matched: Vec<&String> = match known.iter().find(|k| *k == name) {
            Some(exact) => vec![exact],
            None => {
                let folded = name.to_lowercase();
                known
                    .iter()
                    .filter(|k| k.to_lowercase() == folded)
                    .collect()
            }
        };
        if matched.is_empty() {
            return Err(EngineError::NotFound(format!(
                "no host {name:?} in the case; its hosts are {}",
                host_list(&known)
            )));
        }
        for host in matched {
            if !out.contains(host) {
                out.push(host.clone());
            }
        }
    }
    Ok(out)
}

/// The case's hosts for a message: all of them, or the first twenty and
/// how many more.
fn host_list(known: &[String]) -> String {
    const SHOWN: usize = 20;
    if known.is_empty() {
        return "none".into();
    }
    let mut text = known
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if known.len() > SHOWN {
        text.push_str(&format!(" and {} more", known.len() - SHOWN));
    }
    text
}

/// `scope` over a subject already read. `q.relation` is not looked up again:
/// the subject is the relation.
pub(crate) fn scope_of(case: &Case, subject: &Subject, q: &GridQuery) -> Result<Scope> {
    let conn = case.conn();
    let relation = subject.relation.clone();
    let all = &subject.all;
    let columns = match &q.columns {
        None => all.clone(),
        Some(names) => names
            .iter()
            .map(|name| find_column(all, &relation.name, name).cloned())
            .collect::<Result<_>>()?,
    };

    let mut parts = Vec::new();
    let mut params = Vec::new();
    if !q.hosts.is_empty() {
        let host = relation.host_column.as_deref().ok_or_else(|| {
            EngineError::Invalid(format!(
                "{} has no host column, so it cannot be narrowed to hosts",
                relation.name
            ))
        })?;
        let slots = vec!["?"; q.hosts.len()].join(", ");
        parts.push(format!("{} IN ({slots})", quote_ident(host)));
        params.extend(q.hosts.iter().map(|h| SqlValue::Text(h.clone())));
    }
    let mut patterns = Vec::new();
    if let Some(filter) = &q.filter {
        let fragment = compile_filter(filter, all)?;
        regex_leaves(filter, &mut patterns);
        parts.push(fragment.sql);
        params.extend(fragment.params);
    }
    for (pattern, case_sensitive) in &patterns {
        probe_regex(conn, pattern, *case_sensitive)?;
    }
    if let Some(text) = &q.search {
        let opts = SearchOptions {
            regex: q.search_regex,
            case_sensitive: q.search_case_sensitive,
        };
        if let Some(fragment) = search_fragment(conn, text, opts, all)? {
            parts.push(fragment.sql);
            params.extend(fragment.params);
        }
    }

    let where_sql = if parts.is_empty() {
        "TRUE".to_string()
    } else {
        parts.join(" AND ")
    };
    Ok(Scope {
        from_sql: projection(&relation, all, &subject.present),
        where_sql,
        params,
        columns,
        all: all.clone(),
        relation,
    })
}

/// `ORDER BY` keys for `q` over `scope`, without the keyword: the named
/// keys, or the primary time ascending; `_ox_row` is always last, so the
/// order is total. A typed column sorts by its typed value, any other by its
/// text without regard to case; nulls sort last either way. A sort column
/// need not be one the query shows. `NotFound` for one the relation lacks.
pub(crate) fn order_sql(scope: &Scope, q: &GridQuery) -> Result<String> {
    let all = scope.all.as_slice();
    let keys: Vec<(ColumnInfo, bool)> = if q.sort.is_empty() {
        scope
            .relation
            .primary_time
            .iter()
            .filter_map(|name| all.iter().find(|c| c.name == *name))
            .map(|c| (c.clone(), false))
            .collect()
    } else {
        q.sort
            .iter()
            .map(|k| {
                Ok((
                    find_column(all, &scope.relation.name, &k.column)?.clone(),
                    k.descending,
                ))
            })
            .collect::<Result<_>>()?
    };
    let mut out: Vec<String> = keys
        .iter()
        .map(|(c, descending)| {
            let expr = match &c.typed_column {
                Some(typed) => format!("{ROW_ALIAS}.{}", quote_ident(typed)),
                None => format!("lower({ROW_ALIAS}.{})", quote_ident(&c.text_column)),
            };
            let dir = if *descending { "DESC" } else { "ASC" };
            format!("{expr} {dir} NULLS LAST")
        })
        .collect();
    out.push(format!("{ROW_ALIAS}._ox_row ASC"));
    Ok(out.join(", "))
}

/// The search box's text as a boolean expression over `all`, the catalogued
/// columns of the relation, with the parameters it binds; `None` when there
/// is nothing to search for. A regex is compiled in DuckDB first, so one RE2
/// refuses is `Invalid` here. Every column it names is unqualified, so it is
/// placed where `Scope::from_sql` is the only FROM item, as `where_sql` is.
pub(crate) fn search_fragment(
    conn: &Connection,
    text: &str,
    opts: SearchOptions,
    all: &[ColumnInfo],
) -> Result<Option<SqlFragment>> {
    let Some(search) = parse_search(text, opts, all)? else {
        return Ok(None);
    };
    if let Search::Regex(pattern) = &search {
        probe_regex(conn, pattern, opts.case_sensitive)?;
    }
    compile_search(&search, opts, all).map(Some)
}

pub(crate) fn find_column<'a>(
    all: &'a [ColumnInfo],
    relation: &str,
    name: &str,
) -> Result<&'a ColumnInfo> {
    all.iter()
        .find(|c| c.name == name)
        .ok_or_else(|| EngineError::NotFound(format!("{relation} has no column {name}")))
}

/// Every `regex` leaf's pattern in `node`, with its case sensitivity. The
/// node has passed `compile_filter`, whose caps bound this recursion.
pub(crate) fn regex_leaves(node: &FilterNode, out: &mut Vec<(String, bool)>) {
    match node {
        FilterNode::And(children) | FilterNode::Or(children) => {
            children.iter().for_each(|c| regex_leaves(c, out));
        }
        FilterNode::Not(inner) => regex_leaves(inner, out),
        FilterNode::Leaf(leaf) => {
            if let (Op::Regex, Some(pattern)) = (leaf.op, leaf.value.as_str()) {
                out.push((pattern.to_string(), leaf.case_sensitive));
            }
        }
    }
}

/// Compile `pattern` in DuckDB the way the query will, over one row, so a
/// pattern RE2 refuses is `Invalid` here rather than an engine error from
/// the query that uses it. The Rust `regex` crate's check in the search
/// parser does not settle it: the two dialects differ.
pub(crate) fn probe_regex(conn: &Connection, pattern: &str, case_sensitive: bool) -> Result<()> {
    let flags = if case_sensitive { "" } else { ", 'i'" };
    conn.query_row(
        &format!("SELECT regexp_matches('', ?{flags})"),
        params![pattern],
        |r| r.get::<_, Option<bool>>(0),
    )
    .map(|_| ())
    .map_err(|e| EngineError::Invalid(format!("{pattern:?} is not a valid regex: {e}")))
}

/// The physical columns of `relation`'s view, lowercased, as DuckDB itself
/// describes the view; empty when the relation has no view (it has no
/// active source). `view_columns` reads the same for every relation at once.
fn described_columns(conn: &Connection, relation: &RelationInfo) -> Result<BTreeSet<String>> {
    let exists: i64 = conn.query_row(
        "SELECT count(*) FROM duckdb_views() WHERE schema_name = 'main' AND view_name = ?",
        params![relation.name],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Ok(BTreeSet::new());
    }
    Ok(conn
        .prepare(&format!(
            "SELECT lower(column_name) FROM (DESCRIBE SELECT * FROM main.{})",
            quote_ident(&relation.name)
        ))?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?)
}

/// `(SELECT <catalogued physical columns>, _ox_row FROM main."<relation>")
/// AS ox_r`, given the columns the view has (`present`). A relation with no
/// view (`present` is empty) projects the same columns over no rows.
fn projection(relation: &RelationInfo, all: &[ColumnInfo], present: &BTreeSet<String>) -> String {
    let view = format!("main.{}", quote_ident(&relation.name));
    let exists = !present.is_empty();

    // (physical name, SQL type to stand in with when absent)
    let mut wanted: Vec<(String, String)> = Vec::new();
    for c in all {
        wanted.push((c.text_column.clone(), "VARCHAR".into()));
        if let (Some(typed), Some(ty)) = (&c.typed_column, c.sql_type) {
            wanted.push((typed.clone(), ty.sql().into()));
        }
    }
    if let Some(host) = &relation.host_column {
        wanted.push((host.clone(), "VARCHAR".into()));
    }
    let mut seen = BTreeSet::new();
    let mut select: Vec<String> = Vec::new();
    for (name, ty) in wanted {
        // DuckDB resolves identifiers case-insensitively, so that is how two
        // names are the same column.
        if !seen.insert(name.to_lowercase()) {
            continue;
        }
        let ident = quote_ident(&name);
        select.push(if present.contains(&name.to_lowercase()) {
            ident
        } else {
            format!("CAST(NULL AS {ty}) AS {ident}")
        });
    }
    let source = if exists {
        select.push("_ox_row".into());
        format!("FROM {view}")
    } else {
        select.push("CAST(NULL AS BIGINT) AS _ox_row".into());
        "WHERE FALSE".into()
    };
    format!("(SELECT {} {source}) AS {ROW_ALIAS}", select.join(", "))
}
