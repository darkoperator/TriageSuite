//! The read commands: thin calls into the engine's queries. Cell text,
//! column and relation names, hosts and paths go inside the `untrusted`
//! wrapper; counts, ids, flags and marks, which the tool produced, stay
//! outside.

use super::{Ctx, Reply};
use crate::args::{AroundArgs, FindArgs, FindModeArg, PageArgs, QueryArgs, StackOrderArg};
use crate::envelope::wrap;
use oxl_engine::{
    EngineError, FindMode, FindRequest, GridQuery, MergedRow, MergedSource, Result, SortKey,
    StackOrder, MAX_LIMIT,
};
use serde_json::{json, Value};

/// The longest `around` window: a day, in seconds.
const MAX_WINDOW_SECONDS: u64 = 24 * 3600;

/// `text`, or the content of the file named by `@path`. The file is read
/// because the caller named it; a failure to read it is the caller's
/// mistake, so it is `invalid`.
pub(super) fn text_arg(text: &str) -> Result<String> {
    match text.strip_prefix('@') {
        None => Ok(text.to_string()),
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| EngineError::Invalid(format!("cannot read {path}: {e}"))),
    }
}

fn grid_query(relation: &str, args: &QueryArgs) -> Result<GridQuery> {
    let filter = match &args.filter {
        Some(text) => Some(oxl_engine::parse_filter(&text_arg(text)?)?),
        None => None,
    };
    let sort = args
        .sort
        .iter()
        .map(|s| parse_sort(s))
        .collect::<Result<Vec<_>>>()?;
    if let Some(columns) = &args.columns {
        named("--columns", columns)?;
    }
    Ok(GridQuery {
        relation: relation.to_string(),
        filter,
        search: args.search.clone(),
        search_regex: args.regex,
        search_case_sensitive: args.case_sensitive,
        hosts: args.hosts.clone(),
        sort,
        columns: args.columns.clone(),
    })
}

/// A comma list with an empty entry (`a,,b`, `,`) is a typo, not a column.
fn named(flag: &str, names: &[String]) -> Result<()> {
    if names.iter().any(|c| c.is_empty()) {
        return Err(EngineError::Invalid(format!(
            "{flag} names a column with an empty name"
        )));
    }
    Ok(())
}

/// `column` or `column:desc` (`:asc` is accepted too). A colon that does not
/// end in one of those words belongs to the column's name, except a colon
/// with nothing after it, which is a direction left out by mistake.
fn parse_sort(text: &str) -> Result<SortKey> {
    if text.ends_with(':') {
        return Err(EngineError::Invalid(format!(
            "--sort {text:?} has a colon but no direction (asc or desc)"
        )));
    }
    let (column, descending) = match text.rsplit_once(':') {
        Some((column, "desc")) => (column, true),
        Some((column, "asc")) => (column, false),
        _ => (text, false),
    };
    if column.is_empty() {
        return Err(EngineError::Invalid(format!(
            "--sort {text:?} names no column"
        )));
    }
    Ok(SortKey {
        column: column.to_string(),
        descending,
    })
}

/// An integer then `s`, `m` or `h`, at most 24 hours, as seconds.
fn parse_window(text: &str) -> Result<u64> {
    let invalid = || {
        EngineError::Invalid(format!(
            "--window {text:?} is not an integer followed by s, m or h (at most 24h)"
        ))
    };
    let unit = text.chars().last().ok_or_else(invalid)?;
    let digits = &text[..text.len() - unit.len_utf8()];
    let factor = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        _ => return Err(invalid()),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let seconds = digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(factor))
        .ok_or_else(invalid)?;
    if seconds > MAX_WINDOW_SECONDS {
        return Err(invalid());
    }
    Ok(seconds)
}

/// How many rows to ask a list query for so that one extra says whether the
/// list was cut. At the engine's maximum there is no room for the extra, so
/// the caller falls back to `len == limit`.
fn ask_for(limit: u32) -> u32 {
    if limit < MAX_LIMIT {
        limit + 1
    } else {
        limit
    }
}

/// Trim a list fetched with `ask_for` to `limit` and say whether it was cut.
/// At the maximum limit a full list is reported cut, since it may be.
fn trim<T>(list: &mut Vec<T>, limit: u32) -> bool {
    let limit = limit as usize;
    let cut = if limit < MAX_LIMIT as usize {
        list.len() > limit
    } else {
        list.len() == limit
    };
    list.truncate(limit);
    cut
}

