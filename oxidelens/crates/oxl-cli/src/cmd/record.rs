//! The case-record commands: timeline entries and their evidence, the host,
//! account and IOC records, links, candidates, sightings, logon summaries,
//! exports. Every rule (who may change what, what a field may hold, where an
//! entry's time comes from) is the engine's; this file parses arguments and
//! shapes replies.
//!
//! What goes where in a reply. Outside the wrapper: ids (and the `E-12`,
//! `H-3`, `A-7`, `I-40` form of them), counts, flags, statuses, times, actor
//! labels, tactics, ATT&CK ids and IOC types, which the engine mints or
//! validates against a fixed alphabet. Inside `untrusted`: every text that
//! can come from evidence or from the caller's free text: titles,
//! descriptions, notes, host, account and IOC values, evidence relation,
//! host and summary text, candidate values, sighting relations and hosts,
//! logon hosts and filters, and the export paths. Each item's text keeps the
//! `id` (or `row_id`, `index`) of its safe half, so the two can be joined.

use super::annot::{outcomes_reply, parse_rows};
use super::read::text_arg;
use super::{Ctx, Reply};
use crate::args::{
    record_id_of, AccountCmd, EntryCmd, EntryStatusArg, EvidenceCmd, ExportCmd, HostCmd, IocCmd,
    LinkArgs, LinkCmd, PageArgs, RecordStatusArg,
};
use crate::envelope::wrap;
use oxl_engine::{
    AccountSpec, EntryEdit, EntryQuery, EntryStatus, HostSpec, IocQuery, IocSpec, IocType,
    Privilege, RecordKind, RecordStatus, Result,
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::atomic::AtomicBool;

const ENTRY_TEXT: &[&str] = &["title", "description", "hosts", "links"];
const HOST_TEXT: &[&str] = &["name", "role", "note"];
const ACCOUNT_TEXT: &[&str] = &["name", "domain", "sid", "description", "note", "hosts_seen"];
const IOC_TEXT: &[&str] = &["value", "tool", "note", "hosts_seen"];
const EVIDENCE_TEXT: &[&str] = &["relation", "host", "summary"];
const CANDIDATE_TEXT: &[&str] = &["value", "domain", "sid", "column"];

/// An item split in two: what the tool produced, and the text that may not
/// be trusted. Both carry `key`'s value to join them.
struct Parts {
    safe: Map<String, Value>,
    text: Map<String, Value>,
}

fn parts(item: &impl Serialize, key: &str, texts: &[&str]) -> Parts {
    let Ok(Value::Object(mut all)) = serde_json::to_value(item) else {
        // Every item here is a struct of named fields.
        unreachable!("a record item serializes to an object");
    };
    let mut text = Map::new();
    if let Some(id) = all.get(key) {
        text.insert(key.to_string(), id.clone());
    }
    for name in texts {
        if let Some(v) = all.remove(*name) {
            text.insert((*name).to_string(), v);
        }
    }
    Parts { safe: all, text }
}

/// `E-12` for entry 12 and so on, in `safe`'s `shown` key.
fn with_shown(mut p: Parts, prefix: &str) -> Parts {
    if let Some(id) = p.safe.get("id").and_then(Value::as_i64) {
        p.safe
            .insert("shown".to_string(), json!(format!("{prefix}-{id}")));
    }
    p
}

fn entry_parts(e: &oxl_engine::Entry) -> Parts {
    with_shown(parts(e, "id", ENTRY_TEXT), "E")
}

fn record_parts(item: &impl Serialize, kind: RecordKind, texts: &[&str]) -> Parts {
    with_shown(parts(item, "id", texts), kind.prefix())
}

fn one_reply(p: Parts) -> Reply {
    let mut data = p.safe;
    data.insert("untrusted".to_string(), json!(wrap(&Value::Object(p.text))));
    Reply::new(Value::Object(data))
}

/// A list: the safe halves under `name`, the text halves in the wrapper
/// under the same name.
fn list_reply(name: &str, items: Vec<Parts>, truncated: bool) -> Reply {
    let count = items.len();
    let (safe, text): (Vec<Value>, Vec<Value>) = items
        .into_iter()
        .map(|p| (Value::Object(p.safe), Value::Object(p.text)))
        .unzip();
    Reply {
        data: json!({
            "count": count,
            name: safe,
            "untrusted": wrap(&json!({ name: text })),
        }),
        truncated,
    }
}

/// `items`'s page for `--limit` and `--offset`, for a list the engine
/// returns whole.
fn slice<T>(items: Vec<T>, page: &PageArgs) -> (Vec<T>, bool) {
    let offset = usize::try_from(page.offset).unwrap_or(usize::MAX);
    let limit = page.limit as usize;
    let mut rest = items.into_iter().skip(offset);
    let kept: Vec<T> = rest.by_ref().take(limit).collect();
    (kept, rest.next().is_some())
}

fn text_opt(value: Option<&String>) -> Result<Option<String>> {
    value.map(|t| text_arg(t)).transpose()
}

fn entry_status(s: EntryStatusArg) -> EntryStatus {
    match s {
        EntryStatusArg::Draft => EntryStatus::Draft,
        EntryStatusArg::Unreviewed => EntryStatus::Unreviewed,
        EntryStatusArg::Reviewed => EntryStatus::Reviewed,
    }
}

fn record_status(s: RecordStatusArg) -> RecordStatus {
    match s {
        RecordStatusArg::Unreviewed => RecordStatus::Unreviewed,
        RecordStatusArg::Reviewed => RecordStatus::Reviewed,
    }
}

/// An empty value clears: `Some(None)`; absent leaves as it is: `None`.
fn clearable(value: &Option<String>) -> Option<Option<String>> {
    value
        .as_ref()
        .map(|v| if v.is_empty() { None } else { Some(v.clone()) })
}

// ---- entries ----

pub fn entry(ctx: &Ctx, cmd: &EntryCmd) -> Result<Reply> {
    match cmd {
        EntryCmd::List {
            status,
            host,
            tactic,
            text,
            page,
        } => {
            let q = EntryQuery {
                status: status.iter().copied().map(entry_status).collect(),
                host: host.clone(),
                tactic: tactic.clone(),
                text: text.clone(),
                row: None,
                order: None,
            };
            let case = ctx.read_case()?;
            let list = oxl_engine::entries(&case, &q, page.limit, page.offset)?;
            let items = list.items.iter().map(entry_parts).collect();
            Ok(list_reply("entries", items, list.truncated))
        }
        EntryCmd::Show { id } => entry_show(ctx, *id),
        EntryCmd::Add {
            title,
            description,
            time,
            tactic,
            techniques,
        } => {
            let edit = EntryEdit {
                title: Some(title.clone()),
                description: text_opt(description.as_ref())?,
                time: clearable(time),
                tactic: clearable(tactic),
                technique_ids: (!techniques.is_empty()).then(|| techniques.clone()),
            };
            let mut case = ctx.open_case()?;
            let e = oxl_engine::new_entry(&mut case, &edit, &ctx.actor)?;
            Ok(one_reply(entry_parts(&e)))
        }
        EntryCmd::Edit {
            id,
            title,
            description,
            time,
            tactic,
            techniques,
            clear_techniques,
        } => {
            let edit = EntryEdit {
                title: title.clone(),
                description: text_opt(description.as_ref())?,
                time: clearable(time),
                tactic: clearable(tactic),
                technique_ids: if *clear_techniques {
                    Some(Vec::new())
                } else {
                    (!techniques.is_empty()).then(|| techniques.clone())
                },
            };
            let mut case = ctx.open_case()?;
            let e = oxl_engine::edit_entry(&mut case, *id, &edit, &ctx.actor)?;
            Ok(one_reply(entry_parts(&e)))
        }
        EntryCmd::Status { id, status } => {
            let mut case = ctx.open_case()?;
            let e =
                oxl_engine::set_entry_status(&mut case, *id, entry_status(*status), &ctx.actor)?;
            Ok(one_reply(entry_parts(&e)))
        }
        EntryCmd::Delete { id } => {
            let mut case = ctx.open_case()?;
            oxl_engine::delete_entry(&mut case, *id, &ctx.actor)?;
            Ok(Reply::new(json!({"id": id, "shown": format!("E-{id}")})))
        }
        EntryCmd::Promote(rows) => {
            let rows = parse_rows(&rows.rows)?;
            let mut case = ctx.open_case()?;
            let outcomes = oxl_engine::promote_rows(&mut case, &rows, &ctx.actor)?;
            let made = outcomes.iter().filter(|o| o.created).count();
            let done = outcomes.iter().filter(|o| o.entry_id.is_some()).count();
            Ok(Reply::new(json!({
                "done": done,
                "created": made,
                "not_done": outcomes.len() - done,
                "outcomes": outcomes,
            })))
        }
        EntryCmd::Attach { id, rows } => {
            let rows = parse_rows(&rows.rows)?;
            let mut case = ctx.open_case()?;
            let outcomes = oxl_engine::attach_rows(&mut case, *id, &rows, &ctx.actor)?;
            Ok(outcomes_reply(outcomes))
        }
        EntryCmd::Detach { id, row } => {
            let mut case = ctx.open_case()?;
            oxl_engine::detach_row(&mut case, *id, *row, &ctx.actor)?;
            Ok(Reply::new(json!({"entry": id, "row_id": row})))
        }
    }
}

/// An entry and every row attached to it, one read.
fn entry_show(ctx: &Ctx, id: i64) -> Result<Reply> {
    let case = ctx.read_case()?;
    let entry = oxl_engine::entry(&case, id)?;
    let evidence = oxl_engine::evidence(&case, id)?;
    let mut p = entry_parts(&entry);
    let rows: Vec<Parts> = evidence
        .iter()
        .map(|e| parts(e, "row_id", EVIDENCE_TEXT))
        .collect();
    p.safe.insert("evidence".to_string(), safe_array(&rows));
    p.text.insert("evidence".to_string(), text_array(&rows));
    Ok(one_reply(p))
}

fn safe_array(items: &[Parts]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|p| Value::Object(p.safe.clone()))
            .collect(),
    )
}

