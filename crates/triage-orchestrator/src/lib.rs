//! TriageSuite orchestrator: detect a capture, run every parser over it,
//! fan out across hosts, and emit a chain-of-custody manifest.

pub mod archive;
pub mod capture;
pub mod duckdb;
pub mod execute;
pub mod external;
pub mod input;
pub mod manifest;
pub mod progress_ui;
pub mod registry;
pub mod validate;
pub mod velo;

/// How many per-file skip reasons a report keeps. Enough to diagnose a
/// pattern, few enough that a run over thousands of unsupported files still
/// produces a readable manifest.
pub(crate) const MAX_REASON_SAMPLES: usize = 10;

/// The final path component, lossily decoded. Empty when the path has none
/// (`/`, `..`).
pub fn file_name_lossy(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The plain-language statement a `--start`/`--end` run must carry
/// everywhere an analyst might read partial context. Exactly one in-process
/// parser (EvtxTriage) and two external tools (Hayabusa directly, Takajo by
/// inheriting Hayabusa's already-filtered JSONL) see this range; every other
/// tool in the run emits its full, unfiltered output regardless. `None` when
/// no range was given, matching how the flag simply did not apply to
/// anything this run.
pub fn time_range_notice(
    start: Option<chrono::DateTime<chrono::Utc>>,
    end: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<String> {
    if start.is_none() && end.is_none() {
        return None;
    }
    let fmt =
        |dt: chrono::DateTime<chrono::Utc>| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let range = match (start, end) {
        (Some(s), Some(e)) => format!("{} .. {}", fmt(s), fmt(e)),
        (Some(s), None) => format!("{} .. (open end)", fmt(s)),
        (None, Some(e)) => format!("(open start) .. {}", fmt(e)),
        (None, None) => unreachable!("checked above"),
    };
    Some(format!(
        "Time range {range} applies to event logs only (EvtxTriage, \
         Hayabusa, Takajo). All other tools emit their full output."
    ))
}

#[cfg(test)]
mod time_range_notice_tests {
    use super::time_range_notice;
    use chrono::TimeZone;

    fn at(hour: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .with_ymd_and_hms(2026, 3, 13, hour, 0, 0)
            .single()
            .expect("a real, unambiguous UTC instant")
    }

    /// All four arms. The two one-sided ones are reachable from the CLI
    /// (`--start` alone, `--end` alone) and each renders a distinct literal,
    /// which nothing exercised before this test.
    #[test]
    fn every_arm_renders_and_names_the_tools_the_range_reaches() {
        assert_eq!(time_range_notice(None, None), None);

        let both = time_range_notice(Some(at(1)), Some(at(5))).expect("a range was given");
        assert!(
            both.contains("2026-03-13T01:00:00Z .. 2026-03-13T05:00:00Z"),
            "got {both}"
        );

        let open_end = time_range_notice(Some(at(1)), None).expect("a range was given");
        assert!(
            open_end.contains("2026-03-13T01:00:00Z .. (open end)"),
            "got {open_end}"
        );

        let open_start = time_range_notice(None, Some(at(5))).expect("a range was given");
        assert!(
            open_start.contains("(open start) .. 2026-03-13T05:00:00Z"),
            "got {open_start}"
        );

        // The honesty half of the statement is what makes the notice worth
        // carrying, so it is asserted on every arm that produces one.
        for notice in [&both, &open_end, &open_start] {
            assert!(
                notice.contains("event logs only (EvtxTriage, Hayabusa, Takajo)")
                    && notice.contains("All other tools emit their full output."),
                "got {notice}"
            );
        }
    }
}
