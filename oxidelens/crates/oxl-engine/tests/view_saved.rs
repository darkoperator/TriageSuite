//! Saved views and filter presets: kept in the case, listed by the
//! relations their pattern matches, applied to a dataset with warnings for
//! the columns it lacks, and exported and imported as JSON files.

mod common;

use common::{agent, analyst, loaded, n};
use oxl_engine::{
    apply_view, count_view, delete_view, export_view, history, import_view, load_run, parse_filter,
    save_view, views, views_matching, Actor, Case, Chip, ChipOrigin, ColourRule, ColumnLayout,
    FilterNode, RuleColour, RuleScope, SearchMode, SearchState, SortKey, ViewDoc, ViewKind,
    MAX_CHIPS,
};
use std::collections::BTreeMap;

const EVENTS: &str = "evtxtriage_events";
const SCHEMA_V1: &str = include_str!("fixtures/schema_v1.sql");

fn chip(id: &str, filter: &str) -> Chip {
    Chip {
        id: id.into(),
        label: format!("chip {id}"),
        enabled: true,
        origin: ChipOrigin::FilterRow,
        filter: parse_filter(filter).unwrap(),
    }
}

fn channel_chip() -> Chip {
    chip("c1", r#"{"column":"Channel","op":"eq","value":"Security"}"#)
}

fn chips_doc(chips: Vec<Chip>) -> ViewDoc {
    ViewDoc {
        chips,
        search: SearchState::default(),
        hosts: Vec::new(),
        group_by: Vec::new(),
        sort: Vec::new(),
        layout: None,
        colour_rules: Vec::new(),
    }
}

fn sort(column: &str) -> SortKey {
    SortKey {
        column: column.into(),
        descending: true,
    }
}

fn rule(id: &str, filter: &str) -> ColourRule {
    ColourRule {
        id: id.into(),
        name: format!("rule {id}"),
        enabled: true,
        filter: parse_filter(filter).unwrap(),
        colour: RuleColour::Red,
        scope: RuleScope::Row,
    }
}

/// A whole view: a chip, a search, a host, a sort, a group, a layout and a
/// rule, every one of them on a column `evtxtriage_events` has.
fn full_doc() -> ViewDoc {
    let mut widths = BTreeMap::new();
    widths.insert("Channel".to_string(), 120);
    ViewDoc {
        chips: vec![channel_chip()],
        search: SearchState {
            text: "svc_backup".into(),
            regex: false,
            case_sensitive: false,
            mode: SearchMode::Find,
        },
        hosts: vec!["WS01".into()],
        group_by: vec!["EventId".into()],
        sort: vec![sort("TimeCreated")],
        layout: Some(ColumnLayout {
            order: vec!["TimeCreated".into(), "Channel".into()],
            widths,
            hidden: vec!["SourceFile".into()],
            pinned: vec!["TimeCreated".into()],
        }),
        colour_rules: vec![rule(
            "r1",
            r#"{"column":"EventId","op":"eq","value":"4625"}"#,
        )],
    }
}

fn code<T: std::fmt::Debug>(r: oxl_engine::Result<T>) -> &'static str {
    r.expect_err("expected an error").code()
}

fn view_count(case: &Case) -> i64 {
    n(case, "SELECT count(*) FROM ox.saved_view")
}

#[test]
fn save_list_apply_round_trip() {
    let (_d, mut case) = loaded();
    let saved = save_view(
        &mut case,
        "Security on WS01",
        ViewKind::View,
        "evtxtriage_*",
        &full_doc(),
        &analyst(),
    )
    .unwrap();
    assert!(saved.id > 0);
    assert_eq!(saved.name, "Security on WS01");
    assert_eq!(saved.kind, ViewKind::View);
    assert_eq!(saved.created_by, "analyst:tester");
    assert_eq!(saved.updated_by, "analyst:tester");
    assert_eq!(saved.created_at, saved.updated_at);
    assert!(saved.created_at.ends_with('Z'), "{}", saved.created_at);

    let listed = views(&case, None, 50, 0).unwrap();
    assert_eq!(listed.items.len(), 1);
    assert!(!listed.truncated);
    assert_eq!(listed.items[0], saved);
    assert_eq!(listed.items[0].doc, full_doc());

    let applied = apply_view(&case, saved.id, EVENTS).unwrap();
    assert!(applied.warnings.is_empty(), "{:?}", applied.warnings);
    assert_eq!(applied.state.relation, EVENTS);
    assert_eq!(applied.state.chips.len(), 1);
    assert!(applied.state.chips[0].enabled);
    assert_eq!(applied.state.search, full_doc().search);
    assert_eq!(applied.state.hosts, ["WS01"]);
    assert_eq!(applied.state.sort.len(), 1);
    assert_eq!(applied.state.sort[0].column, "TimeCreated");
    assert_eq!(applied.group_by, ["EventId"]);
    assert_eq!(applied.layout, full_doc().layout);
    assert_eq!(applied.colour_rules, full_doc().colour_rules);
    // WS01's Security rows; find mode leaves the search out of the count.
    assert_eq!(count_view(&case, &applied.state).unwrap().rows, 5);

    // The save is in the history.
    let h = history(&case, Some("saved_view"), 10, 0).unwrap();
    assert_eq!(h.items.len(), 1);
    assert_eq!(h.items[0].action, "view_save");
    assert_eq!(h.items[0].target_id, saved.id.to_string());

    // Paging and sort: views before presets, then by name.
    save_view(
        &mut case,
        "A preset",
        ViewKind::Preset,
        "*",
        &chips_doc(vec![channel_chip()]),
        &analyst(),
    )
    .unwrap();
    save_view(
        &mut case,
        "Another view",
        ViewKind::View,
        "*",
        &chips_doc(Vec::new()),
        &analyst(),
    )
    .unwrap();
    let names: Vec<String> = views(&case, None, 50, 0)
        .unwrap()
        .items
        .into_iter()
        .map(|v| v.name)
        .collect();
    assert_eq!(names, ["Another view", "Security on WS01", "A preset"]);
    let first = views(&case, None, 2, 0).unwrap();
    assert!(first.truncated);
    assert_eq!(first.items.len(), 2);
    let rest = views(&case, None, 2, 2).unwrap();
    assert!(!rest.truncated);
    assert_eq!(rest.items[0].name, "A preset");
    assert_eq!(code(views(&case, None, 0, 0)), "invalid");
}