fn text_array(items: &[Parts]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|p| Value::Object(p.text.clone()))
            .collect(),
    )
}

pub fn evidence(ctx: &Ctx, cmd: &EvidenceCmd) -> Result<Reply> {
    let EvidenceCmd::List { entry, page } = cmd;
    let case = ctx.read_case()?;
    // The entry must exist, as `entry show` has it.
    oxl_engine::entry(&case, *entry)?;
    let all = oxl_engine::evidence(&case, *entry)?;
    let (kept, truncated) = slice(all, page);
    let items = kept
        .iter()
        .map(|e| parts(e, "row_id", EVIDENCE_TEXT))
        .collect();
    Ok(list_reply("evidence", items, truncated))
}

// ---- records ----

/// A record and a page of the entries linked to it.
fn show_record(ctx: &Ctx, kind: RecordKind, id: i64, page: &PageArgs) -> Result<Reply> {
    let case = ctx.read_case()?;
    let mut p = match kind {
        RecordKind::Host => record_parts(&oxl_engine::host_record(&case, id)?, kind, HOST_TEXT),
        RecordKind::Account => {
            record_parts(&oxl_engine::account_record(&case, id)?, kind, ACCOUNT_TEXT)
        }
        RecordKind::Ioc => record_parts(&oxl_engine::ioc_record(&case, id)?, kind, IOC_TEXT),
    };
    let linked = oxl_engine::record_entries(&case, kind, id, page.limit, page.offset)?;
    let items: Vec<Parts> = linked.items.iter().map(entry_parts).collect();
    p.safe
        .insert("linked_entries".to_string(), safe_array(&items));
    p.text
        .insert("linked_entries".to_string(), text_array(&items));
    let mut reply = one_reply(p);
    reply.truncated = linked.truncated;
    Ok(reply)
}

