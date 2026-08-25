use triage_core::tool::Tool;

pub struct ToolEntry {
    pub key: &'static str,
    pub tool: Box<dyn Tool>,
}

/// The full key -> tool-builder mapping. Shared by `all_tools()` (which
/// needs every tool at once, e.g. for --only/--skip validation and for
/// deriving the union of file-match patterns) and `tool_for_key()` (which
/// builds exactly one fresh tool by key, e.g. inside a worker thread that
/// can't share a `Box<dyn Tool>` across threads because `Tool` has no
/// `Sync` bound). Keeping one mapping means the two callers cannot drift.
fn builder_for_key(key: &str) -> Option<Box<dyn Tool>> {
    Some(match key {
        "pe" => Box::new(pe_triage::PeTool::default()),
        "jle" => Box::new(jle_triage::JleTool::default()),
        "le" => Box::new(le_triage::LeTool::default()),
        "rb" => Box::new(rb_triage::RbTool),
        "re" => Box::new(re_triage::RegistryTool::default()),
        "sbe" => Box::new(sbe_triage::ShellbagTool::default()),
        "sqle" => Box::new(sqle_triage::SqleTool::default()),
        "srum" => Box::new(srume_triage::SrumeTool::default()),
        "sum" => Box::new(sum_triage::SumTool),
        "wxt" => Box::new(wxt_triage::WxtTool),
        "evtx" => Box::new(evtx_triage::EvtxTool::default()),
        "mft" => Box::new(mft_triage::MftTool::default()),
        "amc" => Box::new(amc_triage::AmcacheTool::default()),
        "acc" => Box::new(acc_triage::AppCompatTool::default()),
        _ => return None,
    })
}

/// The stable short keys for every production parser, in registry order.
/// StubTool is intentionally excluded.
const ALL_KEYS: &[&str] = &[
    "pe", "jle", "le", "rb", "re", "sbe", "sqle", "srum", "sum", "wxt", "evtx", "mft", "amc", "acc",
];

/// Build a single tool by its `--only`/`--skip` key. Used by
/// `run_tools_bounded` to construct a fresh `ToolEntry` inside a worker
/// thread, since `Box<dyn Tool>` is not `Sync` and can't be shared by
/// reference across threads.
pub fn tool_for_key(key: &str) -> Option<ToolEntry> {
    tool_for_key_with_hunt(key, false)
}

pub fn tool_for_key_with_hunt(key: &str, hunt: bool) -> Option<ToolEntry> {
    let static_key = *ALL_KEYS.iter().find(|&&k| k == key)?;
    let tool: Box<dyn Tool> = if static_key == "sqle" && hunt {
        Box::new(sqle_triage::SqleTool::new(true, true, false))
    } else {
        builder_for_key(static_key)?
    };
    Some(ToolEntry {
        key: static_key,
        tool,
    })
}

/// Every production parser with a stable short key for --only/--skip.
/// StubTool is intentionally excluded.
pub fn all_tools() -> Vec<ToolEntry> {
    ALL_KEYS
        .iter()
        .map(|&key| ToolEntry {
            key,
            tool: builder_for_key(key).expect("ALL_KEYS entries must all have a builder"),
        })
        .collect()
}

pub fn select(only: &[String], skip: &[String]) -> Result<Vec<ToolEntry>, String> {
    select_with_hunt(only, skip, false)
}

pub fn select_with_hunt(
    only: &[String],
    skip: &[String],
    hunt: bool,
) -> Result<Vec<ToolEntry>, String> {
    let all = all_tools();
    let known: Vec<&str> = all.iter().map(|t| t.key).collect();
    for k in only.iter().chain(skip.iter()) {
        if !known.contains(&k.as_str()) {
            return Err(format!("unknown tool key: {k}"));
        }
    }
    Ok(all
        .into_iter()
        .filter(|t| {
            if only.is_empty() {
                t.key != "sqle"
            } else {
                only.iter().any(|k| k == t.key)
            }
        })
        .filter(|t| !skip.iter().any(|k| k == t.key))
        .map(|entry| {
            if entry.key == "sqle" && hunt {
                ToolEntry {
                    key: entry.key,
                    tool: Box::new(sqle_triage::SqleTool::new(true, true, false)),
                }
            } else {
                entry
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_all_parsers_with_unique_keys() {
        let tools = all_tools();
        assert_eq!(tools.len(), 14); // 15 tools minus StubTool
        let mut keys: Vec<&str> = tools.iter().map(|t| t.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 14, "keys must be unique");
    }

    #[test]
    fn select_only_and_skip_filter_and_validate() {
        assert_eq!(select(&["pe".into(), "mft".into()], &[]).unwrap().len(), 2);
        assert_eq!(select(&[], &["srum".into()]).unwrap().len(), 12);
        assert_eq!(select(&["sqle".into()], &[]).unwrap().len(), 1);
        assert!(select(&["nope".into()], &[]).is_err());
    }

    #[test]
    fn sqle_is_opt_in_and_hunt_expands_discovery() {
        assert!(select(&[], &[])
            .unwrap()
            .iter()
            .all(|entry| entry.key != "sqle"));
        let normal = select(&["sqle".into()], &[]).unwrap();
        assert_ne!(normal[0].tool.patterns(), &["*"]);
        let hunt = select_with_hunt(&["sqle".into()], &[], true).unwrap();
        assert_eq!(hunt[0].tool.patterns(), &["*"]);
    }
}