#[test]
fn same_name_replaces() {
    let (_d, mut case) = loaded();
    let first = save_view(
        &mut case,
        "triage",
        ViewKind::View,
        "*",
        &chips_doc(Vec::new()),
        &analyst(),
    )
    .unwrap();
    let second = save_view(
        &mut case,
        "triage",
        ViewKind::View,
        "evtxtriage_events",
        &chips_doc(vec![channel_chip()]),
        &other_analyst(),
    )
    .unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(second.created_by, "analyst:tester");
    assert_eq!(second.created_at, first.created_at);
    assert_eq!(second.updated_by, "analyst:other");
    assert_eq!(second.pattern, "evtxtriage_events");
    assert_eq!(second.doc.chips.len(), 1);
    assert_eq!(
        views(&case, None, 50, 0).unwrap().items,
        std::slice::from_ref(&second)
    );

    // The same name as a preset is another thing.
    let preset = save_view(
        &mut case,
        "triage",
        ViewKind::Preset,
        "*",
        &chips_doc(vec![channel_chip()]),
        &analyst(),
    )
    .unwrap();
    assert_ne!(preset.id, first.id);
    assert_eq!(views(&case, None, 50, 0).unwrap().items.len(), 2);

    let h = history(&case, Some("saved_view"), 10, 0).unwrap();
    let actions: Vec<&str> = h.items.iter().map(|r| r.action.as_str()).collect();
    assert_eq!(actions, ["view_save", "view_update", "view_save"]);
    let update = &h.items[1];
    assert_eq!(update.by, "analyst:other");
    assert_eq!(update.before.as_ref().unwrap()["pattern"], "*");
    assert_eq!(
        update.after.as_ref().unwrap()["pattern"],
        "evtxtriage_events"
    );
}

/// A second analyst, for telling saves apart.
fn other_analyst() -> Actor {
    Actor::Analyst("other".into())
}

#[test]
fn a_preset_with_sort_is_invalid() {
    let (_d, mut case) = loaded();
    let mut doc = chips_doc(vec![channel_chip()]);
    doc.sort = vec![sort("TimeCreated")];
    let r = save_view(&mut case, "p", ViewKind::Preset, "*", &doc, &analyst());
    assert_eq!(code(r), "invalid");
    // Each other part is refused too.
    let mut with_search = chips_doc(vec![channel_chip()]);
    with_search.search.text = "x".into();
    let mut with_hosts = chips_doc(vec![channel_chip()]);
    with_hosts.hosts = vec!["WS01".into()];
    let mut with_group = chips_doc(vec![channel_chip()]);
    with_group.group_by = vec!["Channel".into()];
    let mut with_layout = chips_doc(vec![channel_chip()]);
    with_layout.layout = full_doc().layout;
    let mut with_rules = chips_doc(vec![channel_chip()]);
    with_rules.colour_rules = full_doc().colour_rules;
    for doc in [with_search, with_hosts, with_group, with_layout, with_rules] {
        let r = save_view(&mut case, "p", ViewKind::Preset, "*", &doc, &analyst());
        assert_eq!(code(r), "invalid", "{doc:?}");
    }
    // The same document as a view is fine; chips alone are a fine preset.
    let mut doc = chips_doc(vec![channel_chip()]);
    doc.sort = vec![sort("TimeCreated")];
    save_view(&mut case, "p", ViewKind::View, "*", &doc, &analyst()).unwrap();
    let p = chips_doc(vec![channel_chip()]);
    save_view(&mut case, "p", ViewKind::Preset, "*", &p, &analyst()).unwrap();
    assert_eq!(view_count(&case), 2);
}

#[test]
fn names_patterns_and_layouts_are_checked() {
    let (_d, mut case) = loaded();
    let doc = chips_doc(Vec::new());
    for (name, pattern) in [
        ("", "*"),
        (&"n".repeat(129) as &str, "*"),
        ("tab\there", "*"),
        ("ok", ""),
        ("ok", "evtx triage"),
        ("ok", "evtx?"),
        ("ok", &"p".repeat(129)),
    ] {
        let r = save_view(&mut case, name, ViewKind::View, pattern, &doc, &analyst());
        assert_eq!(code(r), "invalid", "{name:?} {pattern:?}");
    }
    // 128 characters, not bytes.
    let long = "é".repeat(128);
    save_view(
        &mut case,
        &long,
        ViewKind::View,
        "a-b_C*9",
        &doc,
        &analyst(),
    )
    .unwrap();

    for width in [39, 2001] {
        let mut doc = full_doc();
        doc.layout
            .as_mut()
            .unwrap()
            .widths
            .insert("Channel".into(), width);
        let r = save_view(&mut case, "w", ViewKind::View, "*", &doc, &analyst());
        assert_eq!(code(r), "invalid", "{width}");
    }
    let mut rules = full_doc();
    rules.colour_rules = (0..51)
        .map(|i| {
            rule(
                &format!("r{i}"),
                r#"{"column":"Channel","op":"eq","value":"x"}"#,
            )
        })
        .collect();
    let r = save_view(&mut case, "r", ViewKind::View, "*", &rules, &analyst());
    assert_eq!(code(r), "invalid");
    assert_eq!(view_count(&case), 1);
}

