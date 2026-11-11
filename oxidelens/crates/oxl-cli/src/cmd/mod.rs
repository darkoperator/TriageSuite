//! One function per command: parsed arguments in, envelope data or an
//! `EngineError` out. `execute` is the only place that knows every command.

mod annot;
mod case;
mod describe;
mod load;
mod read;

use crate::args::{CaseCmd, Cli, ColumnCmd, Command, LoadCmd, MarkCmd, NoteCmd, SourceCmd, TagCmd};
use oxl_engine::{Actor, Case, EngineError, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// What a command returns on success.
pub struct Reply {
    pub data: Value,
    /// True when the result was cut short (a row limit, say).
    pub truncated: bool,
}

impl Reply {
    pub fn new(data: Value) -> Reply {
        Reply {
            data,
            truncated: false,
        }
    }
}

/// The global options a command may need.
pub struct Ctx {
    case: Option<PathBuf>,
    pub actor: Actor,
}

impl Ctx {
    /// Open the case named by `--case` or `OXIDELENS_CASE`.
    pub fn open_case(&self) -> Result<Case> {
        match &self.case {
            Some(path) => Case::open(path),
            None => Err(no_case()),
        }
    }

    /// The case file's path, for the engine calls that open their own
    /// connection and so must not find the case already open here.
    pub fn case_path(&self) -> Result<&Path> {
        self.case.as_deref().ok_or_else(no_case)
    }
}

fn no_case() -> EngineError {
    EngineError::Invalid("no case: pass --case <path> or set OXIDELENS_CASE".to_string())
}

pub fn execute(cli: &Cli) -> Result<Reply> {
    if let Command::Capabilities = cli.command {
        return Ok(describe::capabilities());
    }
    let ctx = Ctx {
        case: cli.case.clone(),
        actor: resolve_actor(cli.actor.as_deref())?,
    };
    match &cli.command {
        Command::Case(CaseCmd::New { path, name }) => case::new(&ctx, path, name),
        Command::Case(CaseCmd::Info) => case::info(&ctx),
        Command::Load(LoadCmd::Run { out_root }) => load::run(&ctx, out_root),
        Command::Load(LoadCmd::File { path, name, host }) => {
            load::file(&ctx, path, name.clone(), host.clone())
        }
        Command::Sources => load::sources(&ctx),
        Command::Source(SourceCmd::Remove {
            id,
            confirm_orphans,
        }) => load::remove(&ctx, *id, *confirm_orphans),
        Command::Capabilities => Ok(describe::capabilities()),
        Command::Schema { relation } => describe::schema(&ctx, relation.as_deref()),
        Command::Rows {
            relation,
            query,
            page,
        } => read::rows(&ctx, relation, query, page),
        Command::Count { relation, query } => read::count(&ctx, relation, query),
        Command::Groups {
            relation,
            by,
            parent,
            query,
            page,
        } => read::groups(&ctx, relation, by, parent.as_deref(), query, page),
        Command::Histogram {
            relation,
            column,
            buckets,
            query,
        } => read::histogram(&ctx, relation, column.as_deref(), *buckets, query),
        Command::Stack {
            relation,
            by,
            order,
            query,
            page,
        } => read::stack(&ctx, relation, by, *order, query, page),
        Command::Find(args) => read::find(&ctx, args),
        Command::Around(args) => read::around(&ctx, args),
        Command::Row { id } => read::row(&ctx, *id),
        Command::Query { sql, page } => read::query(&ctx, sql, page),
        Command::Mark(MarkCmd::Set(a)) => annot::mark_set(&ctx, &a.rows),
        Command::Mark(MarkCmd::Clear(a)) => annot::mark_clear(&ctx, &a.rows),
        Command::Mark(MarkCmd::List { by, page }) => annot::mark_list(&ctx, by.as_deref(), page),
        Command::Tag(TagCmd::Add { rows, tag }) => annot::tag_add(&ctx, &rows.rows, tag),
        Command::Tag(TagCmd::Remove { rows, tag }) => annot::tag_remove(&ctx, &rows.rows, tag),
        Command::Tag(TagCmd::List { page }) => annot::tag_list(&ctx, page),
        Command::Note(NoteCmd::Add { row, text }) => annot::note_add(&ctx, *row, text),
        Command::Note(NoteCmd::Delete { id }) => annot::note_delete(&ctx, *id),
        Command::Note(NoteCmd::List { row, page }) => annot::note_list(&ctx, *row, page),
        Command::Column(ColumnCmd::Declare {
            relation,
            column,
            sql_type,
            semantics,
        }) => annot::declare(&ctx, relation, column, *sql_type, *semantics),
        Command::Column(ColumnCmd::PrimaryTime {
            relation,
            column,
            none,
        }) => annot::primary_time(&ctx, relation, column.as_deref().filter(|_| !*none)),
        Command::History { kind, page } => annot::history(&ctx, kind.as_deref(), page),
    }
}

/// `--as` or `OXIDELENS_ACTOR`, else the OS user name reduced to what an
/// actor name accepts.
fn resolve_actor(given: Option<&str>) -> Result<Actor> {
    if let Some(text) = given {
        return Actor::parse(text);
    }
    let user = std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" }).unwrap_or_default();
    let mut name: String = user
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    name.truncate(64);
    if name.is_empty() {
        return Ok(Actor::Analyst("unknown".to_string()));
    }
    Actor::parse(&format!("analyst:{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_actor_is_parsed() {
        assert_eq!(
            resolve_actor(Some("agent:claude")).unwrap(),
            Actor::Agent("claude".into())
        );
        assert!(resolve_actor(Some("root:x")).is_err());
    }
}
