//! One function per command: parsed arguments in, envelope data or an
//! `EngineError` out. `execute` is the only place that knows every command.

mod case;
mod describe;
mod load;

use crate::args::{CaseCmd, Cli, Command, LoadCmd, SourceCmd};
use oxl_engine::{Actor, Case, EngineError, Result};
use serde_json::Value;
use std::path::PathBuf;

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
            None => Err(EngineError::Invalid(
                "no case: pass --case <path> or set OXIDELENS_CASE".to_string(),
            )),
        }
    }
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