#[test]
fn pattern_matches_relations() {
    let (_d, mut case) = loaded();
    let doc = chips_doc(Vec::new());
    save_view(
        &mut case,
        "evtx",
        ViewKind::View,
        "evtxtriage_*",
        &doc,
        &analyst(),
    )
    .unwrap();
    save_view(&mut case, "all", ViewKind::View, "*", &doc, &analyst()).unwrap();
    save_view(
        &mut case,
        "exact",
        ViewKind::View,
        "PETriage_Timeline",
        &doc,
        &analyst(),
    )
    .unwrap();
    save_view(
        &mut case,
        "mid",
        ViewKind::View,
        "*triage_t*e",
        &doc,
        &analyst(),
    )
    .unwrap();

    let names = |relation: Option<&str>| -> Vec<String> {
        views(&case, relation, 50, 0)
            .unwrap()
            .items
            .into_iter()
            .map(|v| v.name)
            .collect()
    };
    assert_eq!(names(Some("evtxtriage_events")), ["all", "evtx"]);
    assert_eq!(names(Some("petriage_timeline")), ["all", "exact", "mid"]);
    assert_eq!(names(Some("EVTXTRIAGE_EVENTS")), ["all", "evtx"]);
    // `_` is a literal, not a one-character wildcard.
    assert_eq!(names(Some("evtxtriageXevents")), ["all"]);
    assert_eq!(names(None), ["all", "evtx", "exact", "mid"]);
}