fn delete_record(ctx: &Ctx, kind: RecordKind, id: i64) -> Result<Reply> {
    let mut case = ctx.open_case()?;
    oxl_engine::delete_record(&mut case, kind, id, &ctx.actor)?;
    Ok(Reply::new(
        json!({"id": id, "shown": kind.prefix().to_string() + "-" + &id.to_string()}),
    ))
}

fn merge_records(ctx: &Ctx, kind: RecordKind, from: i64, into: i64) -> Result<Reply> {
    let mut case = ctx.open_case()?;
    oxl_engine::merge_records(&mut case, kind, from, into, &ctx.actor)?;
    Ok(Reply::new(json!({"from": from, "into": into})))
}

fn status_record(ctx: &Ctx, kind: RecordKind, id: i64, status: RecordStatusArg) -> Result<Reply> {
    let status = record_status(status);
    let mut case = ctx.open_case()?;
    oxl_engine::set_record_status(&mut case, kind, id, status, &ctx.actor)?;
    Ok(Reply::new(
        json!({"id": id, "shown": format!("{}-{id}", kind.prefix()), "status": status}),
    ))
}

pub fn host(ctx: &Ctx, cmd: &HostCmd) -> Result<Reply> {
    let kind = RecordKind::Host;
    match cmd {
        HostCmd::List { page } => {
            let case = ctx.read_case()?;
            let list = oxl_engine::hosts(&case, page.limit, page.offset)?;
            let items = list
                .items
                .iter()
                .map(|h| record_parts(h, kind, HOST_TEXT))
                .collect();
            Ok(list_reply("hosts", items, list.truncated))
        }
        HostCmd::Show { id, page } => show_record(ctx, kind, *id, page),
        HostCmd::Add { name, role, note } => {
            let spec = HostSpec {
                id: None,
                name: name.clone(),
                role: role.clone().unwrap_or_default(),
                note: text_opt(note.as_ref())?.unwrap_or_default(),
            };
            let mut case = ctx.open_case()?;
            let h = oxl_engine::upsert_host(&mut case, &spec, &ctx.actor)?;
            Ok(one_reply(record_parts(&h, kind, HOST_TEXT)))
        }
        HostCmd::Edit {
            id,
            name,
            role,
            note,
        } => {
            let note = text_opt(note.as_ref())?;
            let mut case = ctx.open_case()?;
            let old = oxl_engine::host_record(&case, *id)?;
            let spec = HostSpec {
                id: Some(*id),
                name: name.clone().unwrap_or(old.name),
                role: role.clone().unwrap_or(old.role),
                note: note.unwrap_or(old.note),
            };
            let h = oxl_engine::upsert_host(&mut case, &spec, &ctx.actor)?;
            Ok(one_reply(record_parts(&h, kind, HOST_TEXT)))
        }
        HostCmd::Delete { id } => delete_record(ctx, kind, *id),
        HostCmd::Merge { from, into } => merge_records(ctx, kind, *from, *into),
        HostCmd::Status { id, status } => status_record(ctx, kind, *id, *status),
    }
}

