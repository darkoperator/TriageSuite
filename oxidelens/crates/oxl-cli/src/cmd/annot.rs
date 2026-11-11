//! The write commands: marks, tags, notes, column typing and the history.
//! Every rule (who may change what, the `ai:` tag prefix, inactive rows, the
//! per-call cap) is the engine's; this file only parses `--rows` and shapes
//! the reply.
//!
//! Outcome reasons and tag names are text the engine produced from fixed
//! wording, row and source ids, validated tag characters and validated actor
//! labels (never a relation or host name, which come from evidence), so they
//! stay outside the wrapper. Everything a list returns about rows (relation
//! and host names, note text, history targets and their before and after
//! objects) goes inside it.

use super::read::text_arg;
use super::{Ctx, Reply};
use crate::args::{PageArgs, SemanticsArg, SqlTypeArg};
use crate::envelope::wrap;
use oxl_engine::{Actor, EngineError, Result, RowOutcome, SqlType, TimeSemantics};
use serde_json::json;

/// `1,2,3` as row ids. Anything else, an empty entry included, is
/// `invalid`: a stray comma is a typo, and acting on the rest would hide it.
fn parse_rows(text: &str) -> Result<Vec<i64>> {
    text.split(',')
        .map(|part| {
            let digits = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
            match part.parse::<i64>() {
                Ok(id) if digits => Ok(id),
                _ => Err(EngineError::Invalid(format!(
                    "--rows is a comma-separated list of integers, not {text:?}"
                ))),
            }
        })
        .collect()
}

/// A batch call that was accepted: exit 0 even when some rows were not done.
fn outcomes_reply(outcomes: Vec<RowOutcome>) -> Reply {
    let done = outcomes.iter().filter(|o| o.done).count();
    Reply::new(json!({
        "done": done,
        "not_done": outcomes.len() - done,
        "outcomes": outcomes,
    }))
}

pub fn mark_set(ctx: &Ctx, rows: &str) -> Result<Reply> {
    let rows = parse_rows(rows)?;
    let mut case = ctx.open_case()?;
    Ok(outcomes_reply(oxl_engine::mark_suspicious(
        &mut case, &rows, &ctx.actor,
    )?))
}

pub fn mark_clear(ctx: &Ctx, rows: &str) -> Result<Reply> {
    let rows = parse_rows(rows)?;
    let mut case = ctx.open_case()?;
    Ok(outcomes_reply(oxl_engine::clear_suspicious(
        &mut case, &rows, &ctx.actor,
    )?))
}

pub fn mark_list(ctx: &Ctx, by: Option<&str>, page: &PageArgs) -> Result<Reply> {
    let by = by.map(Actor::parse).transpose()?;
    let case = ctx.open_case()?;
    let (marks, truncated) = oxl_engine::marks(&case, by.as_ref(), page.limit, page.offset)?;
    Ok(Reply {
        data: json!({"marks": marks.len(), "untrusted": wrap(&json!({"marks": marks}))}),
        truncated,
    })
}

pub fn tag_add(ctx: &Ctx, rows: &str, tag: &str) -> Result<Reply> {
    let rows = parse_rows(rows)?;
    let mut case = ctx.open_case()?;
    Ok(outcomes_reply(oxl_engine::add_tag(
        &mut case, &rows, tag, &ctx.actor,
    )?))
}

pub fn tag_remove(ctx: &Ctx, rows: &str, tag: &str) -> Result<Reply> {
    let rows = parse_rows(rows)?;
    let mut case = ctx.open_case()?;
    Ok(outcomes_reply(oxl_engine::remove_tag(
        &mut case, &rows, tag, &ctx.actor,
    )?))
}

pub fn tag_list(ctx: &Ctx) -> Result<Reply> {
    let case = ctx.open_case()?;
    Ok(Reply::new(json!({"tags": oxl_engine::tags(&case)?})))
}

pub fn note_add(ctx: &Ctx, row: i64, text: &str) -> Result<Reply> {
    let text = text_arg(text)?;
    let mut case = ctx.open_case()?;
    let note = oxl_engine::add_note(&mut case, row, &text, &ctx.actor)?;
    Ok(Reply::new(json!({"id": note.id, "row_id": note.row_id})))
}

pub fn note_delete(ctx: &Ctx, id: i64) -> Result<Reply> {
    let mut case = ctx.open_case()?;
    oxl_engine::delete_note(&mut case, id, &ctx.actor)?;
    Ok(Reply::new(json!({"id": id})))
}

pub fn note_list(ctx: &Ctx, row: i64) -> Result<Reply> {
    let case = ctx.open_case()?;
    let notes = oxl_engine::notes(&case, row)?;
    Ok(Reply::new(json!({
        "notes": notes.len(),
        "untrusted": wrap(&json!({"notes": notes})),
    })))
}

pub fn declare(
    ctx: &Ctx,
    relation: &str,
    column: &str,
    sql_type: SqlTypeArg,
    semantics: Option<SemanticsArg>,
) -> Result<Reply> {
    let sql_type = match sql_type {
        SqlTypeArg::Timestamp => SqlType::Timestamp,
        SqlTypeArg::BigInt => SqlType::BigInt,
        SqlTypeArg::UBigInt => SqlType::UBigInt,
        SqlTypeArg::Boolean => SqlType::Boolean,
    };
    // A zone is never guessed: without --semantics a TIMESTAMP reaches the
    // engine without one, and the engine refuses it as invalid.
    let semantics = semantics.map(|s| match s {
        SemanticsArg::Utc => TimeSemantics::Utc,
        SemanticsArg::Unknown => TimeSemantics::Unknown,
    });
    let mut case = ctx.open_case()?;
    oxl_engine::declare_type(&mut case, relation, column, sql_type, semantics, &ctx.actor)?;
    Ok(Reply::new(json!({"declared": true})))
}

pub fn primary_time(ctx: &Ctx, relation: &str, column: Option<&str>) -> Result<Reply> {
    let mut case = ctx.open_case()?;
    oxl_engine::set_primary_time(&mut case, relation, column, &ctx.actor)?;
    Ok(Reply::new(json!({"set": column.is_some()})))
}

pub fn history(ctx: &Ctx, kind: Option<&str>, page: &PageArgs) -> Result<Reply> {
    let case = ctx.open_case()?;
    let (entries, truncated) = oxl_engine::history(&case, kind, page.limit, page.offset)?;
    Ok(Reply {
        data: json!({"entries": entries.len(), "untrusted": wrap(&json!({"entries": entries}))}),
        truncated,
    })
}
