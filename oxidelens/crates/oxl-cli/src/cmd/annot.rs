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
//! objects) goes inside it; a mark list repeats its row ids and `active`
//! flags outside, as the engine's own values.

use super::read::text_arg;
use super::{Ctx, Reply};
use crate::args::{PageArgs, SemanticsArg, SqlTypeArg};
use crate::envelope::wrap;
use oxl_engine::{Actor, EngineError, Result, RowOutcome, SqlType, TimeSemantics};
use serde_json::json;

/// `1,2,3` as row ids, or with `@path` the ids in that file, which a list
/// too long for a command line needs: there they are separated by commas,
/// spaces or new lines, in any mix. Anything else, an empty entry between
/// commas included, is `invalid`: a stray comma is a typo, and acting on the
/// rest would hide it.
pub(super) fn parse_rows(text: &str) -> Result<Vec<i64>> {
    // Names the entry that is wrong, cut short: the whole list can be tens of
    // thousands of ids, and an error that repeats it is no help.
    let invalid = |what: &str| {
        let shown: String = what.chars().take(40).collect();
        let cut = if shown.len() < what.len() { "..." } else { "" };
        EngineError::Invalid(format!(
            "--rows is a comma-separated list of integers, or @file holding integers \
             separated by commas, spaces or new lines; not {shown:?}{cut}"
        ))
    };
    let id = |word: &str| {
        let digits = !word.is_empty() && word.bytes().all(|b| b.is_ascii_digit());
        match word.parse::<i64>() {
            Ok(id) if digits => Ok(id),
            _ => Err(invalid(word)),
        }
    };
    if !text.starts_with('@') {
        return text.split(',').map(id).collect();
    }
    let content = text_arg(text)?;
    let mut out = Vec::new();
    for entry in content.split(',') {
        let words: Vec<&str> = entry.split_ascii_whitespace().collect();
        if words.is_empty() {
            return Err(invalid(entry));
        }
        for word in words {
            out.push(id(word)?);
        }
    }
    Ok(out)
}

/// A batch call that was accepted: exit 0 even when some rows were not done.
pub(super) fn outcomes_reply(outcomes: Vec<RowOutcome>) -> Reply {
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
    let case = ctx.read_case()?;
    let marks = oxl_engine::marks(&case, by.as_ref(), page.limit, page.offset)?;
    Ok(Reply {
        data: json!({
            "marks": marks.items.len(),
            "row_ids": marks.items.iter().map(|m| m.row_id).collect::<Vec<_>>(),
            "active": marks.items.iter().map(|m| m.active).collect::<Vec<_>>(),
            "untrusted": wrap(&json!({"marks": marks.items})),
        }),
        truncated: marks.truncated,
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

pub fn tag_list(ctx: &Ctx, page: &PageArgs) -> Result<Reply> {
    let case = ctx.read_case()?;
    let tags = oxl_engine::tags(&case, page.limit, page.offset)?;
    Ok(Reply {
        data: json!({"tags": tags.items}),
        truncated: tags.truncated,
    })
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

/// The notes on a row, with where the row stands: a superseded row's notes
/// usually moved to `current_row`, which an empty list would not say.
pub fn note_list(ctx: &Ctx, row: i64, page: &PageArgs) -> Result<Reply> {
    let case = ctx.read_case()?;
    let notes = oxl_engine::notes(&case, row, page.limit, page.offset)?;
    let place = oxl_engine::row_place(&case, row)?;
    Ok(Reply {
        data: json!({
            "standing": place.map(|p| p.standing),
            "current_row": place.and_then(|p| p.current_row),
            "notes": notes.items.len(),
            "untrusted": wrap(&json!({"notes": notes.items})),
        }),
        truncated: notes.truncated,
    })
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

pub fn history(ctx: &Ctx, kind: Option<&str>, id: Option<&str>, page: &PageArgs) -> Result<Reply> {
    let case = ctx.read_case()?;
    // An entry or record by the id the case shows (`E-12`) is its number.
    let id = match (kind, id) {
        (Some(kind), Some(id)) => Some(crate::args::target_id(kind, id)),
        _ => None,
    };
    let entries = oxl_engine::history(&case, kind, id.as_deref(), page.limit, page.offset)?;
    Ok(Reply {
        data: json!({
            "entries": entries.items.len(),
            "untrusted": wrap(&json!({"entries": entries.items})),
        }),
        truncated: entries.truncated,
    })
}