pub fn account(ctx: &Ctx, cmd: &AccountCmd) -> Result<Reply> {
    let kind = RecordKind::Account;
    match cmd {
        AccountCmd::List { page } => {
            let case = ctx.read_case()?;
            let list = oxl_engine::accounts(&case, page.limit, page.offset)?;
            let items = list
                .items
                .iter()
                .map(|a| record_parts(a, kind, ACCOUNT_TEXT))
                .collect();
            Ok(list_reply("accounts", items, list.truncated))
        }
        AccountCmd::Show { id, page } => show_record(ctx, kind, *id, page),
        AccountCmd::Add {
            name,
            domain,
            sid,
            description,
            privilege,
            note,
        } => {
            let spec = AccountSpec {
                id: None,
                name: name.clone(),
                domain: domain.clone(),
                sid: sid.clone(),
                description: text_opt(description.as_ref())?.unwrap_or_default(),
                privilege: privilege
                    .as_deref()
                    .map(Privilege::parse)
                    .transpose()?
                    .unwrap_or_default(),
                note: text_opt(note.as_ref())?.unwrap_or_default(),
            };
            let mut case = ctx.open_case()?;
            let a = oxl_engine::upsert_account(&mut case, &spec, &ctx.actor)?;
            Ok(one_reply(record_parts(&a, kind, ACCOUNT_TEXT)))
        }
        AccountCmd::Edit {
            id,
            name,
            domain,
            sid,
            description,
            privilege,
            note,
        } => {
            let description = text_opt(description.as_ref())?;
            let note = text_opt(note.as_ref())?;
            let privilege = privilege.as_deref().map(Privilege::parse).transpose()?;
            let mut case = ctx.open_case()?;
            let old = oxl_engine::account_record(&case, *id)?;
            let spec = AccountSpec {
                id: Some(*id),
                name: name.clone().unwrap_or(old.name),
                domain: domain.clone().or(old.domain),
                sid: sid.clone().or(old.sid),
                description: description.unwrap_or(old.description),
                privilege: privilege.unwrap_or(old.privilege),
                note: note.unwrap_or(old.note),
            };
            let a = oxl_engine::upsert_account(&mut case, &spec, &ctx.actor)?;
            Ok(one_reply(record_parts(&a, kind, ACCOUNT_TEXT)))
        }
        AccountCmd::Delete { id } => delete_record(ctx, kind, *id),
        AccountCmd::Merge { from, into } => merge_records(ctx, kind, *from, *into),
        AccountCmd::Status { id, status } => status_record(ctx, kind, *id, *status),
        AccountCmd::Logons { id } => logons(ctx, *id),
    }
}

