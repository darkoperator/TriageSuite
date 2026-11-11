//! Loading a loose CSV or TSV: its sniffed companions, host assignment, the
//! names it may and may not keep, and the refusals that leave the case alone.

mod common;

use common::{analyst, n};
use oxl_engine::{
    columns, count, load_file, page, parse_filter, relation, relation_name_for_file, sources, Case,
    FileOptions, GridQuery, SqlType, TimeSemantics, TypeOrigin,
};
use oxl_fixtures::{loose_fw_csv, new_case};
use std::path::Path;

fn code<T>(result: oxl_engine::Result<T>) -> (&'static str, String) {
    match result {
        Ok(_) => ("ok", String::new()),
        Err(e) => (e.code(), e.to_string()),
    }
}

fn fresh() -> (tempfile::TempDir, Case) {
    let d = tempfile::tempdir().unwrap();
    let case = new_case(d.path());
    (d, case)
}

fn write_csv(path: &Path, rows: &[Vec<&[u8]>]) {
    let mut w = csv::Writer::from_path(path).unwrap();
    for row in rows {
        w.write_record(row).unwrap();
    }
    w.flush().unwrap();
}

#[test]
fn a_plain_csv_loads_with_sniffed_companions() {
    let (d, mut case) = fresh();
    let r = load_file(
        &mut case,
        &loose_fw_csv(d.path()),
        FileOptions::default(),
        &analyst(),
    )
    .unwrap();
    assert_eq!(
        (r.relations[0].name.as_str(), r.relations[0].rows),
        ("fw_csv", 3)
    );
    let cols = columns(&case, "fw_csv").unwrap();
    let time = cols.iter().find(|c| c.name == "time").unwrap();
    assert_eq!(
        (
            time.typed_column.as_deref(),
            time.time_semantics,
            time.origin
        ),
        (
            Some("time__typed"),
            Some(TimeSemantics::Unknown),
            Some(TypeOrigin::Sniffed)
        )
    );
    assert_eq!(
        cols.iter().find(|c| c.name == "bytes").unwrap().sql_type,
        Some(SqlType::BigInt)
    );
    assert_eq!(
        cols.iter()
            .find(|c| c.name == "src_ip")
            .unwrap()
            .typed_column,
        None
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from fw_csv where typeof(\"time\") = 'VARCHAR'"
        ),
        3
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from fw_csv where \"time__typed\" is not null"
        ),
        3
    );
    assert_eq!(
        relation(&case, "fw_csv").unwrap().primary_time.as_deref(),
        Some("time")
    );
    let src = sources(&case).unwrap();
    assert_eq!((src[0].kind.as_str(), src[0].hosts.len()), ("file", 0));
    assert_eq!(n(&case, "select count(*) from raw_fw_csv"), 3);
}

#[test]
fn a_sniffed_timestamp_is_utc_only_when_every_value_ends_in_z() {
    let (d, mut case) = fresh();
    let p = d.path().join("z.csv");
    std::fs::write(
        &p,
        "t,u,m\n2026-01-05T02:14:00Z,2026-01-05T02:14:00,2026-01-05T02:14:00Z\n\
         2026-01-05T02:15:00Z,2026-01-05T02:15:00,2026-01-05T02:15:00\n,,\n",
    )
    .unwrap();
    load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    let cols = columns(&case, "z_csv").unwrap();
    let sem = |name: &str| cols.iter().find(|c| c.name == name).unwrap().time_semantics;
    assert_eq!(sem("t"), Some(TimeSemantics::Utc));
    assert_eq!(sem("u"), Some(TimeSemantics::Unknown));
    // Zoned text that is not all UTC stays text.
    assert_eq!(
        cols.iter().find(|c| c.name == "m").unwrap().typed_column,
        None
    );
}

#[test]
fn a_zoned_timestamp_column_stays_text_as_written() {
    let (d, mut case) = fresh();
    let p = d.path().join("zoned.csv");
    std::fs::write(
        &p,
        "t,n\n2026-01-05 02:14:00+02:00,1\n2026-01-05 02:15:00+02:00,2\n",
    )
    .unwrap();
    load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    let cols = columns(&case, "zoned_csv").unwrap();
    let t = cols.iter().find(|c| c.name == "t").unwrap();
    assert_eq!((t.typed_column.as_deref(), t.origin), (None, None));
    assert_eq!(
        n(
            &case,
            "select count(*) from zoned_csv where t = '2026-01-05 02:14:00+02:00'"
        ),
        1
    );
    assert!(cols
        .iter()
        .any(|c| c.name == "n" && c.typed_column.is_some()));
}

