//! The search box: Timeline Explorer's search syntax, parsed and compiled in
//! the engine. It is untrusted input, like a filter document.

mod common;

use common::{analyst, loaded};
use oxl_engine::search::{
    compile_search, parse_search, Search, SearchExpr, SearchOptions, Term, MAX_SEARCH, MAX_TERMS,
};
use oxl_engine::{columns, count, load_file, page, Case, ColumnInfo, FileOptions, GridQuery};
use oxl_fixtures::{loose_fw_csv, new_case};

const RELATION: &str = "evtxtriage_events";

/// A loaded case and the catalogued columns of the events relation.
fn setup() -> (tempfile::TempDir, Case, Vec<ColumnInfo>) {
    let (d, case) = loaded();
    let cols = columns(&case, RELATION).unwrap();
    (d, case, cols)
}

/// How many events rows `q` selects under `opts`.
fn count_in(case: &Case, cols: &[ColumnInfo], q: &str, opts: SearchOptions) -> i64 {
    let Some(search) = parse_search(q, opts, cols).unwrap_or_else(|e| panic!("{q}: {e}")) else {
        panic!("{q:?} parsed as blank");
    };
    let fragment = compile_search(&search, opts, cols).unwrap_or_else(|e| panic!("{q}: {e}"));
    let sql = format!("select count(*) from {RELATION} where {}", fragment.sql);
    case.scalar_i64_with(&sql, &fragment.params)
        .unwrap_or_else(|e| panic!("{q} -> {sql}: {e}"))
}

fn t(text: &str) -> Term {
    Term {
        text: text.into(),
        column: None,
    }
}

