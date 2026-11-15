//! The grid's pages as the app shows them: cells rendered in the display
//! zone, colour rules, and the rows the search finds in find mode.

mod common;

use common::{analyst, loaded};
use oxl_engine::{
    default_colour_rules, delete_record, load_file, mark_suspicious, page_view, parse_filter,
    row_ids_at, row_positions, upsert_ioc, Case, Chip, ChipOrigin, ColourRule, EngineError,
    FileOptions, GridPage, GridState, IocSpec, IocType, PageOptions, RecordKind, RuleColour,
    RuleScope, SearchMode, SearchState, SortKey, SqlType, TimeSemantics, MAX_ACTION_ROWS,
    MAX_RULES,
};
use oxl_fixtures::loose_fw_csv;

const EVENTS: &str = "evtxtriage_events";
const NY: &str = "America/New_York";

fn state(relation: &str) -> GridState {
    GridState {
        relation: relation.into(),
        chips: Vec::new(),
        search: SearchState::default(),
        hosts: Vec::new(),
        sort: Vec::new(),
        columns: None,
    }
}

fn opts(zone: &str, rules: Vec<ColourRule>) -> PageOptions {
    PageOptions {
        zone: zone.into(),
        rules,
    }
}

fn rule(id: &str, filter: &str, colour: RuleColour) -> ColourRule {
    ColourRule {
        id: id.into(),
        name: format!("rule {id}"),
        enabled: true,
        filter: parse_filter(filter).unwrap(),
        colour,
        scope: RuleScope::Row,
    }
}

fn col(p: &GridPage, name: &str) -> usize {
    p.columns.iter().position(|c| c.name == name).unwrap()
}

/// The page's row index whose `column` text is `text`.
fn row_with(p: &GridPage, column: &str, text: &str) -> usize {
    let c = col(p, column);
    p.cells
        .iter()
        .position(|r| r[c].text.as_deref() == Some(text))
        .unwrap_or_else(|| panic!("no row with {column} = {text}"))
}

fn whole(case: &Case, st: &GridState, o: &PageOptions) -> GridPage {
    let p = page_view(case, st, 1000, 0, o).unwrap();
    assert!(!p.truncated);
    p
}

#[test]
fn utc_cells_render_in_the_zone() {
    let (_d, case) = loaded();
    let p = whole(&case, &state(EVENTS), &opts(NY, Vec::new()));
    assert_eq!(p.zone, NY);
    let t = col(&p, "TimeCreated");
    let meta = &p.columns[t];
    assert_eq!(meta.sql_type, Some(SqlType::Timestamp));
    assert_eq!(meta.time_semantics, Some(TimeSemantics::Utc));
    // The grid's default order is the primary time, so the first row is
    // WS01's 02:13:58 failed logon; its fraction stays seven digits.
    let first = &p.cells[0][t];
    assert_eq!(first.text.as_deref(), Some("2026-01-05T02:13:58.2040000Z"));
    assert_eq!(
        first.display.as_deref(),
        Some("2026-01-04 21:13:58.2040000")
    );
    assert!(!first.failed);
    // The time that does not convert keeps its text, flagged, with nothing
    // to display; the blank time has neither.
    let bad = &p.cells[row_with(&p, "TimeCreated", "not-a-time")][t];
    assert_eq!((bad.display.as_deref(), bad.failed), (None, true));
    assert!(p
        .cells
        .iter()
        .any(|r| r[t].text.is_none() && r[t].display.is_none() && !r[t].failed));
    // A typed column that is not a time is never rendered.
    let e = col(&p, "EventId");
    assert_eq!(p.columns[e].sql_type, Some(SqlType::BigInt));
    assert!(p.cells.iter().all(|r| r[e].display.is_none()));
    // Untyped text is never rendered either.
    let c = col(&p, "Channel");
    assert!(p.cells.iter().all(|r| r[c].display.is_none()));

    let pf = whole(&case, &state("petriage_timeline"), &opts(NY, Vec::new()));
    let rc = col(&pf, "RunCount");
    assert!(pf.cells.iter().all(|r| r[rc].display.is_none()));
    assert!(pf.cells.iter().all(|r| r[rc].text.is_some()));

    // In UTC the display is the same instant without the `T` and `Z`.
    let utc = whole(&case, &state(EVENTS), &opts("UTC", Vec::new()));
    assert_eq!(
        utc.cells[0][t].display.as_deref(),
        Some("2026-01-05 02:13:58.2040000")
    );
    // The page is the plain page's rows, ids and marks.
    let plain = oxl_engine::page(&case, &state(EVENTS).query().unwrap(), 1000, 0).unwrap();
    assert_eq!(utc.row_ids, plain.row_ids);
    assert_eq!(utc.marks, plain.marks);
    assert_eq!(
        utc.columns
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>(),
        plain.columns
    );
}

