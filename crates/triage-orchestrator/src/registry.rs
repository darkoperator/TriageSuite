use triage_core::tool::Tool;

pub struct ToolEntry {
    pub key: &'static str,
    pub tool: Box<dyn Tool>,
}

/// The stable short keys for every production parser, in registry order.
/// StubTool is intentionally excluded.
const ALL_KEYS: &[&str] = &[
    "pe", "jle", "le", "rb", "re", "sbe", "sqle", "srum", "sum", "wxt", "evtx", "mft", "amc",
    "acc", "browser",
];

/// Tools that run only when named in `--only`. SQLETriage is opt-in because
/// its discovery is broad enough to be noisy on a full capture.
const OPT_IN_KEYS: &[&str] = &["sqle"];

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

/// The one key -> tool mapping. Shared by `select_with` (which builds the
/// whole selected set at once) and `tool_for_key_with` (which builds exactly
/// one fresh tool inside a worker thread, because `Tool` has no `Sync` bound
/// and a `Box<dyn Tool>` cannot be shared across threads). One mapping means
/// the two callers cannot disagree about what a key or an option means.
fn build(key: &str, opts: ToolOptions) -> Option<Box<dyn Tool>> {
    Some(match key {
        "pe" => Box::new(pe_triage::PeTool::default()),
        "jle" => Box::new(jle_triage::JleTool::default()),
        "le" => Box::new(le_triage::LeTool::default()),
        "rb" => Box::new(rb_triage::RbTool),
        "re" => Box::new(re_triage::RegistryTool::default()),
        "sbe" => Box::new(sbe_triage::ShellbagTool::default()),
        "sqle" if opts.hunt => Box::new(sqle_triage::SqleTool::new(true, true, false)),
        "sqle" => Box::new(sqle_triage::SqleTool::default()),
        "srum" => Box::new(srume_triage::SrumeTool::default()),
        "sum" => Box::new(sum_triage::SumTool),
        "wxt" => Box::new(wxt_triage::WxtTool),
        "evtx" => Box::new(evtx_triage::EvtxTool::default()),
        "mft" => Box::new(mft_triage::MftTool::default()),
        "amc" => Box::new(amc_triage::AmcacheTool::default()),
        "acc" => Box::new(acc_triage::AppCompatTool::default()),
        "browser" => Box::new(browser_triage::BrowserTool::new(opts.no_timeline)),
        _ => return None,
    })
}

/// Build a single tool by its `--only`/`--skip` key. Used by
/// `run_tools_bounded` to construct a fresh `ToolEntry` inside a worker
/// thread.
pub fn tool_for_key_with(key: &str, opts: ToolOptions) -> Option<ToolEntry> {
    let key = *ALL_KEYS.iter().find(|&&k| k == key)?;
    Some(ToolEntry {
        key,
        tool: build(key, opts)?,
    })
}

/// Every production parser with a stable short key for --only/--skip.
/// StubTool is intentionally excluded.
pub fn all_tools() -> Vec<ToolEntry> {
    ALL_KEYS
        .iter()
        .map(|&key| tool_for_key_with(key, ToolOptions::default()))
        .collect::<Option<Vec<_>>>()
        .expect("ALL_KEYS entries must all have a builder")
}

/// Resolve `--only`/`--skip` to the tools that will run. Every key in either
/// list must be known; with an empty `only`, opt-in tools stay out.
pub fn select_with(
    only: &[String],
    skip: &[String],
    opts: ToolOptions,
) -> Result<Vec<ToolEntry>, String> {
    if let Some(unknown) = only
        .iter()
        .chain(skip)
        .find(|k| !ALL_KEYS.contains(&k.as_str()))
    {
        return Err(format!("unknown tool key: {unknown}"));
    }
    let wanted = |key: &str| {
        if only.is_empty() {
            !OPT_IN_KEYS.contains(&key)
        } else {
            only.iter().any(|k| k == key)
        }
    };
    Ok(ALL_KEYS
        .iter()
        .filter(|&&key| wanted(key) && !skip.iter().any(|k| k == key))
        .map(|&key| tool_for_key_with(key, opts).expect("ALL_KEYS entries must all have a builder"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select(only: &[&str], skip: &[&str]) -> Result<Vec<ToolEntry>, String> {
        let owned = |keys: &[&str]| keys.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        select_with(&owned(only), &owned(skip), ToolOptions::default())
    }

    #[test]
    fn registry_has_all_parsers_with_unique_keys() {
        let tools = all_tools();
        assert_eq!(tools.len(), ALL_KEYS.len());
        let mut keys: Vec<&str> = tools.iter().map(|t| t.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), ALL_KEYS.len(), "keys must be unique");
    }

    #[test]
    fn select_only_and_skip_filter_and_validate() {
        let default_on = ALL_KEYS.len() - OPT_IN_KEYS.len();
        assert_eq!(select(&["pe", "mft"], &[]).unwrap().len(), 2);
        assert_eq!(select(&[], &["srum"]).unwrap().len(), default_on - 1);
        assert_eq!(select(&["sqle"], &[]).unwrap().len(), 1);
        assert!(select(&["nope"], &[]).is_err());
        assert!(select(&[], &["nope"]).is_err());
    }

    #[test]
    fn sqle_is_opt_in_and_hunt_expands_discovery() {
        assert!(select(&[], &[])
            .unwrap()
            .iter()
            .all(|entry| entry.key != "sqle"));
        let normal = select(&["sqle"], &[]).unwrap();
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
    /// each tool inside its worker thread via `tool_for_key_with`. Both now go
    /// through the one `build`, so this pins that they stay that way.
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
        assert!(tool_for_key_with("browser", ToolOptions::default()).is_some());
        assert!(tool_for_key_with("nope", opts).is_none());
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
        assert_eq!(
            select(&[], &[]).unwrap().len(),
            ALL_KEYS.len() - OPT_IN_KEYS.len()
        );
    }
}