#[test]
fn a_host_can_be_assigned() {
    let (d, mut case) = fresh();
    let r = load_file(
        &mut case,
        &loose_fw_csv(d.path()),
        FileOptions {
            host: Some("WS01".into()),
            ..FileOptions::default()
        },
        &analyst(),
    )
    .unwrap();
    assert_eq!(r.relations[0].hosts, vec!["WS01".to_string()]);
    let rel = relation(&case, "fw_csv").unwrap();
    assert_eq!(rel.host_column.as_deref(), Some("_triage_host"));
    assert_eq!(rel.hosts, vec!["WS01".to_string()]);
    assert_eq!(rel.rows, 3);
    assert_eq!(
        n(
            &case,
            "select count(*) from fw_csv where _triage_host = 'WS01'"
        ),
        3
    );
    assert_eq!(sources(&case).unwrap()[0].hosts, vec!["WS01".to_string()]);
}

#[test]
fn hostile_headers_load_page_and_filter() {
    let (d, mut case) = fresh();
    let p = d.path().join("hostile.csv");
    let long = "x".repeat(600);
    write_csv(
        &p,
        &[
            vec![
                b"a\"b".as_slice(),
                b"line\nbreak",
                long.as_bytes(),
                b"dup",
                b"dup",
                b"_ox_row",
                b"",
            ],
            vec![b"1", b"2", b"3", b"4", b"5", b"6", b"7"],
        ],
    );
    let r = load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    let name = r.relations[0].name.clone();
    let cols = columns(&case, &name).unwrap();
    assert_eq!(cols.len(), 7);
    let mut lower: Vec<String> = cols.iter().map(|c| c.name.to_lowercase()).collect();
    lower.sort();
    lower.dedup();
    assert_eq!(lower.len(), 7);
    assert!(cols.iter().all(|c| !c.name.starts_with("_ox_")));
    assert!(cols.iter().any(|c| c.name == "x_ox_row"));
    assert!(cols.iter().any(|c| c.name == "a\"b"));
    assert!(cols.iter().any(|c| c.name == "line\nbreak"));
    assert!(r.warnings.iter().any(|w| w.contains("renamed")));
    assert_eq!(n(&case, &format!("select count(*) from {name}")), 1);
    assert_eq!(
        n(
            &case,
            &format!("select count(*) from {name} where \"a\"\"b\" = '1' and \"x_ox_row\" = '6'")
        ),
        1
    );
}

#[test]
fn names_are_sanitised_and_unique() {
    assert_eq!(
        relation_name_for_file("fw export (1).CSV", &[]),
        "fw_export__1__csv"
    );
    assert_eq!(relation_name_for_file("2026.csv", &[]), "_2026_csv");
    assert_eq!(
        relation_name_for_file("fw.csv", &["fw_csv".into()]),
        "fw_csv_2"
    );
    assert_eq!(
        relation_name_for_file("fw.csv", &["fw_csv".into(), "fw_csv_2".into()]),
        "fw_csv_3"
    );
    assert_eq!(relation_name_for_file("", &[]), "file");
}

#[test]
fn relation_name_is_total() {
    for s in ["\u{0}", "..", "CON", &"é".repeat(5000), "a/b\\c"] {
        let n = relation_name_for_file(s, &[]);
        assert!(
            !n.is_empty()
                && n.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        );
    }
}

#[test]
fn a_second_file_with_the_same_name_gets_a_suffix() {
    let (d, mut case) = fresh();
    let one = {
        std::fs::create_dir_all(d.path().join("one")).unwrap();
        loose_fw_csv(&d.path().join("one"))
    };
    let two = {
        std::fs::create_dir_all(d.path().join("two")).unwrap();
        loose_fw_csv(&d.path().join("two"))
    };
    let a = load_file(&mut case, &one, FileOptions::default(), &analyst()).unwrap();
    let b = load_file(&mut case, &two, FileOptions::default(), &analyst()).unwrap();
    assert_eq!(a.relations[0].name, "fw_csv");
    assert_eq!(b.relations[0].name, "fw_csv_2");
}

#[test]
fn non_utf8_bytes_and_a_one_megabyte_cell_do_not_panic() {
    let (d, mut case) = fresh();
    let bad = d.path().join("bad.csv");
    std::fs::write(&bad, b"a,b\n\xff\xfe,2\n").unwrap();
    let outcome = load_file(&mut case, &bad, FileOptions::default(), &analyst());
    assert!(matches!(
        outcome.as_ref().map_err(|e| e.code()),
        Ok(_) | Err(_)
    ));
    if outcome.is_err() {
        // A failed load leaves nothing behind.
        assert_eq!(n(&case, "select count(*) from ox.source"), 0);
    }

    let big = d.path().join("big.csv");
    let cell = "y".repeat(1024 * 1024);
    write_csv(&big, &[vec![b"a".as_slice()], vec![cell.as_bytes()]]);
    let r = load_file(&mut case, &big, FileOptions::default(), &analyst()).unwrap();
    assert_eq!(r.relations[0].rows, 1);
}

