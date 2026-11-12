use super::{Ctx, Reply};
use crate::envelope::wrap;
use oxl_engine::{Case, CaseInfo, Result};
use serde_json::json;
use std::path::Path;

/// A new case at `path`, or, without one, at the path `--case` names.
pub fn new(ctx: &Ctx, path: Option<&Path>, name: &str) -> Result<Reply> {
    let case = Case::create(ctx.new_case_path(path)?, name, &ctx.actor)?;
    Ok(info_reply(case.info()?))
}

pub fn info(ctx: &Ctx) -> Result<Reply> {
    let case = ctx.read_case()?;
    let mut reply = info_reply(case.info()?);
    let size = case.size()?;
    reply.data["bytes"] = size.bytes.into();
    reply.data["free_bytes"] = size.free_bytes.into();
    Ok(reply)
}

/// The case is only read, but opened as for a change: DuckDB writes the
/// copy only from a case opened that way, and no other process can then
/// change the case while it is copied.
pub fn compact(ctx: &Ctx, out: &Path) -> Result<Reply> {
    let case = ctx.open_case()?;
    let report = case.compact_to(out)?;
    Ok(Reply::new(json!({
        "bytes_before": report.bytes_before,
        "bytes_after": report.bytes_after,
        "untrusted": wrap(&json!({"path": report.path.to_string_lossy()})),
    })))
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
