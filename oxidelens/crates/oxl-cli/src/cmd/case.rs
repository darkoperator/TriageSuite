use super::{Ctx, Reply};
use crate::envelope::wrap;
use oxl_engine::{Case, CaseInfo, Result};
use serde_json::json;
use std::path::Path;

pub fn new(ctx: &Ctx, path: &Path, name: &str) -> Result<Reply> {
    let case = Case::create(path, name, &ctx.actor)?;
    Ok(info_reply(case.info()?))
}

pub fn info(ctx: &Ctx) -> Result<Reply> {
    Ok(info_reply(ctx.read_case()?.info()?))
}

fn info_reply(info: CaseInfo) -> Reply {
    Reply::new(json!({
        "format": info.format,
        "created_by": info.created_by,
        "created_at": info.created_at,
        "untrusted": wrap(&json!({
            "name": info.name,
            "path": info.path.to_string_lossy(),
        })),
    }))
}