pub fn rows(ctx: &Ctx, relation: &str, query: &QueryArgs, page: &PageArgs) -> Result<Reply> {
    let q = grid_query(relation, query)?;
    let case = ctx.open_case()?;
    let p = oxl_engine::page(&case, &q, page.limit, page.offset)?;
    let mut failed = Vec::new();
    let mut clipped = false;
    let cells: Vec<Vec<Option<&str>>> = p
        .cells
        .iter()
        .enumerate()
        .map(|(r, row)| {
            row.iter()
                .enumerate()
                .map(|(c, cell)| {
                    if cell.failed {
                        failed.push(json!([r, c]));
                    }
                    clipped |= cell.clipped;
                    cell.text.as_deref()
                })
                .collect()
        })
        .collect();
    let data = json!({
        "row_ids": p.row_ids,
        "marks": p.marks,
        "cells_clipped": clipped,
        "untrusted": wrap(&json!({"columns": p.columns, "cells": cells, "failed": failed})),
    });
    Ok(Reply {
        data,
        truncated: p.truncated,
    })
}

pub fn count(ctx: &Ctx, relation: &str, query: &QueryArgs) -> Result<Reply> {
    let q = grid_query(relation, query)?;
    let case = ctx.open_case()?;
    Ok(Reply::new(json!({"count": oxl_engine::count(&case, &q)?})))
}

pub fn groups(
    ctx: &Ctx,
    relation: &str,
    by: &[String],
    parent: Option<&str>,
    query: &QueryArgs,
    page: &PageArgs,
) -> Result<Reply> {
    named("--by", by)?;
    let q = grid_query(relation, query)?;
    let parent: Vec<Option<String>> = match parent {
        Some(text) => serde_json::from_str(text).map_err(|e| {
            EngineError::Invalid(format!(
                "--parent must be a JSON array of strings and nulls: {e}"
            ))
        })?,
        None => Vec::new(),
    };
    let case = ctx.open_case()?;
    let mut list = oxl_engine::groups(&case, &q, by, &parent, ask_for(page.limit), page.offset)?;
    let truncated = trim(&mut list, page.limit);
    let groups: Vec<_> = list
        .iter()
        .map(|g| {
            json!({"key": g.key, "count": g.count, "first": g.first, "last": g.last,
                   "suspicious": g.suspicious})
        })
        .collect();
    Ok(Reply {
        data: json!({
            "groups": groups.len(),
            "untrusted": wrap(&json!({"groups": groups})),
        }),
        truncated,
    })
}

pub fn histogram(
    ctx: &Ctx,
    relation: &str,
    column: Option<&str>,
    buckets: u32,
    query: &QueryArgs,
) -> Result<Reply> {
    let q = grid_query(relation, query)?;
    let case = ctx.open_case()?;
    let h = oxl_engine::histogram(&case, &q, column, buckets)?;
    let list: Vec<_> = h
        .buckets
        .iter()
        .map(|b| json!({"start": b.start, "count": b.count, "suspicious": b.suspicious}))
        .collect();
    Ok(Reply::new(json!({
        "bucket_seconds": h.bucket_seconds,
        "untimed": h.untimed,
        "buckets": list,
        "untrusted": wrap(&json!({"column": h.column})),
    })))
}

pub fn stack(
    ctx: &Ctx,
    relation: &str,
    by: &[String],
    order: StackOrderArg,
    query: &QueryArgs,
    page: &PageArgs,
) -> Result<Reply> {
    named("--by", by)?;
    let q = grid_query(relation, query)?;
    let order = match order {
        StackOrderArg::Rarest => StackOrder::Rarest,
        StackOrderArg::Commonest => StackOrder::Commonest,
    };
    let case = ctx.open_case()?;
    let s = oxl_engine::stack(&case, &q, by, order, page.limit, page.offset)?;
    let rows: Vec<_> = s
        .rows
        .iter()
        .map(|r| {
            json!({"values": r.values, "count": r.count, "hosts": r.hosts, "first": r.first,
                   "last": r.last, "suspicious": r.suspicious})
        })
        .collect();
    Ok(Reply {
        data: json!({
            "hosts_total": s.hosts_total,
            "distinct": s.distinct,
            "rows_counted": s.rows_counted,
            "rows": rows.len(),
            "untrusted": wrap(&json!({"by": s.by, "rows": rows})),
        }),
        truncated: s.truncated,
    })
}

/// A merged-rows reply: ids and marks outside, the readable parts inside.
fn merged_reply(rows: &[MergedRow], truncated: bool) -> Reply {
    let list: Vec<_> = rows
        .iter()
        .map(|r| {
            json!({"relation": r.relation, "time": r.time, "host": r.host, "summary": r.summary})
        })
        .collect();
    Reply {
        data: json!({
            "row_ids": rows.iter().map(|r| r.row_id).collect::<Vec<_>>(),
            "marks": rows.iter().map(|r| r.mark).collect::<Vec<_>>(),
            "untrusted": wrap(&json!({"rows": list})),
        }),
        truncated,
    }
}

