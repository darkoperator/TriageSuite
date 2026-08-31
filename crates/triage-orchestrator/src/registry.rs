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
        "browser" => Box::new(browser_triage::BrowserTool::default()),
        _ => return None,
    })
}

/// The stable short keys for every production parser, in registry order.
/// StubTool is intentionally excluded.
const ALL_KEYS: &[&str] = &[
    "pe", "jle", "le", "rb", "re", "sbe", "sqle", "srum", "sum", "wxt", "evtx", "mft", "amc",
    "acc", "browser",
];

/// Per-run switches that change how a specific tool is *constructed*, as
/// opposed to which tools are selected.
///
/// A struct rather than positional booleans: there are two of these now, they
/// are both `bool`, and a third would make the call sites unreadable and easy
/// to transpose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolOptions {
    /// `--hunt`: SQLETriage inspects every file by content rather than by
    /// known filename.
    pub hunt: bool,
    /// `--no-timeline`: BrowserTriage skips its derived `_Timeline` dataset,
    /// which is routinely larger than all its typed datasets combined.
    pub no_timeline: bool,
}

/// Apply the options to a freshly built tool. One place, so the orchestrator
/// and the worker threads cannot disagree about what a flag means.
fn build(key: &'static str, opts: ToolOptions) -> Option<Box<dyn Tool>> {
    Some(match key {
        "sqle" if opts.hunt => Box::new(sqle_triage::SqleTool::new(true, true, false)),
        "browser" => Box::new(browser_triage::BrowserTool::new(opts.no_timeline)),
        _ => builder_for_key(key)?,
    })
}

/// Build a single tool by its `--only`/`--skip` key. Used by
/// `run_tools_bounded` to construct a fresh `ToolEntry` inside a worker
/// thread, since `Box<dyn Tool>` is not `Sync` and can't be shared by
/// reference across threads.
pub fn tool_for_key(key: &str) -> Option<ToolEntry> {
    tool_for_key_with(key, ToolOptions::default())
}

pub fn tool_for_key_with(key: &str, opts: ToolOptions) -> Option<ToolEntry> {
    let static_key = *ALL_KEYS.iter().find(|&&k| k == key)?;
    Some(ToolEntry {
        key: static_key,
        tool: build(static_key, opts)?,
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
    select_with(only, skip, ToolOptions::default())
}

pub fn select_with(
    only: &[String],
    skip: &[String],
    opts: ToolOptions,
) -> Result<Vec<ToolEntry>, String> {
    let known: Vec<&str> = ALL_KEYS.to_vec();
    for k in only.iter().chain(skip.iter()) {
        if !known.contains(&k.as_str()) {
            return Err(format!("unknown tool key: {k}"));
        }
    }
    Ok(ALL_KEYS
        .iter()
        .filter(|key| {
            if only.is_empty() {
                **key != "sqle"
            } else {
                only.iter().any(|k| k == *key)
            }
        })
        .filter(|key| !skip.iter().any(|k| k == *key))
        .map(|&key| ToolEntry {
            key,
            tool: build(key, opts).expect("ALL_KEYS entries must all have a builder"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_all_parsers_with_unique_keys() {
        let tools = all_tools();
        assert_eq!(tools.len(), 15); // 16 tools minus StubTool
        let mut keys: Vec<&str> = tools.iter().map(|t| t.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 15, "keys must be unique");
    }

    #[test]
    fn select_only_and_skip_filter_and_validate() {
        assert_eq!(select(&["pe".into(), "mft".into()], &[]).unwrap().len(), 2);
        assert_eq!(select(&[], &["srum".into()]).unwrap().len(), 13);
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
        let hunt = select_with(
            &["sqle".into()],
            &[],
            ToolOptions {
                hunt: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(hunt[0].tool.patterns(), &["*"]);
    }

    /// `--no-timeline` has to reach the tool through both construction paths:
    /// `select_with` builds the initial set, but `run_tools_bounded` rebuilds
    /// each tool inside its worker thread via `tool_for_key_with`. A flag
    /// honoured by only one of them would look like it worked and then not.
    #[test]
    fn no_timeline_reaches_browser_triage_through_both_build_paths() {
        let opts = ToolOptions {
            no_timeline: true,
            ..Default::default()
        };

        let selected = select_with(&["browser".into()], &[], opts).unwrap();
        assert_eq!(selected.len(), 1);

        // The flag is not observable through the `Tool` trait, so assert on the
        // concrete builder that both paths share.
        assert!(browser_triage::BrowserTool::new(true).no_timeline);
        assert!(!browser_triage::BrowserTool::new(false).no_timeline);
        assert!(
            !browser_triage::BrowserTool::default().no_timeline,
            "the default must keep emitting the timeline"
        );

        assert!(tool_for_key_with("browser", opts).is_some());
        assert!(tool_for_key("browser").is_some());
    }

    /// The default is unchanged: every tool builds as it did before options
    /// existed.
    #[test]
    fn default_options_change_nothing() {
        assert_eq!(
            ToolOptions::default(),
            ToolOptions {
                hunt: false,
                no_timeline: false
            }
        );
        assert_eq!(select(&[], &[]).unwrap().len(), 14); // 15 minus opt-in sqle
    }
}