pub fn ioc(ctx: &Ctx, cmd: &IocCmd) -> Result<Reply> {
    let kind = RecordKind::Ioc;
    match cmd {
        IocCmd::List {
            ioc_type,
            tool,
            text,
            page,
        } => {
            let q = IocQuery {
                ioc_type: ioc_type.as_deref().map(IocType::parse).transpose()?,
                tool: tool.clone(),
                text: text.clone(),
            };
            let case = ctx.read_case()?;
            let list = oxl_engine::iocs(&case, &q, page.limit, page.offset)?;
            let items = list
                .items
                .iter()
                .map(|i| record_parts(i, kind, IOC_TEXT))
                .collect();
            Ok(list_reply("iocs", items, list.truncated))
        }
        IocCmd::Show { id, page } => show_record(ctx, kind, *id, page),
        IocCmd::Add {
            ioc_type,
            value,
            tool,
            attack_ids,
            note,
        } => {
            let spec = IocSpec {
                id: None,
                ioc_type: IocType::parse(ioc_type)?,
                value: value.clone(),
                tool: tool.clone(),
                attack_ids: attack_ids.clone(),
                note: text_opt(note.as_ref())?.unwrap_or_default(),
            };
            let mut case = ctx.open_case()?;
            let i = oxl_engine::upsert_ioc(&mut case, &spec, &ctx.actor)?;
            Ok(one_reply(record_parts(&i, kind, IOC_TEXT)))
        }
        IocCmd::Edit {
            id,
            ioc_type,
            value,
            tool,
            attack_ids,
            clear_attack_ids,
            note,
        } => {
            let note = text_opt(note.as_ref())?;
            let ioc_type = ioc_type.as_deref().map(IocType::parse).transpose()?;
            let mut case = ctx.open_case()?;
            let old = oxl_engine::ioc_record(&case, *id)?;
            let spec = IocSpec {
                id: Some(*id),
                ioc_type: ioc_type.unwrap_or(old.ioc_type),
                value: value.clone().unwrap_or(old.value),
                tool: tool.clone().or(old.tool),
                attack_ids: if *clear_attack_ids {
                    Vec::new()
                } else if attack_ids.is_empty() {
                    old.attack_ids
                } else {
                    attack_ids.clone()
                },
                note: note.unwrap_or(old.note),
            };
            let i = oxl_engine::upsert_ioc(&mut case, &spec, &ctx.actor)?;
            Ok(one_reply(record_parts(&i, kind, IOC_TEXT)))
        }
        IocCmd::Delete { id } => delete_record(ctx, kind, *id),
        IocCmd::Merge { from, into } => merge_records(ctx, kind, *from, *into),
        IocCmd::Status { id, status } => status_record(ctx, kind, *id, *status),
        IocCmd::Sightings { id } => sightings(ctx, *id),
    }
}