#[test]
fn unknown_zone_columns_are_not_converted() {
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let fw = loose_fw_csv(d.path());
    let rel = load_file(&mut case, &fw, FileOptions::default(), &analyst())
        .unwrap()
        .relations[0]
        .name
        .clone();
    let p = whole(&case, &state(&rel), &opts(NY, Vec::new()));
    let t = col(&p, "time");
    assert_eq!(p.columns[t].time_semantics, Some(TimeSemantics::Unknown));
    assert_eq!(p.columns[t].sql_type, Some(SqlType::Timestamp));
    assert!(p.cells.iter().all(|r| r[t].display.is_none()));
    assert!(p.cells.iter().all(|r| r[t].text.is_some() && !r[t].failed));
}

#[test]
fn an_unknown_display_zone_is_invalid() {
    let (_d, case) = loaded();
    for zone in ["Mars/Olympus_Mons", "", "america/new_york", "+05:00"] {
        let r = page_view(&case, &state(EVENTS), 10, 0, &opts(zone, Vec::new()));
        assert!(matches!(r, Err(EngineError::Invalid(_))), "{zone:?}");
    }
}

#[test]
fn first_matching_rule_wins() {
    let (_d, case) = loaded();
    let sysmon = || {
        rule(
            "sysmon",
            r#"{"column":"Channel","op":"eq","value":"Sysmon"}"#,
            RuleColour::Blue,
        )
    };
    let rclone = || {
        rule(
            "rclone",
            r#"{"column":"PayloadData1","op":"contains","value":"rclone"}"#,
            RuleColour::Red,
        )
    };
    let st = state(EVENTS);

    let p = whole(&case, &st, &opts("UTC", vec![sysmon(), rclone()]));
    let rc = row_with(&p, "PayloadData1", "rclone.exe");
    let svch = row_with(&p, "PayloadData1", "svch0st.exe");
    let logon = row_with(&p, "PayloadData1", "LogonType 10");
    assert_eq!(p.row_rule[rc], Some(0));
    assert_eq!(p.row_rule[svch], Some(0));
    assert_eq!(p.row_rule[logon], None);
    assert_eq!(p.row_rule.len(), p.row_ids.len());
    assert_eq!(
        p.rule_columns,
        vec![
            vec!["Channel".to_string()],
            vec!["PayloadData1".to_string()]
        ]
    );
    assert!(p.rule_errors.is_empty());

    // Swapped, the rclone row is the first rule's, which is now Red; the
    // other Sysmon rows are still the Sysmon rule's.
    let p = whole(&case, &st, &opts("UTC", vec![rclone(), sysmon()]));
    assert_eq!(
        p.row_rule[row_with(&p, "PayloadData1", "rclone.exe")],
        Some(0)
    );
    assert_eq!(
        p.row_rule[row_with(&p, "PayloadData1", "svch0st.exe")],
        Some(1)
    );

    // A disabled rule never matches; the next one still does.
    let mut off = sysmon();
    off.enabled = false;
    let p = whole(&case, &st, &opts("UTC", vec![off, rclone()]));
    assert_eq!(
        p.row_rule[row_with(&p, "PayloadData1", "rclone.exe")],
        Some(1)
    );
    assert_eq!(
        p.row_rule[row_with(&p, "PayloadData1", "svch0st.exe")],
        None
    );
    assert!(p.rule_errors.is_empty());

    // A rule naming a column the relation lacks is skipped and says why;
    // the rest apply, with their indexes unchanged.
    let missing = rule(
        "missing",
        r#"{"column":"NoSuchColumn","op":"eq","value":"x"}"#,
        RuleColour::Teal,
    );
    let p = whole(&case, &st, &opts("UTC", vec![missing, rclone()]));
    assert_eq!(p.rule_errors.len(), 1);
    assert_eq!(p.rule_errors[0].0, 0);
    assert!(
        p.rule_errors[0].1.contains("NoSuchColumn"),
        "{:?}",
        p.rule_errors
    );
    assert_eq!(
        p.row_rule[row_with(&p, "PayloadData1", "rclone.exe")],
        Some(1)
    );
    assert!(p.rule_columns[0].is_empty());
}