#[test]
fn missing_columns_disable_with_warnings() {
    let (_d, mut case) = loaded();
    let mut doc = chips_doc(vec![
        channel_chip(),
        chip(
            "c2",
            r#"{"or":[{"column":"RuleTitle","op":"contains","value":"Rclone"},{"column":"Channel","op":"eq","value":"Sysmon"}]}"#,
        ),
    ]);
    doc.sort = vec![sort("RuleTitle"), sort("TimeCreated")];
    doc.group_by = vec!["Level".into()];
    doc.colour_rules = vec![
        rule("lvl", r#"{"column":"Level","op":"eq","value":"crit"}"#),
        rule("ok", r#"{"column":"EventId","op":"eq","value":"4625"}"#),
    ];
    let saved = save_view(&mut case, "hayabusa", ViewKind::View, "*", &doc, &analyst()).unwrap();

    // Where every column is there, nothing is disabled.
    let all_there = apply_view(&case, saved.id, "ext_hayabusa_csv_timeline").unwrap();
    let missing: Vec<&str> = all_there.warnings.iter().map(|w| w.name.as_str()).collect();
    // Hayabusa's timeline has no EventId or TimeCreated (it has EventID and
    // Timestamp), so those are the only warnings there.
    assert!(
        missing
            .iter()
            .all(|m| *m == "EventId" || *m == "TimeCreated"),
        "{:?}",
        all_there.warnings
    );
    assert!(all_there.state.chips.iter().all(|c| c.enabled));

    let applied = apply_view(&case, saved.id, EVENTS).unwrap();
    let chips: Vec<(&str, bool)> = applied
        .state
        .chips
        .iter()
        .map(|c| (c.id.as_str(), c.enabled))
        .collect();
    assert_eq!(chips, [("c1", true), ("c2", false)]);
    let sort_columns: Vec<&str> = applied
        .state
        .sort
        .iter()
        .map(|k| k.column.as_str())
        .collect();
    assert_eq!(sort_columns, ["TimeCreated"]);
    assert!(applied.group_by.is_empty());
    // A colour rule naming a missing column stays, switched off (the spec:
    // the affected clause stays, inactive, with a warning).
    let rules: Vec<(&str, bool)> = applied
        .colour_rules
        .iter()
        .map(|r| (r.id.as_str(), r.enabled))
        .collect();
    assert_eq!(rules, [("lvl", false), ("ok", true)]);
    assert_eq!(applied.colour_rules[0].filter, doc.colour_rules[0].filter);

    let warnings: Vec<(&str, &str)> = applied
        .warnings
        .iter()
        .map(|w| (w.part.as_str(), w.name.as_str()))
        .collect();
    assert_eq!(
        warnings,
        [
            ("chip:c2", "RuleTitle"),
            ("sort", "RuleTitle"),
            ("group_by", "Level"),
            ("colour_rule:lvl", "Level"),
        ]
    );
    assert!(applied.warnings.iter().all(|w| !w.message.is_empty()));
    assert!(
        applied.warnings[3].message.contains("switched off"),
        "{:?}",
        applied.warnings[3]
    );
    // The disabled chip is not applied: the Channel chip alone.
    assert_eq!(count_view(&case, &applied.state).unwrap().rows, 8);

    // An unknown relation is not found.
    assert_eq!(
        code(apply_view(&case, saved.id, "no_such_relation")),
        "not_found"
    );
    assert_eq!(code(apply_view(&case, saved.id + 100, EVENTS)), "not_found");
}

#[test]
fn hosts_and_layout_the_dataset_lacks_are_dropped() {
    let (_d, mut case) = loaded();
    let mut doc = full_doc();
    doc.hosts = vec!["ws01".into(), "NOPE01".into()];
    let layout = doc.layout.as_mut().unwrap();
    layout.order.push("Gone1".into());
    layout.hidden.push("Gone2".into());
    layout.pinned.push("Gone1".into());
    layout.widths.insert("Gone3".into(), 100);
    let saved = save_view(&mut case, "v", ViewKind::View, "*", &doc, &analyst()).unwrap();

    let applied = apply_view(&case, saved.id, EVENTS).unwrap();
    assert_eq!(applied.state.hosts, ["ws01"]);
    assert_eq!(applied.layout, full_doc().layout);
    let warnings: Vec<(&str, &str)> = applied
        .warnings
        .iter()
        .map(|w| (w.part.as_str(), w.name.as_str()))
        .collect();
    assert_eq!(
        warnings,
        [("host", "NOPE01"), ("layout", "Gone1, Gone2, Gone3")]
    );
    assert!(
        applied.warnings[1].message.contains('4'),
        "{:?}",
        applied.warnings[1]
    );
    count_view(&case, &applied.state).unwrap();
}

#[test]
fn agent_cannot_replace_or_delete_an_analysts_view() {
    let (_d, mut case) = loaded();
    let doc = chips_doc(vec![channel_chip()]);
    let mine = save_view(&mut case, "mine", ViewKind::View, "*", &doc, &analyst()).unwrap();
    let r = save_view(
        &mut case,
        "mine",
        ViewKind::View,
        "petriage_*",
        &doc,
        &agent(),
    );
    assert_eq!(code(r), "refused");
    assert_eq!(code(delete_view(&mut case, mine.id, &agent())), "refused");
    assert_eq!(
        views(&case, None, 50, 0).unwrap().items,
        std::slice::from_ref(&mine)
    );

    // An agent saves, replaces and deletes its own.
    let its = save_view(&mut case, "its", ViewKind::View, "*", &doc, &agent()).unwrap();
    let again = save_view(
        &mut case,
        "its",
        ViewKind::View,
        "petriage_*",
        &doc,
        &agent(),
    )
    .unwrap();
    assert_eq!(again.id, its.id);
    delete_view(&mut case, its.id, &agent()).unwrap();
    // An analyst replaces an agent's view and then it is the analyst's.
    let theirs = save_view(&mut case, "theirs", ViewKind::View, "*", &doc, &agent()).unwrap();
    save_view(&mut case, "theirs", ViewKind::View, "*", &doc, &analyst()).unwrap();
    assert_eq!(code(delete_view(&mut case, theirs.id, &agent())), "refused");
    delete_view(&mut case, mine.id, &analyst()).unwrap();
}

#[test]
fn delete_is_a_tombstone_with_history() {
    let (_d, mut case) = loaded();
    let doc = chips_doc(vec![channel_chip()]);
    let v = save_view(&mut case, "gone", ViewKind::View, "*", &doc, &analyst()).unwrap();
    delete_view(&mut case, v.id, &analyst()).unwrap();
    assert!(views(&case, None, 50, 0).unwrap().items.is_empty());
    assert_eq!(
        n(
            &case,
            &format!(
                "SELECT count(*) FROM ox.saved_view WHERE id = {} AND deleted",
                v.id
            )
        ),
        1
    );
    let h = history(&case, Some("saved_view"), 10, 0).unwrap();
    assert_eq!(h.items[0].action, "view_delete");
    assert_eq!(h.items[0].target_id, v.id.to_string());
    assert_eq!(h.items[0].before.as_ref().unwrap()["deleted"], false);
    assert_eq!(h.items[0].after.as_ref().unwrap()["deleted"], true);
    // Deleted is gone for every read, and deleting again is refused.
    assert_eq!(code(apply_view(&case, v.id, EVENTS)), "not_found");
    assert_eq!(code(export_view(&case, v.id)), "not_found");
    assert_eq!(code(delete_view(&mut case, v.id, &analyst())), "refused");
    assert_eq!(
        code(delete_view(&mut case, v.id + 100, &analyst())),
        "not_found"
    );
    // Saving the name again makes a new view; the tombstone stays.
    let w = save_view(&mut case, "gone", ViewKind::View, "*", &doc, &analyst()).unwrap();
    assert_ne!(w.id, v.id);
    assert_eq!(view_count(&case), 2);
}

/// A view file around `doc`, which is raw JSON.
fn file_with(name: &str, doc: &str) -> String {
    format!(
        r#"{{"oxidelens_view": 1, "name": {name:?}, "kind": "view", "pattern": "*", "doc": {doc}}}"#
    )
}

#[test]
fn import_refuses_hostile_documents() {
    let (_d, mut case) = loaded();
    let leaf = r#"{"column":"Channel","op":"eq","value":"Security"}"#;
    let chip_json = |id: &str, filter: &str| {
        format!(
            r#"{{"id":"{id}","label":"x","enabled":true,"origin":"filter_row","filter":{filter}}}"#
        )
    };
    let chips_json = |chips: Vec<String>| format!(r#"{{"chips":[{}]}}"#, chips.join(","));

    // A document over 256 KiB compact, though well formed: one chip value
    // of 300 KiB.
    let big = file_with(
        "big",
        &chips_json(vec![chip_json(
            "c1",
            &format!(
                r#"{{"column":"Channel","op":"eq","value":"{}"}}"#,
                "a".repeat(300 * 1024)
            ),
        )]),
    );
    // 51 chips.
    let many = file_with(
        "many",
        &chips_json(
            (0..=MAX_CHIPS)
                .map(|i| chip_json(&format!("c{i}"), leaf))
                .collect(),
        ),
    );
    // A filter nested 9 deep: eight `not`s around a leaf.
    let mut deep = leaf.to_string();
    for _ in 0..8 {
        deep = format!(r#"{{"not":{deep}}}"#);
    }
    let deep = file_with("deep", &chips_json(vec![chip_json("c1", &deep)]));
    let unknown_field = r#"{"oxidelens_view": 1, "name": "u", "kind": "view", "pattern": "*", "doc": {}, "extra": 1}"#.to_string();
    let unknown_doc_field = file_with("u", r#"{"chips":[],"script":"x"}"#);
    let unknown_op = file_with(
        "op",
        &chips_json(vec![chip_json(
            "c1",
            r#"{"column":"Channel","op":"drop_table","value":"x"}"#,
        )]),
    );
    let bel = file_with("bell\u{7}", "{}");
    let wrong_version =
        r#"{"oxidelens_view": 2, "name": "v", "kind": "view", "pattern": "*", "doc": {}}"#
            .to_string();
    let not_json = "{\"oxidelens_view\": 1,".to_string();
    let brackets = "[".repeat(100_000);

    for (what, json) in [
        ("300 KiB", big),
        ("51 chips", many),
        ("depth 9", deep),
        ("unknown field", unknown_field),
        ("unknown doc field", unknown_doc_field),
        ("unknown op", unknown_op),
        ("BEL in the name", bel),
        ("version 2", wrong_version),
        ("truncated", not_json),
        ("brackets", brackets),
    ] {
        let r = import_view(&mut case, &json, &analyst());
        assert_eq!(code(r), "invalid", "{what}");
    }
    assert_eq!(view_count(&case), 0);
    assert_eq!(
        n(
            &case,
            "SELECT count(*) FROM ox.history WHERE target_kind = 'saved_view'"
        ),
        0
    );

    // A chip nested 8 deep is at the cap on its own, and over it once the
    // grid puts its `and` around the enabled chips: switched off it is kept,
    // switched on it is refused, as the grid would refuse it.
    let mut eight = leaf.to_string();
    for _ in 0..7 {
        eight = format!(r#"{{"not":{eight}}}"#);
    }
    let on = chips_json(vec![chip_json("c1", &eight)]);
    let r = import_view(&mut case, &file_with("eight", &on), &analyst());
    assert_eq!(code(r), "invalid");
    let off = on.replace(r#""enabled":true"#, r#""enabled":false"#);
    import_view(&mut case, &file_with("eight", &off), &analyst()).unwrap();
    assert_eq!(view_count(&case), 1);
}

#[test]
fn export_import_round_trip() {
    let (d, mut case) = loaded();
    let saved = save_view(
        &mut case,
        "full <b>view</b> \u{202e}",
        ViewKind::View,
        "evtxtriage_*",
        &full_doc(),
        &analyst(),
    )
    .unwrap();
    let json = export_view(&case, saved.id).unwrap();
    // Pretty, versioned, and nothing but the five keys.
    assert!(json.contains('\n'));
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 5, "{keys:?}");
    assert_eq!(value["oxidelens_view"], 1);
    assert_eq!(value["name"], "full <b>view</b> \u{202e}");
    assert_eq!(value["kind"], "view");
    assert_eq!(value["pattern"], "evtxtriage_*");

    // Into another case.
    let root = d.path().join("other");
    std::fs::create_dir_all(&root).unwrap();
    let run = oxl_fixtures::two_hosts(&root.join("run"));
    let mut other = oxl_fixtures::new_case(&root);
    load_run(&mut other, &run, &analyst()).unwrap();
    let imported = import_view(&mut other, &json, &agent()).unwrap();
    assert_eq!(imported.name, saved.name);
    assert_eq!(imported.kind, saved.kind);
    assert_eq!(imported.pattern, saved.pattern);
    assert_eq!(imported.doc, saved.doc);
    assert_eq!(imported.created_by, "agent:claude");
    assert_eq!(export_view(&other, imported.id).unwrap(), json);

    // Into the same case it replaces the view of the same name and kind.
    let again = import_view(&mut case, &json, &analyst()).unwrap();
    assert_eq!(again.id, saved.id);
    assert_eq!(views(&case, None, 50, 0).unwrap().items.len(), 1);
    // An agent importing over an analyst's view is refused like a save.
    assert_eq!(code(import_view(&mut case, &json, &agent())), "refused");

    // A preset round-trips too.
    let preset = save_view(
        &mut case,
        "pre",
        ViewKind::Preset,
        "*",
        &chips_doc(vec![channel_chip()]),
        &analyst(),
    )
    .unwrap();
    let json = export_view(&case, preset.id).unwrap();
    assert!(json.contains("\"preset\""));
    let back = import_view(&mut other, &json, &analyst()).unwrap();
    assert_eq!(back.kind, ViewKind::Preset);
    assert_eq!(back.doc, preset.doc);
}

/// A view whose stored document no longer decodes (the case was edited by
/// hand, or a later engine wrote a shape this one does not know) is left out
/// of both listings and named in `skipped`, instead of failing them whole.
/// The bad view sorts between the good ones, so a listing that stopped at it
/// would lose the one after it.
#[test]
fn views_listing_skips_an_undecodable_view() {
    let (d, mut case) = loaded();
    let mut save = |name: &str| {
        save_view(
            &mut case,
            name,
            ViewKind::View,
            "*",
            &chips_doc(Vec::new()),
            &analyst(),
        )
        .unwrap()
        .id
    };
    let (a, b, c) = (save("A"), save("B"), save("C"));
    let path = d.path().join("t.case.duckdb");
    drop(case);
    {
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute("UPDATE ox.saved_view SET doc_json = '{' WHERE id = ?", [b])
            .unwrap();
    }
    let case = Case::open(&path).unwrap();

    let listed = views(&case, None, 50, 0).unwrap();
    let ids: Vec<i64> = listed.items.iter().map(|v| v.id).collect();
    assert_eq!(ids, [a, c]);
    assert!(!listed.truncated);
    assert_eq!(listed.skipped.len(), 1, "{:?}", listed.skipped);
    assert_eq!(listed.skipped[0].id, b);
    assert!(
        listed.skipped[0].reason.contains("does not decode"),
        "{}",
        listed.skipped[0].reason
    );

    let matching = views_matching(&case, 50, 0).unwrap();
    let ids: Vec<i64> = matching.items.iter().map(|m| m.view.id).collect();
    assert_eq!(ids, [a, c]);
    let skipped: Vec<i64> = matching.skipped.iter().map(|s| s.id).collect();
    assert_eq!(skipped, [b]);
}

#[test]
fn views_on_format1_read_only_are_empty() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("v1.case.duckdb");
    let case = Case::create_at_format_1(&path, SCHEMA_V1, "IR-1", &analyst()).unwrap();
    drop(case);
    let mut case = Case::open_read_only(&path).unwrap();
    assert_eq!(case.format(), 1);
    let listed = views(&case, None, 50, 0).unwrap();
    assert!(listed.items.is_empty());
    assert!(!listed.truncated);
    assert!(views(&case, Some(EVENTS), 50, 0).unwrap().items.is_empty());
    assert_eq!(code(apply_view(&case, 1, EVENTS)), "not_found");
    assert_eq!(code(export_view(&case, 1)), "not_found");
    let doc = chips_doc(Vec::new());
    let r = save_view(&mut case, "v", ViewKind::View, "*", &doc, &analyst());
    assert_eq!(code(r), "refused");
    assert_eq!(code(delete_view(&mut case, 1, &analyst())), "refused");
    let r = import_view(&mut case, &file_with("v", "{}"), &analyst());
    assert_eq!(code(r), "refused");
}

/// The import file cap, a literal here so the test pins its value.
const FILE_CAP: usize = 1024 * 1024;

/// A one-chip document of `values` (an `in` list on Channel), its last
/// value padded so that the document encodes compact to exactly `target`
/// bytes.
fn doc_of_size(values: usize, target: usize) -> ViewDoc {
    let list: Vec<serde_json::Value> = (0..values)
        .map(|i| serde_json::Value::String(format!("v{i:05}")))
        .collect();
    let leaf = serde_json::json!({"column": "Channel", "op": "in", "value": list});
    let mut doc = chips_doc(vec![chip("c1", &leaf.to_string())]);
    let size = serde_json::to_string(&doc).unwrap().len();
    assert!(size < target, "{size} >= {target}");
    let FilterNode::Leaf(leaf) = &mut doc.chips[0].filter else {
        unreachable!("the chip is one condition")
    };
    let last = leaf.value.as_array_mut().unwrap().last_mut().unwrap();
    let padded = format!("{}{}", last.as_str().unwrap(), "p".repeat(target - size));
    *last = serde_json::Value::String(padded);
    assert_eq!(serde_json::to_string(&doc).unwrap().len(), target);
    doc
}

/// Saved views, and history rows about them.
fn view_rows(case: &Case) -> (i64, i64) {
    (
        view_count(case),
        n(
            case,
            "SELECT count(*) FROM ox.history WHERE target_kind = 'saved_view'",
        ),
    )
}

#[test]
fn a_view_saved_at_the_cap_exports_to_a_file_that_imports() {
    let (d, mut case) = loaded();
    // 9,000 short values, about 90 KiB compact, padded to one byte under
    // 256 KiB.
    let doc = doc_of_size(9_000, 256 * 1024 - 1);
    let saved = save_view(
        &mut case,
        "at the cap",
        ViewKind::View,
        "*",
        &doc,
        &analyst(),
    )
    .unwrap();
    // Two bytes more is over, and save refuses it.
    let over = doc_of_size(9_000, 256 * 1024 + 1);
    let r = save_view(&mut case, "over", ViewKind::View, "*", &over, &analyst());
    assert_eq!(code(r), "invalid");

    let json = export_view(&case, saved.id).unwrap();
    // Pretty, and well past the compact document's 256 KiB.
    assert!(json.contains("\n  "), "not pretty");
    assert!(json.len() > 256 * 1024, "{}", json.len());
    assert!(json.len() <= FILE_CAP, "{}", json.len());

    let root = d.path().join("other");
    std::fs::create_dir_all(&root).unwrap();
    let mut other = oxl_fixtures::new_case(&root);
    let back = import_view(&mut other, &json, &analyst()).unwrap();
    assert_eq!(back.doc, doc);
    assert_eq!(export_view(&other, back.id).unwrap(), json);
}

#[test]
fn a_view_whose_pretty_file_is_over_the_cap_exports_compact() {
    let (d, mut case) = loaded();
    // Seven conditions of 10,000 empty strings: about 210 KiB compact, but
    // past 1 MiB pretty, each value on an indented line of its own.
    let leaves: Vec<serde_json::Value> = (0..7)
        .map(|_| serde_json::json!({"column": "EventId", "op": "in", "value": vec![""; 10_000]}))
        .collect();
    let filter = serde_json::json!({ "and": leaves }).to_string();
    let doc = chips_doc(vec![chip("c1", &filter)]);
    let compact = serde_json::to_string(&doc).unwrap().len();
    assert!(compact < 256 * 1024, "{compact}");
    let saved = save_view(&mut case, "wide", ViewKind::View, "*", &doc, &analyst()).unwrap();

    let json = export_view(&case, saved.id).unwrap();
    assert!(json.len() <= FILE_CAP, "{}", json.len());
    assert!(!json.contains('\n'), "a compact file has no line breaks");
    let root = d.path().join("other");
    std::fs::create_dir_all(&root).unwrap();
    let mut other = oxl_fixtures::new_case(&root);
    let back = import_view(&mut other, &json, &analyst()).unwrap();
    assert_eq!(back.doc, doc);
}

#[test]
fn import_caps_the_file_before_parsing_and_the_document_as_save_does() {
    let (_d, mut case) = loaded();
    let before = view_rows(&case);

    // Over the file cap: refused for its size, unparsed (it is not JSON).
    let huge = "x".repeat(FILE_CAP + 1);
    let e = import_view(&mut case, &huge, &analyst()).unwrap_err();
    assert_eq!(e.code(), "invalid");
    assert!(e.to_string().contains("view file is at most"), "{e}");
    // A well-formed view file one byte over the cap, refused the same way.
    let doc = doc_of_size(9_000, 200 * 1024);
    let mut file = serde_json::json!({
        "oxidelens_view": 1, "name": "big", "kind": "view", "pattern": "*", "doc": doc,
    })
    .to_string();
    file.push_str(&" ".repeat(FILE_CAP + 1 - file.len()));
    let e = import_view(&mut case, &file, &analyst()).unwrap_err();
    assert_eq!(e.code(), "invalid");
    assert!(e.to_string().contains("view file is at most"), "{e}");

    // Under the file cap, its document one byte over 256 KiB compact: the
    // save rule refuses it.
    let over = doc_of_size(9_000, 256 * 1024 + 1);
    let file = serde_json::to_string_pretty(&serde_json::json!({
        "oxidelens_view": 1, "name": "over", "kind": "view", "pattern": "*", "doc": over,
    }))
    .unwrap();
    assert!(file.len() <= FILE_CAP, "{}", file.len());
    let e = import_view(&mut case, &file, &analyst()).unwrap_err();
    assert_eq!(e.code(), "invalid");
    assert!(e.to_string().contains("view document is at most"), "{e}");

    assert_eq!(view_rows(&case), before);
}

/// `"Security"` inside `depth` arrays.
fn nested_value(depth: usize) -> serde_json::Value {
    let mut value = serde_json::Value::String("Security".into());
    for _ in 0..depth {
        value = serde_json::Value::Array(vec![value]);
    }
    value
}

#[test]
fn a_nested_leaf_value_is_refused_at_save_and_at_import() {
    let (_d, mut case) = loaded();
    let before = view_rows(&case);

    // Saved from the app: a switched-off chip whose value nests 200 deep,
    // past serde_json's recursion limit, so that a saved copy could never be
    // read back or exported. Built in code, as serde would not parse it.
    let mut doc = chips_doc(vec![channel_chip()]);
    doc.chips[0].enabled = false;
    let FilterNode::Leaf(leaf) = &mut doc.chips[0].filter else {
        unreachable!("one condition")
    };
    leaf.value = nested_value(200);
    let r = save_view(&mut case, "nested", ViewKind::View, "*", &doc, &analyst());
    assert_eq!(code(r), "invalid");
    // One level is already too many, in a chip and in a colour rule, in a
    // view and in a preset.
    let mut doc = chips_doc(vec![channel_chip()]);
    let FilterNode::Leaf(leaf) = &mut doc.chips[0].filter else {
        unreachable!("one condition")
    };
    leaf.value = nested_value(1);
    let r = save_view(&mut case, "nested", ViewKind::Preset, "*", &doc, &analyst());
    assert_eq!(code(r), "invalid");
    let mut rules = chips_doc(Vec::new());
    rules.colour_rules = vec![rule(
        "r1",
        r#"{"column":"Channel","op":"eq","value":"Security"}"#,
    )];
    let FilterNode::Leaf(leaf) = &mut rules.colour_rules[0].filter else {
        unreachable!("one condition")
    };
    leaf.value = nested_value(1);
    let r = save_view(&mut case, "rule", ViewKind::View, "*", &rules, &analyst());
    assert_eq!(code(r), "invalid");

    // Imported: the same chip as a file, built as a string, 200 deep (past
    // serde's limit) and 100 deep (within it, refused by the shape check).
    for depth in [200, 100, 1] {
        let value = format!("{}\"Security\"{}", "[".repeat(depth), "]".repeat(depth));
        let json = file_with(
            "nested",
            &format!(
                r#"{{"chips":[{{"id":"c1","label":"x","enabled":false,"origin":"filter_row",
                    "filter":{{"column":"Channel","op":"eq","value":{value}}}}}]}}"#
            ),
        );
        let r = import_view(&mut case, &json, &analyst());
        assert_eq!(code(r), "invalid", "depth {depth}");
    }
    let list = file_with(
        "nested",
        r#"{"chips":[{"id":"c1","label":"x","enabled":false,"origin":"filter_row",
            "filter":{"column":"Channel","op":"in","value":["Security",["Sysmon"]]}}]}"#,
    );
    assert_eq!(code(import_view(&mut case, &list, &analyst())), "invalid");

    assert_eq!(view_rows(&case), before);
}

