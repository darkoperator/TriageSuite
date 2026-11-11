//! The query templates in the repo's `docs/DuckDB.md` (sections 2 to 4),
//! run through `query_sql` against a case. A case presents each dataset under
//! the view name the run's own `duckdb/views.sql` gives it, so a query
//! written for those views runs here unchanged except where the fixture
//! lacks a column or a dataset the original names.

mod common;

use common::loaded;
use oxl_engine::{query_sql, SqlResult};
use std::path::Path;

fn run(path: &Path, sql: &str) -> SqlResult {
    query_sql(path, sql, 100, 0).unwrap_or_else(|e| panic!("{sql}\n{e}"))
}

#[test]
fn the_documented_dataset_view_selects_run_against_a_case() {
    let (_d, case) = loaded();
    let path = case.path().to_path_buf();
    drop(case);

    // §2, channel coverage and window: verbatim.
    let coverage = run(
        &path,
        "SELECT Channel, count(*) AS events, \
                min(TimeCreated) AS first_event, max(TimeCreated) AS last_event, \
                date_diff('day', min(TimeCreated), max(TimeCreated)) AS days \
         FROM evtxtriage_events \
         GROUP BY Channel ORDER BY events DESC",
    );
    assert_eq!(coverage.rows.len(), 2);

    // §2, blackout gaps: verbatim, a window function through the parse and
    // re-render. The fixture spans minutes, so no gap reaches 24 hours.
    let gaps = run(
        &path,
        "WITH e AS ( \
           SELECT Channel, TimeCreated, \
                  lag(TimeCreated) OVER (PARTITION BY Channel ORDER BY TimeCreated) AS prev \
           FROM evtxtriage_events \
           WHERE Channel IN ('Security','System','Microsoft-Windows-Sysmon/Operational') \
         ) \
         SELECT Channel, prev AS gap_start, TimeCreated AS gap_end, \
                date_diff('hour', prev, TimeCreated) AS gap_hours \
         FROM e \
         WHERE prev IS NOT NULL AND date_diff('hour', prev, TimeCreated) >= 24 \
         ORDER BY gap_hours DESC",
    );
    assert_eq!((gaps.rows.len(), gaps.columns.len()), (0, 4));

    // §2, explicit clearing: verbatim. WS01's two identical 1102 rows.
    let cleared = run(
        &path,
        "SELECT TimeCreated, Channel, EventId, MapDescription, UserName, Computer \
         FROM evtxtriage_events \
         WHERE (Channel = 'Security' AND EventId = 1102) \
            OR (Channel = 'System'   AND EventId IN (104, 1100)) \
         ORDER BY TimeCreated",
    );
    assert_eq!(cleared.rows.len(), 2);

    // §2, did anything stop parsing: verbatim. WS01's "not-a-time" is the one
    // cast failure and SRV01's empty cell the one blank.
    let health = run(
        &path,
        "SELECT count(*) FILTER (WHERE TimeCreated IS NULL AND TimeCreated__text <> '') AS cast_failures, \
                count(*) FILTER (WHERE TimeCreated__text = '') AS blank \
         FROM evtxtriage_events",
    );
    assert_eq!(health.rows[0][0].as_deref(), Some("1"));

    // The Takajo-style process stack over the untyped Hayabusa timeline, with
    // the fixture's columns: two hosts' worth of grouping, two processes.
    let procs = run(
        &path,
        "SELECT _triage_host AS host, count(*) AS \"Count\", \
                trim(regexp_extract(Details, 'Proc: ([^¦]*)', 1)) AS processes \
         FROM ext_hayabusa_csv_timeline \
         WHERE EventID IN ('1','4688') \
         GROUP BY ALL ORDER BY \"Count\" DESC",
    );
    assert_eq!(procs.rows.len(), 2);

    // The raw (all-text) view of a dataset.
    let raw = run(&path, "SELECT * FROM raw_evtxtriage_events LIMIT 5");
    assert_eq!(raw.rows.len(), 5);
    let event_id = raw.columns.iter().position(|c| c == "EventId").unwrap();
    assert_eq!(raw.types[event_id], "VARCHAR");

    // §4, cross-artifact timeline around an anchor, with its EVTX and
    // Prefetch legs (the fixture has no RecycleBin rows or USN journal) and
    // the fixture's anchor and window. Inside 02:14:44 to 02:16:44: the two
    // Sysmon events and SVCH0ST.EXE's prefetch run.
    let around = run(
        &path,
        "WITH anchor AS (SELECT TIMESTAMP '2026-01-05 02:15:44' AS t, INTERVAL 1 MINUTE AS w) \
         SELECT e.ts, e.src, e.what, e.who FROM ( \
           SELECT TimeCreated AS ts, 'EVTX' AS src, \
                  Channel || ' ' || EventId || ' ' || coalesce(MapDescription, '') AS what, \
                  coalesce(UserName, '') AS who                          FROM evtxtriage_events \
           UNION ALL SELECT RunTime,   'Prefetch',   ExecutableName, ''   FROM petriage_timeline \
         ) e, anchor p \
         WHERE e.ts BETWEEN p.t - p.w AND p.t + p.w \
         ORDER BY e.ts",
    );
    let srcs: Vec<_> = around
        .rows
        .iter()
        .map(|r| r[1].as_deref().unwrap_or_default())
        .collect();
    assert_eq!(srcs, ["EVTX", "Prefetch", "EVTX"]);
}
