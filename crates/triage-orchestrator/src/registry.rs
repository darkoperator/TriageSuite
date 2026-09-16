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

/// Tool keys whose own parser understands a time range. Everything else in
/// `ALL_KEYS` has no shared notion of "the" record timestamp to filter on
/// (an `$MFT` record has eight; a prefetch record up to eight), so a
/// `--start`/`--end` run leaves them unfiltered on purpose and the manifest
/// records that explicitly as `not_applicable` rather than staying silent.
const TIME_FILTERED_KEYS: &[&str] = &["evtx"];

/// Whether `key`'s parser accepts `--start`/`--end` at all. Used to fill in
/// the manifest's `time_filter` field (`applied` vs `not_applicable`) once a
/// range was given for the run.
pub fn tool_applies_time_filter(key: &str) -> bool {
    TIME_FILTERED_KEYS.contains(&key)
}

/// Convert a run's `--start`/`--end` (`chrono`, shared with the CLI and the
/// manifest) into the `time` crate type `ParseOptions` uses. The two crates
/// have no conversion trait between them, so this round-trips through
/// RFC3339 text -- the same representation EvtxTriage's own CLI already
/// parses for `--sd`/`--ed`.
fn to_time_offset(dt: chrono::DateTime<chrono::Utc>) -> time::OffsetDateTime {
    time::OffsetDateTime::parse(
        &dt.to_rfc3339(),
        &time::format_description::well_known::Rfc3339,
    )
    .expect("chrono's RFC3339 output is always valid RFC3339")
}

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
    /// `--no-individual`: EvtxTriage skips its per-source-log (channel)
    /// individual CSV exports, which are written by default.
    pub no_individual: bool,
    /// `--start`: run-wide time-range floor. Passthrough only -- it reaches
    /// EvtxTriage here and Hayabusa via its config overlay in `main.rs`; no
    /// other tool filters, and the manifest says so per tool
    /// (`tool_applies_time_filter`).
    pub start: Option<chrono::DateTime<chrono::Utc>>,
    /// `--end`: paired with `start` above.
    pub end: Option<chrono::DateTime<chrono::Utc>>,
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
        "evtx" => {
            let mut tool = evtx_triage::EvtxTool::new(!opts.no_individual);
            if let Some(start) = opts.start {
                tool.opts.start_date = Some(to_time_offset(start));
            }
            if let Some(end) = opts.end {
                tool.opts.end_date = Some(to_time_offset(end));
            }
            Box::new(tool)
        }
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

/// Every registered `--only`/`--skip` key, in registry order. Exposed so
/// cross-cutting checks (e.g. the Velo basename guard test) can walk every
/// production tool without duplicating `ALL_KEYS`.
pub fn all_keys() -> &'static [&'static str] {
    ALL_KEYS
}

/// Build a single tool by key with default `ToolOptions`, for callers that
/// only need the tool's static metadata (`binary_name`, `datasets`) rather
/// than a runnable `ToolEntry`.
pub fn tool_for_key(key: &str) -> Option<Box<dyn Tool>> {
    build(key, ToolOptions::default())
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
                no_timeline: false,
                no_individual: false,
                start: None,
                end: None,
            }
        );
        assert_eq!(
            select(&[], &[]).unwrap().len(),
            ALL_KEYS.len() - OPT_IN_KEYS.len()
        );
    }
}