/// A switched-off chip whose one condition is `op` with `value` (raw JSON).
fn disabled_chip_doc(column: &str, op: &str, value: &str) -> ViewDoc {
    let leaf: oxl_engine::Leaf = serde_json::from_str(&format!(
        r#"{{"column":"{column}","op":"{op}","value":{value}}}"#
    ))
    .unwrap();
    let mut chip = channel_chip();
    chip.enabled = false;
    chip.filter = FilterNode::Leaf(leaf);
    chips_doc(vec![chip])
}

#[test]
fn a_disabled_chip_holds_only_a_value_the_compiler_takes() {
    let (_d, mut case) = loaded();
    let before = view_rows(&case);
    for (op, value) in [
        ("between", "[]"),
        ("between", "[4000]"),
        ("between", "[1, 2, 3]"),
        ("in", "[4624]"),
        ("in", r#"["4624", true]"#),
        ("not_in", "[false]"),
        ("eq", "4624"),
        ("contains", "true"),
        ("gt", "[1]"),
        ("is_empty", r#""""#),
    ] {
        let doc = disabled_chip_doc("EventId", op, value);
        let r = save_view(&mut case, "shape", ViewKind::View, "*", &doc, &analyst());
        assert_eq!(code(r), "invalid", "{op} {value}");
        // The same as a file to import.
        let json = file_with("shape", &serde_json::to_string(&doc).unwrap());
        let r = import_view(&mut case, &json, &analyst());
        assert_eq!(code(r), "invalid", "import {op} {value}");
    }
    assert_eq!(view_rows(&case), before);

    // The forms the compiler takes save, and each compiles once switched on.
    for (i, (column, op, value)) in [
        ("EventId", "between", "[4000, 5000]"),
        ("EventId", "in", r#"["4624", "4625"]"#),
        ("EventId", "not_in", "[]"),
        ("EventId", "eq", r#""4624""#),
        ("EventId", "gt", "4000"),
        ("UserName", "is_empty", "null"),
    ]
    .into_iter()
    .enumerate()
    {
        let doc = disabled_chip_doc(column, op, value);
        let saved = save_view(
            &mut case,
            &format!("shape {i}"),
            ViewKind::Preset,
            "*",
            &doc,
            &analyst(),
        )
        .unwrap();
        let mut applied = apply_view(&case, saved.id, EVENTS).unwrap();
        assert!(applied.warnings.is_empty(), "{:?}", applied.warnings);
        applied.state.chips[0].enabled = true;
        count_view(&case, &applied.state).unwrap_or_else(|e| panic!("{op} {value}: {e}"));
    }
}

fn matching_names(case: &Case, limit: u32, offset: u64) -> (Vec<(String, Vec<String>)>, bool) {
    let l = views_matching(case, limit, offset).unwrap();
    (
        l.items
            .into_iter()
            .map(|m| (m.view.name, m.relations))
            .collect(),
        l.truncated,
    )
}

#[test]
fn views_matching_lists_each_view_with_its_relations() {
    let (_d, mut case) = loaded();
    let doc = chips_doc(Vec::new());
    for (name, kind, pattern) in [
        ("evtx", ViewKind::View, "evtxtriage_*"),
        ("all", ViewKind::View, "*"),
        ("Case-folded", ViewKind::View, "PETRIAGE_TIMELINE"),
        ("a preset", ViewKind::Preset, "evtxtriage_events"),
        ("nowhere", ViewKind::View, "nomatch_*"),
    ] {
        save_view(&mut case, name, kind, pattern, &doc, &analyst()).unwrap();
    }
    let every: Vec<String> = oxl_engine::relations(&case)
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect();
    let (got, truncated) = matching_names(&case, 50, 0);
    assert!(!truncated);
    // Views before presets, each by name; a view matching nothing is omitted.
    let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Case-folded", "all", "evtx", "a preset"]);
    let rels = |name: &str| got.iter().find(|(n, _)| n == name).unwrap().1.clone();
    assert_eq!(rels("all"), every);
    assert_eq!(rels("evtx"), ["evtxtriage_events"]);
    assert_eq!(rels("Case-folded"), ["petriage_timeline"]);
    assert_eq!(rels("a preset"), ["evtxtriage_events"]);
    // The same rule as `views` with a relation.
    for relation in &every {
        let listed: Vec<String> = views(&case, Some(relation), 50, 0)
            .unwrap()
            .items
            .into_iter()
            .map(|v| v.name)
            .collect();
        let via_matching: Vec<&str> = got
            .iter()
            .filter(|(_, r)| r.contains(relation))
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(listed, via_matching, "{relation}");
    }
}

#[test]
fn views_matching_pages_after_matching() {
    let (_d, mut case) = loaded();
    let doc = chips_doc(Vec::new());
    // Sorted first, matching nothing: they must not use up the page.
    for i in 0..3 {
        save_view(
            &mut case,
            &format!("a{i}"),
            ViewKind::View,
            "nomatch",
            &doc,
            &analyst(),
        )
        .unwrap();
    }
    for name in ["m1", "m2", "m3"] {
        save_view(&mut case, name, ViewKind::View, "*", &doc, &analyst()).unwrap();
    }
    let (first, truncated) = matching_names(&case, 2, 0);
    assert_eq!(
        first.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["m1", "m2"]
    );
    assert!(truncated);
    let (rest, truncated) = matching_names(&case, 2, 2);
    assert_eq!(
        rest.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["m3"]
    );
    assert!(!truncated);
    assert_eq!(code(views_matching(&case, 0, 0)), "invalid");
}

#[test]
fn views_matching_skips_deleted_views() {
    let (_d, mut case) = loaded();
    let saved = save_view(
        &mut case,
        "gone",
        ViewKind::View,
        "*",
        &chips_doc(Vec::new()),
        &analyst(),
    )
    .unwrap();
    delete_view(&mut case, saved.id, &analyst()).unwrap();
    assert!(matching_names(&case, 10, 0).0.is_empty());
}