#[test]
fn parses_the_timeline_explorer_forms() {
    let (_d, _case, cols) = setup();
    let e = |s| match parse_search(s, SearchOptions::default(), &cols)
        .unwrap()
        .unwrap()
    {
        Search::Terms(e) => e,
        _ => panic!(),
    };
    assert_eq!(e("one two").any.len(), 2);
    assert_eq!(
        e(r#"mimikatz +"Bad Guy" -mark"#),
        SearchExpr {
            any: vec![t("mimikatz")],
            all: vec![t("Bad Guy")],
            none: vec![t("mark")]
        }
    );
    assert_eq!(
        e("eventid:4624 kali").any,
        vec![
            Term {
                text: "4624".into(),
                column: Some("EventId".into())
            },
            t("kali")
        ]
    );
    assert_eq!(e("http://x").any, vec![t("http://x")]); // "http" is not a column
    assert_eq!(
        e(r#""unterminated quote"#).any,
        vec![t("unterminated quote")]
    );
    assert!(parse_search("   ", SearchOptions::default(), &cols)
        .unwrap()
        .is_none());
    // Quotes may follow the colon.
    assert_eq!(
        e(r#"+PayloadData1:"svch0st.exe:443""#).all,
        vec![Term {
            text: "svch0st.exe:443".into(),
            column: Some("PayloadData1".into())
        }]
    );
}

#[test]
fn a_scope_with_nothing_after_the_colon_is_plain_text() {
    let (_d, _case, cols) = setup();
    for s in ["EventId:", r#"EventId:"""#] {
        assert_eq!(
            parse_search(s, SearchOptions::default(), &cols).unwrap(),
            Some(Search::Terms(SearchExpr {
                any: vec![t("EventId:")],
                ..Default::default()
            })),
            "{s}"
        );
    }
}

#[test]
fn selects_the_documented_rows() {
    let (_d, case, cols) = setup();
    let hits_opts = |q: &str, opts| count_in(&case, &cols, q, opts);
    let hits = |q: &str| hits_opts(q, SearchOptions::default());
    assert_eq!(hits("svch0st"), 2);
    assert_eq!(hits("+svc_backup -Sysmon"), 6);
    assert_eq!(hits(r#""Process creation""#), 3);
    assert_eq!(hits("Channel:Sysmon 5140"), 4);
    assert_eq!(hits("SVCH0ST"), 2);
    assert_eq!(
        hits_opts(
            "SVCH0ST",
            SearchOptions {
                case_sensitive: true,
                ..Default::default()
            }
        ),
        0
    );
    assert_eq!(
        hits_opts(
            r"^(1102|5140)$",
            SearchOptions {
                regex: true,
                ..Default::default()
            }
        ),
        3
    );
}

#[test]
fn wildcards_in_terms_are_literal() {
    let (_d, case, cols) = setup();
    let hits = |q: &str| count_in(&case, &cols, q, SearchOptions::default());
    assert_eq!(hits("svc%backup"), 0);
    assert_eq!(hits("svc_backup"), 9);
    assert_eq!(hits("s_c"), 0);
}

#[test]
fn run_stamp_and_output_path_are_not_searched() {
    let (_d, case, cols) = setup();
    let hits = |q: &str| count_in(&case, &cols, q, SearchOptions::default());
    assert_eq!(hits("20260101000000"), 0);
    assert_eq!(hits("evtx.csv"), 0);
    // WS01: 8 by host + SRV01's two RemoteHost = WS01 rows
    assert_eq!(hits("WS01"), 10);
}

#[test]
fn parse_search_is_total() {
    let (_d, _case, cols) = setup();
    for s in [
        "+",
        "-",
        ":",
        "\"",
        "+\"",
        "a:",
        ":b",
        "++--",
        &"\"".repeat(1999),
        "\u{0}\u{1f}",
        "+\u{202e}evil",
    ] {
        let _ = parse_search(s, SearchOptions::default(), &cols);
    }
    assert_eq!(
        parse_search(&"x ".repeat(2000), SearchOptions::default(), &cols)
            .unwrap_err()
            .code(),
        "invalid"
    );
    // bad regex is refused at parse
    assert_eq!(
        parse_search(
            "(",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
            &cols
        )
        .unwrap_err()
        .code(),
        "invalid"
    );
}

#[test]
fn caps_are_enforced_at_the_boundary() {
    let (_d, case, cols) = setup();
    let opts = SearchOptions::default();
    let at_cap = vec!["svch0st"; MAX_TERMS].join(" ");
    assert_eq!(count_in(&case, &cols, &at_cap, opts), 2);
    let over = vec!["svch0st"; MAX_TERMS + 1].join(" ");
    assert_eq!(
        parse_search(&over, opts, &cols).unwrap_err().code(),
        "invalid"
    );
    let long = "a".repeat(MAX_SEARCH);
    assert_eq!(count_in(&case, &cols, &long, opts), 0);
    let longer = "a".repeat(MAX_SEARCH + 1);
    assert_eq!(
        parse_search(&longer, opts, &cols).unwrap_err().code(),
        "invalid"
    );
}

#[test]
fn hostile_terms_are_values_not_sql() {
    let (_d, case, cols) = setup();
    let hits = |q: &str| count_in(&case, &cols, q, SearchOptions::default());
    assert_eq!(hits(r#""' OR 1=1 --""#), 0);
    assert_eq!(hits(r#"Channel:"x') OR TRUE --""#), 0);
    // Every SourceFile path has a backslash, which is a plain character.
    assert_eq!(hits(r"\"), 11);
    // Channel and Computer are neighbours, so a term holding the separator
    // the cells are joined with must not match across the two.
    assert_eq!(hits("Security\u{1f}WS01"), 0);
    // A scoped term naming a column the relation lacks is refused at compile.
    let stray = Search::Terms(SearchExpr {
        any: vec![Term {
            text: "x".into(),
            column: Some(r#"EventId" OR 1=1 --"#.into()),
        }],
        ..Default::default()
    });
    assert_eq!(
        compile_search(&stray, SearchOptions::default(), &cols)
            .unwrap_err()
            .code(),
        "invalid"
    );
}

/// A loose file's host is metadata in `_triage_host`, a host-role column,
/// and no cell of the file says it: a search for the host still finds its
/// rows, which is how an analyst finds "everything from WS01".
#[test]
fn host_metadata_column_is_searched() {
    let d = tempfile::tempdir().unwrap();
    let mut case = new_case(d.path());
    let path = loose_fw_csv(d.path());
    let opts = FileOptions {
        host: Some("WS01".into()),
        ..Default::default()
    };
    load_file(&mut case, &path, opts, &analyst()).unwrap();
    assert_eq!(
        common::n(
            &case,
            "select count(*) from fw_csv where \"time\" like '%WS01%' or src_ip like '%WS01%' \
             or dst_ip like '%WS01%' or action like '%WS01%' or bytes::varchar like '%WS01%'"
        ),
        0,
        "the file's own cells must not say WS01"
    );
    let cols = columns(&case, "fw_csv").unwrap();
    let hits = |q: &str| {
        let q = GridQuery {
            relation: "fw_csv".into(),
            search: Some(q.into()),
            ..Default::default()
        };
        count(&case, &q).unwrap()
    };
    assert!(cols.iter().any(|c| c.name == "_triage_host"));
    assert_eq!(hits("WS01"), 3);
    assert_eq!(hits("ws01"), 3);
    assert_eq!(hits("-WS01"), 0);
    assert_eq!(hits("SRV01"), 0);
}

/// `Name:text` scopes a term only to a column whose name is one unquoted
/// word with no colon in it. A column named `Remote Host` or `a:b` is still
/// searched by bare terms, but cannot be scoped in the search box (the
/// README says so); the engine reads the attempt as plain text.
#[test]
fn a_column_with_whitespace_or_a_colon_cannot_be_scoped() {
    let d = tempfile::tempdir().unwrap();
    let mut case = new_case(d.path());
    let p = d.path().join("scopes.csv");
    std::fs::write(&p, "Remote Host,a:b,plain\nsrv,vvv,p1\nother,www,p2\n").unwrap();
    load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    let cols = columns(&case, "scopes_csv").unwrap();
    let parsed = |q: &str| match parse_search(q, SearchOptions::default(), &cols)
        .unwrap()
        .unwrap()
    {
        Search::Terms(e) => e.any,
        other => panic!("{other:?}"),
    };
    let hits = |q: &str| {
        let q = GridQuery {
            relation: "scopes_csv".into(),
            search: Some(q.into()),
            ..Default::default()
        };
        count(&case, &q).unwrap()
    };
    // A plain name scopes.
    assert_eq!(
        parsed("plain:p1"),
        vec![Term {
            text: "p1".into(),
            column: Some("plain".into())
        }]
    );
    assert_eq!(hits("plain:p1"), 1);
    // Quoting the name does not make it one: the whole token is text, which
    // no cell holds.
    assert_eq!(parsed(r#""Remote Host":srv"#), vec![t("Remote Host:srv")]);
    assert_eq!(hits(r#""Remote Host":srv"#), 0);
    assert_eq!(parsed("a:b:vvv"), vec![t("a:b:vvv")]);
    assert_eq!(hits("a:b:vvv"), 0);
    // Without the scope the same words find the rows.
    assert_eq!(hits("srv"), 1);
    assert_eq!(hits("vvv"), 1);
}

/// Every input the totality test feeds the parser also runs end to end, as
/// the grid runs it, in both modes: the answer is a page or `invalid`,
/// never an engine fault or a panic.
#[test]
fn hostile_searches_compile_and_run() {
    let (_d, case, _cols) = setup();
    let long_quotes = "\"".repeat(1999);
    let too_many = "x ".repeat(2000);
    for regex in [false, true] {
        for s in [
            "+",
            "-",
            ":",
            "\"",
            "+\"",
            "a:",
            ":b",
            "++--",
            long_quotes.as_str(),
            "\u{0}\u{1f}",
            "+\u{202e}evil",
            too_many.as_str(),
            "(",
            "[a-",
            "*",
            "(?<n>a)",
            "a{1001}",
        ] {
            let q = GridQuery {
                relation: RELATION.into(),
                search: Some(s.into()),
                search_regex: regex,
                ..Default::default()
            };
            match page(&case, &q, 50, 0) {
                Ok(p) => assert!(p.row_ids.len() <= 11),
                Err(e) => assert_eq!(e.code(), "invalid", "regex={regex} {s:.20?}: {e}"),
            }
            match count(&case, &q) {
                Ok(n) => assert!(n <= 11),
                Err(e) => assert_eq!(e.code(), "invalid", "regex={regex} {s:.20?}: {e}"),
            }
        }
    }
    // The inputs that are plain text in term mode really ran: a lone `:` is
    // a term, which every event's `C:\...` source file contains.
    let q = GridQuery {
        relation: RELATION.into(),
        search: Some(":".into()),
        ..Default::default()
    };
    assert_eq!(count(&case, &q).unwrap(), 11);
}