// ---- links ----

pub fn link(ctx: &Ctx, cmd: &LinkCmd) -> Result<Reply> {
    let (args, adding) = match cmd {
        LinkCmd::Add(a) => (a, true),
        LinkCmd::Remove(a) => (a, false),
    };
    let LinkArgs { entry, kind, id } = args;
    let kind = RecordKind::parse(kind)?;
    let record = record_id_of(kind, id)?;
    let mut case = ctx.open_case()?;
    if adding {
        oxl_engine::link(&mut case, *entry, kind, record, &ctx.actor)?;
    } else {
        oxl_engine::unlink(&mut case, *entry, kind, record, &ctx.actor)?;
    }
    Ok(Reply::new(json!({
        "entry": entry,
        "kind": kind,
        "id": record,
        "linked": adding,
    })))
}

// ---- reads over a row, an IOC, an account ----

pub fn candidates(ctx: &Ctx, row: i64) -> Result<Reply> {
    let case = ctx.read_case()?;
    let found = oxl_engine::candidates(&case, row)?;
    let items: Vec<Parts> = found
        .iter()
        .enumerate()
        .map(|(index, c)| {
            let mut value = serde_json::to_value(c).unwrap_or(Value::Null);
            if let Value::Object(map) = &mut value {
                map.insert("index".to_string(), json!(index));
            }
            parts(&value, "index", CANDIDATE_TEXT)
        })
        .collect();
    Ok(list_reply("candidates", items, false))
}

fn sightings(ctx: &Ctx, id: i64) -> Result<Reply> {
    let case = ctx.read_case()?;
    let s = oxl_engine::ioc_sightings(&case, id, &AtomicBool::new(false))?;
    Ok(Reply::new(json!({
        "id": id,
        "hits": s.hits,
        "first": s.first,
        "last": s.last,
        "untrusted": wrap(&json!({"relations": s.relations, "hosts": s.hosts})),
    })))
}

fn logons(ctx: &Ctx, id: i64) -> Result<Reply> {
    let case = ctx.read_case()?;
    let summary = oxl_engine::logon_summary(&case, id)?;
    let counts: Vec<Value> = summary
        .rows
        .iter()
        .enumerate()
        .map(|(index, r)| {
            json!({"index": index, "outcome": r.outcome, "logon_type": r.logon_type,
                   "count": r.count})
        })
        .collect();
    let text: Vec<Value> = summary
        .rows
        .iter()
        .enumerate()
        .map(|(index, r)| json!({"index": index, "host": r.host, "filter": r.filter}))
        .collect();
    Ok(Reply::new(json!({
        "id": id,
        "count": counts.len(),
        "logons": counts,
        "unparsed": summary.unparsed,
        "untrusted": wrap(&json!({"source": summary.source, "logons": text})),
    })))
}

// ---- exports ----

pub fn export(ctx: &Ctx, cmd: &ExportCmd) -> Result<Reply> {
    let cancel = AtomicBool::new(false);
    let (path, report): (&Path, _) = match cmd {
        ExportCmd::Timeline { path } => {
            let case = ctx.read_case()?;
            (path, oxl_engine::export_timeline_csv(&case, path, &cancel)?)
        }
        ExportCmd::Workbook { path } => {
            let case = ctx.read_case()?;
            (path, oxl_engine::export_workbook(&case, path, &cancel)?)
        }
        ExportCmd::Report { path } => {
            let case = ctx.read_case()?;
            (path, oxl_engine::export_report(&case, path, &cancel)?)
        }
    };
    Ok(Reply::new(json!({
        "rows": report.rows,
        "untrusted": wrap(&json!({"path": path.to_string_lossy()})),
    })))
}