pub fn find(ctx: &Ctx, args: &FindArgs) -> Result<Reply> {
    let req = FindRequest {
        value: args.value.clone(),
        mode: match args.mode {
            FindModeArg::Exact => FindMode::Exact,
            FindModeArg::Contains => FindMode::Contains,
        },
        hosts: args.hosts.clone(),
        from: args.from.clone(),
        to: args.to.clone(),
    };
    let case = ctx.open_case()?;
    if args.rows {
        let (rows, truncated) = oxl_engine::merged_rows(
            &case,
            &MergedSource::Find(req),
            args.page.limit,
            args.page.offset,
        )?;
        return Ok(merged_reply(&rows, truncated));
    }
    let found = oxl_engine::find(&case, &req, &mut |_, _| true)?;
    let hits: Vec<_> = found
        .hits
        .iter()
        .map(|h| {
            json!({"relation": h.relation, "column": h.column, "host": h.host,
                   "count": h.count, "first": h.first, "last": h.last})
        })
        .collect();
    Ok(Reply::new(json!({
        "hits": hits.len(),
        "cancelled": found.cancelled,
        "untrusted": wrap(&json!({
            "hits": hits,
            "searched": found.searched,
            "no_hit": found.no_hit,
            "hosts_not_seen": found.hosts_not_seen,
        })),
    })))
}

pub fn around(ctx: &Ctx, args: &AroundArgs) -> Result<Reply> {
    let window_seconds = parse_window(&args.window)?;
    let case = ctx.open_case()?;
    let (host, at) = match (args.row, &args.host, &args.at) {
        (Some(row), _, _) => oxl_engine::anchor_of(&case, row)?,
        (None, Some(host), Some(at)) => (host.clone(), at.clone()),
        // clap requires one anchor and pairs --host with --at.
        _ => {
            return Err(EngineError::Invalid(
                "around needs --row, or --host with --at".to_string(),
            ))
        }
    };
    if args.rows {
        let source = MergedSource::Around {
            host,
            at,
            window_seconds,
        };
        let (rows, truncated) =
            oxl_engine::merged_rows(&case, &source, args.page.limit, args.page.offset)?;
        return Ok(merged_reply(&rows, truncated));
    }
    let counts: Vec<Value> = oxl_engine::around(&case, &host, &at, window_seconds)?
        .iter()
        .map(|c| json!({"relation": c.relation, "count": c.count}))
        .collect();
    Ok(Reply::new(json!({
        "counts": counts.len(),
        "untrusted": wrap(&json!({"counts": counts})),
    })))
}

pub fn row(ctx: &Ctx, id: i64) -> Result<Reply> {
    let case = ctx.open_case()?;
    let d = oxl_engine::row_detail(&case, id)?;
    let fields: Vec<_> = d
        .fields
        .iter()
        .map(|(name, cell)| json!([name, cell.text, cell.failed]))
        .collect();
    let notes: Vec<_> = d
        .notes
        .iter()
        .map(|n| {
            json!({"id": n.id, "text": n.text, "written_by": n.written_by,
                   "written_at": n.written_at})
        })
        .collect();
    Ok(Reply::new(json!({
        "row_id": d.row_id,
        "source_id": d.source_id,
        "hash": d.hash,
        "active": d.active,
        "mark": d.mark,
        "marked_by": d.marked_by,
        "tags": d.tags,
        "untrusted": wrap(&json!({
            "relation": d.relation,
            "host": d.host,
            "fields": fields,
            "notes": notes,
            "evidence_path": d.evidence_path,
            "output_file": d.output_file,
        })),
    })))
}

/// No `Case` is opened here: `query_sql` takes its own locked connection to
/// the file, and a second open handle would defeat the lock.
pub fn query(ctx: &Ctx, sql: &str, page: &PageArgs) -> Result<Reply> {
    let sql = text_arg(sql)?;
    let r = oxl_engine::query_sql(ctx.case_path()?, &sql, page.limit, page.offset)?;
    Ok(Reply {
        data: json!({
            "rows_returned": r.rows.len(),
            "cells_clipped": r.cells_clipped,
            "untrusted": wrap(&json!({"columns": r.columns, "types": r.types, "rows": r.rows})),
        }),
        truncated: r.truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows() {
        assert_eq!(parse_window("30s").unwrap(), 30);
        assert_eq!(parse_window("5m").unwrap(), 300);
        assert_eq!(parse_window("24h").unwrap(), 86_400);
        for bad in [
            "",
            "s",
            "1",
            "25h",
            "1441m",
            "2d",
            "-1s",
            "+1s",
            " 1s",
            "1.5m",
            "1ss",
            "１s",
            "99999999999999999999h",
        ] {
            assert!(parse_window(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn sort_keys() {
        let k = parse_sort("EventId:desc").unwrap();
        assert_eq!((k.column.as_str(), k.descending), ("EventId", true));
        let k = parse_sort("a:b").unwrap();
        assert_eq!((k.column.as_str(), k.descending), ("a:b", false));
        assert!(parse_sort(":desc").is_err());
        assert!(parse_sort("").is_err());
    }

    #[test]
    fn truncation_of_a_list_without_a_flag() {
        let mut v = vec![1, 2, 3];
        assert!(trim(&mut v, 2));
        assert_eq!(v, [1, 2]);
        let mut v = vec![1, 2];
        assert!(!trim(&mut v, 2));
    }
}
