use super::{Ctx, Reply};
use crate::envelope::wrap;
use oxl_engine::{FileOptions, LoadReport, Result, RunOptions};
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;

pub fn run(ctx: &Ctx, out_root: &Path, per_channel: bool) -> Result<Reply> {
    let mut case = ctx.open_case()?;
    Ok(report_reply(oxl_engine::load_run_with(
        &mut case,
        out_root,
        RunOptions { per_channel },
        &ctx.actor,
    )?))
}

pub fn file(ctx: &Ctx, path: &Path, name: Option<String>, host: Option<String>) -> Result<Reply> {
    let mut case = ctx.open_case()?;
    let opts = FileOptions { name, host };
    Ok(report_reply(oxl_engine::load_file(
        &mut case, path, opts, &ctx.actor,
    )?))
}

fn report_reply(report: LoadReport) -> Reply {
    let hosts: BTreeSet<&String> = report.relations.iter().flat_map(|r| &r.hosts).collect();
    let rows: u64 = report.relations.iter().map(|r| r.rows).sum();
    let relations: Vec<_> = report
        .relations
        .iter()
        .map(|r| json!({"name": r.name, "hosts": r.hosts, "rows": r.rows}))
        .collect();
    let superseded: Vec<_> = report
        .superseded
        .iter()
        .map(|s| {
            json!({"relation": s.relation, "host": s.host, "old_source": s.old_source,
                   "moved": s.moved, "left_behind": s.left_behind})
        })
        .collect();
    // A skipped dataset's id and relation name carry a channel name from
    // the evidence, so the list goes inside the wrapper; its length does not.
    let skipped: Vec<_> = report
        .skipped
        .iter()
        .map(|s| {
            json!({"relation": s.relation, "tool": s.tool, "dataset_id": s.dataset_id,
                   "hosts": s.hosts})
        })
        .collect();
    Reply::new(json!({
        "source_id": report.source_id,
        "relations": relations.len(),
        "rows": rows,
        "superseded": superseded.len(),
        "skipped": skipped.len(),
        "untrusted": wrap(&json!({
            "relations": relations,
            "hosts": hosts,
            "superseded": superseded,
            "warnings": report.warnings,
            "skipped": skipped,
        })),
    }))
}

pub fn sources(ctx: &Ctx) -> Result<Reply> {
    let case = ctx.read_case()?;
    let list: Vec<_> = oxl_engine::sources(&case)?
        .into_iter()
        .map(|s| {
            json!({"id": s.id, "kind": s.kind, "path": s.path, "run_id": s.run_id,
                   "hosts": s.hosts, "loaded_at": s.loaded_at, "loaded_by": s.loaded_by,
                   "removed": s.removed})
        })
        .collect();
    Ok(Reply::new(json!({
        "sources": list.len(),
        "untrusted": wrap(&json!({"sources": list})),
    })))
}

pub fn remove(ctx: &Ctx, id: i64, confirm_orphans: bool) -> Result<Reply> {
    let mut case = ctx.open_case()?;
    let report = oxl_engine::remove_source(&mut case, id, confirm_orphans, &ctx.actor)?;
    Ok(Reply::new(json!({
        "orphaned_marks": report.orphaned_marks,
        "orphaned_tags": report.orphaned_tags,
        "orphaned_notes": report.orphaned_notes,
        "orphaned_evidence": report.orphaned_evidence,
    })))
}