#[test]
fn an_unsupported_extension_and_a_repeat_load_are_refused() {
    let (d, mut case) = fresh();
    let x = d.path().join("book.xlsx");
    std::fs::write(&x, b"PK").unwrap();
    let (c, m) = code(load_file(&mut case, &x, FileOptions::default(), &analyst()));
    assert_eq!(c, "invalid");
    assert!(m.contains("xlsx"), "{m}");

    let p = loose_fw_csv(d.path());
    load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    let (c, _) = code(load_file(&mut case, &p, FileOptions::default(), &analyst()));
    assert_eq!(c, "refused");
    assert_eq!(n(&case, "select count(*) from ox.source"), 1);
}

#[test]
fn text_shifted_across_a_separator_does_not_collide() {
    // Under the old `name=value` parts joined with chr(31), both rows encode
    // as `a=x␟b=y␟b=z`.
    let (d, mut case) = fresh();
    let p = d.path().join("shift.csv");
    write_csv(
        &p,
        &[
            vec![b"a".as_slice(), b"b"],
            vec![b"x\x1fb=y", b"z"],
            vec![b"x", b"y\x1fb=z"],
        ],
    );
    load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    assert_eq!(n(&case, "select count(*) from shift_csv"), 2);
    assert_eq!(
        n(&case, "select count(distinct _ox_hash) from shift_csv"),
        2
    );
    assert_eq!(n(&case, "select max(_ox_occ) from shift_csv"), 1);
}

#[test]
fn a_null_cell_and_the_old_null_marker_hash_differently() {
    // An empty cell reads as NULL; the old encoding wrote chr(0) for NULL, so
    // a cell holding a NUL character collided with it.
    let (d, mut case) = fresh();
    let p = d.path().join("nul.csv");
    std::fs::write(&p, b"a,b\n,1\n\x00,1\n").unwrap();
    load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    assert_eq!(n(&case, "select count(*) from nul_csv where a is null"), 1);
    assert_eq!(n(&case, "select count(*) from nul_csv where a = chr(0)"), 1);
    assert_eq!(n(&case, "select count(distinct _ox_hash) from nul_csv"), 2);
}

#[test]
fn a_header_only_file_loads_with_zero_rows() {
    let (d, mut case) = fresh();
    let p = d.path().join("empty.csv");
    std::fs::write(&p, "a,b\n").unwrap();
    let r = load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    assert_eq!(r.relations[0].rows, 0);
    assert_eq!(relation(&case, "empty_csv").unwrap().rows, 0);
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.source_table where first_row is null and last_row is null"
        ),
        1
    );
}

#[test]
fn a_tsv_loads_with_its_tabs() {
    let (d, mut case) = fresh();
    let p = d.path().join("x.tsv");
    std::fs::write(&p, "a\tb\n1\thello, world\n").unwrap();
    load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    assert_eq!(
        n(&case, "select count(*) from x_tsv where b = 'hello, world'"),
        1
    );
}

#[test]
fn hostile_headers_page_and_filter() {
    let (d, mut case) = fresh();
    let p = d.path().join("hostile.csv");
    let long = "x".repeat(600);
    write_csv(
        &p,
        &[
            vec![
                b"a\"b".as_slice(),
                b"line\nbreak",
                long.as_bytes(),
                b"dup",
                b"dup",
                b"_ox_row",
                b"",
            ],
            vec![b"1", b"2", b"3", b"4", b"5", b"6", b"7"],
        ],
    );
    let r = load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    let all = GridQuery {
        relation: r.relations[0].name.clone(),
        ..Default::default()
    };
    let p = page(&case, &all, 50, 0).unwrap();
    assert_eq!((p.row_ids.len(), p.columns.len()), (1, 7));
    assert_eq!(p.cells[0].len(), 7);
    let texts: Vec<_> = p.cells[0].iter().map(|c| c.text.clone().unwrap()).collect();
    assert_eq!(texts, ["1", "2", "3", "4", "5", "6", "7"]);
    let q = GridQuery {
        filter: Some(parse_filter(r#"{"column":"a\"b","op":"eq","value":"1"}"#).unwrap()),
        ..all
    };
    assert_eq!(count(&case, &q).unwrap(), 1);
}