#[test]
fn rules_are_checked_like_chips() {
    let (_d, case) = loaded();
    let st = state(EVENTS);
    let ok = |id: &str| {
        rule(
            id,
            r#"{"column":"Channel","op":"eq","value":"Sysmon"}"#,
            RuleColour::Grey,
        )
    };
    let invalid = |rules: Vec<ColourRule>| {
        matches!(
            page_view(&case, &st, 10, 0, &opts("UTC", rules)),
            Err(EngineError::Invalid(_))
        )
    };
    assert!(invalid(
        (0..=MAX_RULES).map(|i| ok(&format!("r{i}"))).collect()
    ));
    assert!(!invalid(
        (0..MAX_RULES).map(|i| ok(&format!("r{i}"))).collect()
    ));
    assert!(invalid(vec![ok("same"), ok("same")]));
    assert!(invalid(vec![ok("")]));
    assert!(invalid(vec![ok("a b")]));
    assert!(invalid(vec![ok(&"x".repeat(65))]));
    let mut long = ok("long");
    long.name = "n".repeat(81);
    assert!(invalid(vec![long]));
    let mut fits = ok("fits");
    fits.name = "n".repeat(80);
    assert!(!invalid(vec![fits]));
    // A filter over the depth cap is refused, not compiled.
    let mut deep = parse_filter(r#"{"column":"Channel","op":"eq","value":"Sysmon"}"#).unwrap();
    for _ in 0..oxl_engine::filter::MAX_DEPTH {
        deep = oxl_engine::FilterNode::Not(Box::new(deep));
    }
    let mut too_deep = ok("deep");
    too_deep.filter = deep;
    assert!(invalid(vec![too_deep]));

    // From the app as JSON: unknown fields are refused, the colours and
    // scopes are a fixed set, and the zone defaults to UTC.
    let json = r#"{"rules":[{"id":"a","name":"A","enabled":true,
        "filter":{"column":"Channel","op":"eq","value":"Sysmon"},
        "colour":"teal","scope":"cell"}]}"#;
    let o: PageOptions = serde_json::from_str(json).unwrap();
    assert_eq!(
        (o.zone.as_str(), o.rules[0].colour, o.rules[0].scope),
        ("UTC", RuleColour::Teal, RuleScope::Cell)
    );
    assert!(serde_json::from_str::<PageOptions>(r#"{"zone":"UTC","extra":1}"#).is_err());
    let bad_colour = json.replace("teal", "#ff0000");
    assert!(serde_json::from_str::<PageOptions>(&bad_colour).is_err());
    let extra_rule_field = json.replace(r#""scope":"cell""#, r#""scope":"cell","class":"x""#);
    assert!(serde_json::from_str::<PageOptions>(&extra_rule_field).is_err());
}

#[test]
fn hayabusa_gets_default_rules() {
    let (_d, case) = loaded();
    let rules = default_colour_rules(&case, "ext_hayabusa_csv_timeline").unwrap();
    assert_eq!(
        rules.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        [
            "hayabusa-crit",
            "hayabusa-high",
            "hayabusa-med",
            "hayabusa-low"
        ]
    );
    assert_eq!(
        rules.iter().map(|r| r.colour).collect::<Vec<_>>(),
        [
            RuleColour::Red,
            RuleColour::Orange,
            RuleColour::Amber,
            RuleColour::Green
        ]
    );
    assert!(rules
        .iter()
        .all(|r| r.enabled && r.scope == RuleScope::Row && r.name.chars().count() <= 80));
    assert!(default_colour_rules(&case, EVENTS).unwrap().is_empty());
    assert!(default_colour_rules(&case, "petriage_timeline")
        .unwrap()
        .is_empty());
    assert_eq!(
        default_colour_rules(&case, "nope").unwrap_err().code(),
        "not_found"
    );

    let p = whole(
        &case,
        &state("ext_hayabusa_csv_timeline"),
        &opts("UTC", rules.clone()),
    );
    let level = col(&p, "Level");
    let by_level = |l: &str| -> Vec<Option<u16>> {
        p.cells
            .iter()
            .zip(&p.row_rule)
            .filter(|(r, _)| r[level].text.as_deref() == Some(l))
            .map(|(_, rule)| *rule)
            .collect()
    };
    assert_eq!(by_level("crit"), [Some(0), Some(0)]);
    assert_eq!(by_level("high"), [Some(1)]);
    assert_eq!(by_level("med"), [Some(2)]);
    assert_eq!(by_level("info"), [None]);
    assert!(p.rule_columns.iter().all(|c| c == &["Level".to_string()]));

    // Hayabusa's long spellings, in any case, match the same rules.
    let d = tempfile::tempdir().unwrap();
    let mut other = oxl_fixtures::new_case(d.path());
    let path = d.path().join("hb.csv");
    std::fs::write(
        &path,
        "Timestamp,Level,RuleTitle\n\
         2026-01-05T02:13:58Z,CRITICAL,A\n\
         2026-01-05T02:13:59Z,Medium,B\n\
         2026-01-05T02:14:00Z,low,C\n\
         2026-01-05T02:14:01Z,informational,D\n",
    )
    .unwrap();
    let rel = load_file(&mut other, &path, FileOptions::default(), &analyst())
        .unwrap()
        .relations[0]
        .name
        .clone();
    let rules = default_colour_rules(&other, &rel).unwrap();
    assert_eq!(rules.len(), 4);
    let p = whole(&other, &state(&rel), &opts("UTC", rules));
    assert_eq!(p.row_rule, [Some(0), Some(2), Some(3), None]);
}

#[test]
fn rule_values_are_bound() {
    let (_d, case) = loaded();
    let hostile = [
        rule(
            "q",
            r#"{"column":"PayloadData1","op":"eq","value":"' OR 1=1 --"}"#,
            RuleColour::Red,
        ),
        rule(
            "q2",
            r#"{"column":"Channel","op":"contains","value":"') OR TRUE OR ('"}"#,
            RuleColour::Red,
        ),
    ];
    let p = whole(&case, &state(EVENTS), &opts("UTC", hostile.to_vec()));
    assert_eq!(p.row_ids.len(), 11);
    assert!(p.row_rule.iter().all(Option::is_none), "{:?}", p.row_rule);
    assert!(p.rule_errors.is_empty());
}

#[test]
fn search_hits_in_find_mode() {
    let (_d, case) = loaded();
    let mut st = state(EVENTS);
    st.search = SearchState {
        text: "rclone".into(),
        regex: false,
        case_sensitive: false,
        mode: SearchMode::Find,
    };
    // Find mode keeps every row and marks the ones the search selects,
    // alongside a colour rule's parameters.
    let p = whole(
        &case,
        &st,
        &opts(
            NY,
            vec![rule(
                "sec",
                r#"{"column":"Channel","op":"eq","value":"Security"}"#,
                RuleColour::Grey,
            )],
        ),
    );
    assert_eq!(p.row_ids.len(), 11);
    assert_eq!(p.search_hits.len(), 11);
    let rc = row_with(&p, "PayloadData1", "rclone.exe");
    assert!(p.search_hits[rc]);
    assert_eq!(p.search_hits.iter().filter(|h| **h).count(), 1);
    assert_eq!(
        p.row_rule.iter().filter(|r| **r == Some(0)).count(),
        8,
        "the Security rows"
    );
    // The hits follow the page: one row from the rclone row on.
    let at = p
        .row_ids
        .iter()
        .position(|id| *id == p.row_ids[rc])
        .unwrap();
    let one = page_view(&case, &st, 1, at as u64, &opts("UTC", Vec::new())).unwrap();
    assert_eq!(one.search_hits, [true]);

    // In filter mode the search narrows the rows, and nothing is a hit.
    st.search.mode = SearchMode::Filter;
    let p = whole(&case, &st, &opts("UTC", Vec::new()));
    assert_eq!(p.row_ids.len(), 1);
    assert_eq!(p.search_hits, [false]);
    // A blank search in find mode finds nothing.
    st.search = SearchState {
        mode: SearchMode::Find,
        ..SearchState::default()
    };
    let p = whole(&case, &st, &opts("UTC", Vec::new()));
    assert!(p.search_hits.iter().all(|h| !h));
    assert_eq!(p.search_hits.len(), 11);
}

#[test]
fn an_empty_page_has_empty_lists() {
    let (_d, case) = loaded();
    let p = page_view(
        &case,
        &state(EVENTS),
        10,
        1000,
        &opts(
            NY,
            vec![rule(
                "a",
                r#"{"column":"Channel","op":"eq","value":"x"}"#,
                RuleColour::Red,
            )],
        ),
    )
    .unwrap();
    assert!(p.row_ids.is_empty() && p.cells.is_empty() && p.row_rule.is_empty());
    assert!(p.search_hits.is_empty() && p.marks.is_empty());
    assert_eq!(p.rule_columns.len(), 1);
    assert!(!p.truncated);
}

fn sorted_by_event_id(descending: bool) -> GridState {
    GridState {
        sort: vec![SortKey {
            column: "EventId".into(),
            descending,
        }],
        ..state(EVENTS)
    }
}

fn no_mark_chip() -> Chip {
    Chip {
        id: "unmarked".into(),
        label: "no mark".into(),
        enabled: true,
        origin: ChipOrigin::FilterRow,
        filter: parse_filter(r#"{"column":"_mark","op":"eq","value":"none"}"#).unwrap(),
    }
}

#[test]
fn row_ids_follow_the_sort() {
    let (_d, case) = loaded();
    for descending in [false, true] {
        let st = sorted_by_event_id(descending);
        let shown = whole(&case, &st, &opts("UTC", Vec::new())).row_ids;
        assert!(shown.len() > 6);
        // Two ranges, given out of order, answer in the grid's order.
        let ids = row_ids_at(&case, &st, &[(4, 4), (1, 2)]).unwrap();
        assert_eq!(ids, vec![shown[1], shown[2], shown[4]]);
        // A range running past the last row answers the rows there are.
        let last = shown.len() as u64 - 1;
        let tail = row_ids_at(&case, &st, &[(last, last + 50)]).unwrap();
        assert_eq!(tail, vec![shown[shown.len() - 1]]);
    }
    // Ranges that overlap name each row once.
    let st = state(EVENTS);
    let shown = whole(&case, &st, &opts("UTC", Vec::new())).row_ids;
    let ids = row_ids_at(&case, &st, &[(0, 2), (1, 3)]).unwrap();
    assert_eq!(ids, shown[..4].to_vec());
    // A range whose end is before its start is refused, not reversed.
    assert!(matches!(
        row_ids_at(&case, &st, &[(3, 1)]),
        Err(EngineError::Invalid(_))
    ));
}

#[test]
fn ten_thousand_and_one_rows_are_refused() {
    let (_d, case) = loaded();
    let st = state(EVENTS);
    assert_eq!(MAX_ACTION_ROWS, 10_000);
    // Ten thousand is allowed, however few rows the relation has.
    let all = row_ids_at(&case, &st, &[(0, 9_999)]).unwrap();
    assert_eq!(all, whole(&case, &st, &opts("UTC", Vec::new())).row_ids);
    // One more is refused, and the message names the count; the selection is
    // counted once however its ranges overlap.
    for ranges in [vec![(0, 10_000)], vec![(0, 5_000), (5_001, 10_000)]] {
        match row_ids_at(&case, &st, &ranges) {
            Err(EngineError::Invalid(m)) => {
                assert_eq!(m, "select at most 10,000 rows (you have 10,001)");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
    assert!(row_ids_at(&case, &st, &[(0, 5_000), (0, 9_999)]).is_ok());
    // A range past i64::MAX is refused as an offset is.
    assert!(matches!(
        row_ids_at(&case, &st, &[(u64::MAX, u64::MAX)]),
        Err(EngineError::Invalid(_))
    ));
    // As many ids are the most `row_positions` takes.
    let ids: Vec<i64> = (0..=10_000).collect();
    match row_positions(&case, &st, &ids) {
        Err(EngineError::Invalid(m)) => {
            assert_eq!(m, "select at most 10,000 rows (you have 10,001)");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn positions_of_ids_after_a_change() {
    let (_d, mut case) = loaded();
    let st = state(EVENTS);
    let unmarked = GridState {
        chips: vec![no_mark_chip()],
        ..state(EVENTS)
    };
    let shown = whole(&case, &st, &opts("UTC", Vec::new())).row_ids;
    let ids = vec![shown[2], shown[3], shown[4]];
    assert_eq!(
        row_positions(&case, &unmarked, &ids).unwrap(),
        vec![
            (shown[2], Some(2)),
            (shown[3], Some(3)),
            (shown[4], Some(4))
        ]
    );

    assert!(mark_suspicious(&mut case, &[shown[3]], &analyst()).unwrap()[0].done);

    // The mark moves no row: each keeps its index.
    assert_eq!(
        row_positions(&case, &st, &ids).unwrap(),
        vec![
            (shown[2], Some(2)),
            (shown[3], Some(3)),
            (shown[4], Some(4))
        ]
    );
    // Under a filter that now leaves the marked row out, it has no place and
    // the rows after it close up; the answer keeps the ids' order, an id no
    // row has included.
    let asked = vec![shown[4], shown[3], -1, shown[2]];
    assert_eq!(
        row_positions(&case, &unmarked, &asked).unwrap(),
        vec![
            (shown[4], Some(3)),
            (shown[3], None),
            (-1, None),
            (shown[2], Some(2)),
        ]
    );
    assert!(row_positions(&case, &st, &[]).unwrap().is_empty());
}

fn new_ioc(case: &mut Case, t: IocType, value: &str) -> i64 {
    upsert_ioc(
        case,
        &IocSpec {
            id: None,
            ioc_type: t,
            value: value.into(),
            tool: None,
            attack_ids: Vec::new(),
            note: String::new(),
        },
        &analyst(),
    )
    .unwrap()
    .id
}

/// `(column, text)` of every cell of `p` flagged as an IOC, sorted.
fn ioc_cells(p: &GridPage) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for row in &p.cells {
        for (i, cell) in row.iter().enumerate() {
            if cell.ioc {
                out.push((p.columns[i].name.clone(), cell.text.clone().unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// A cell is flagged when its whole text is a live IOC's value, ASCII case
/// ignored: `svch0st.exe` for the IOC `SVCH0ST.EXE`, but not
/// `svch0st.exe:443`, nor Hayabusa's `SrcIP: 203.0.113.7`, which only
/// contain a value; not `wevtutil.exe`, whose IOC was deleted; and not
/// `svc_backup` for the IOC `svc_bac\u{212A}up`, whose Kelvin sign
/// Unicode lowering takes for `k` but ASCII case does not.
#[test]
fn page_flags_ioc_cells() {
    let (_d, mut case) = loaded();
    new_ioc(&mut case, IocType::Ip, "203.0.113.7");
    new_ioc(&mut case, IocType::FileName, "SVCH0ST.EXE");
    let gone = new_ioc(&mut case, IocType::FileName, "wevtutil.exe");
    delete_record(&mut case, RecordKind::Ioc, gone, &analyst()).unwrap();
    new_ioc(&mut case, IocType::Other, "svc_bac\u{212A}up");

    let p = whole(&case, &state(EVENTS), &opts("UTC", Vec::new()));
    let ip = ("RemoteHost".to_string(), "203.0.113.7".to_string());
    assert_eq!(
        ioc_cells(&p),
        [
            ("PayloadData1".to_string(), "svch0st.exe".to_string()),
            ip.clone(),
            ip.clone(),
            ip,
        ]
    );
    let r = row_with(&p, "PayloadData1", "svch0st.exe:443");
    assert!(!p.cells[r][col(&p, "PayloadData1")].ioc);
    assert!(p.cells[r][col(&p, "RemoteHost")].ioc);

    let hayabusa = whole(
        &case,
        &state("ext_hayabusa_csv_timeline"),
        &opts("UTC", Vec::new()),
    );
    let details = col(&hayabusa, "Details");
    assert!(hayabusa.cells.iter().any(|r| r[details]
        .text
        .as_deref()
        .is_some_and(|t| t.contains("203.0.113.7"))));
    assert_eq!(ioc_cells(&hayabusa), []);
}
