//! Loading a loose CSV or TSV: its sniffed companions, host assignment, the
//! names it may and may not keep, and the refusals that leave the case alone.

mod common;

use common::{analyst, long_dir, n};
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
fn a_file_whose_path_passes_the_windows_path_limit_loads() {
    let (d, mut case) = fresh();
    let path = loose_fw_csv(&long_dir(d.path()));
    let r = load_file(&mut case, &path, FileOptions::default(), &analyst()).unwrap();
    assert_eq!(
        (r.relations[0].name.as_str(), r.relations[0].rows),
        ("fw_csv", 3)
    );
    assert_eq!(n(&case, "select count(*) from fw_csv"), 3);
    // The source is recorded under its ordinary path, not the `\\?\` form
    // the file is read through.
    let s = sources(&case).unwrap();
    assert!(
        s[0].path.len() > 260 && !s[0].path.starts_with(r"\\?\"),
        "{}",
        s[0].path
    );
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

/// What each hostile header becomes, exactly: names are kept whole and as
/// written, an empty one is named for its position, a reserved or repeated
/// one is renamed and said so.
#[test]
fn hostile_header_names_survive() {
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
    let names: Vec<String> = columns(&case, &r.relations[0].name)
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names[2], long, "the 600-character name is kept whole");
    assert_eq!(names.iter().map(|n| n.chars().count()).max(), Some(600));
    assert_eq!(
        names,
        vec![
            "a\"b".to_string(),
            "line\nbreak".to_string(),
            long.clone(),
            "dup".to_string(),
            "dup_1".to_string(),
            "x_ox_row".to_string(),
            "column6".to_string(),
        ]
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
fn a_one_megabyte_cell_loads_whole() {
    // (Non-UTF-8 bytes: `non_utf8_csv_is_invalid`.)
    let (d, mut case) = fresh();
    let big = d.path().join("big.csv");
    let cell = "y".repeat(1024 * 1024);
    write_csv(&big, &[vec![b"a".as_slice()], vec![cell.as_bytes()]]);
    let r = load_file(&mut case, &big, FileOptions::default(), &analyst()).unwrap();
    assert_eq!(r.relations[0].rows, 1);
    assert_eq!(n(&case, "select length(a) from big_csv"), 1024 * 1024);
}

/// Bytes that are not UTF-8 are the file's fault: `invalid`, naming the file
/// and what it must be, and nothing of the load is left in the case.
#[test]
fn non_utf8_csv_is_invalid() {
    let (d, mut case) = fresh();
    let bad = d.path().join("bad.csv");
    std::fs::write(&bad, b"a,b\n\xff\xfe,2\n").unwrap();
    let (c, m) = code(load_file(
        &mut case,
        &bad,
        FileOptions::default(),
        &analyst(),
    ));
    assert_eq!(c, "invalid", "{m}");
    assert!(m.contains("bad.csv") && m.contains("UTF-8"), "{m}");
    assert_eq!(n(&case, "select count(*) from ox.source"), 0);
    assert_eq!(
        n(
            &case,
            "select count(*) from duckdb_schemas() where schema_name like 'src_%'"
        ),
        0
    );
}

/// A file whose bad row (30,000, an extra field) is far past the start is
/// `invalid`, naming the file and the line, and leaves no source, no schema
/// and no history behind.
#[test]
fn failed_file_load_leaves_nothing() {
    let (d, mut case) = fresh();
    let p = d.path().join("late.csv");
    let mut body = String::from("a,b\n");
    for i in 1..=29_999 {
        body.push_str(&format!("{i},x\n"));
    }
    body.push_str("30000,x,extra\n");
    std::fs::write(&p, body).unwrap();
    let (c, m) = code(load_file(&mut case, &p, FileOptions::default(), &analyst()));
    assert_eq!(c, "invalid", "{m}");
    assert!(m.contains("late.csv") && m.contains("Line: 30001"), "{m}");
    assert_eq!(n(&case, "select count(*) from ox.source"), 0);
    assert_eq!(
        n(
            &case,
            "select count(*) from duckdb_schemas() where schema_name like 'src_%'"
        ),
        0
    );
    assert_eq!(n(&case, "select count(*) from ox.history"), 0);
    assert_eq!(n(&case, "select count(*) from ox.relation"), 0);
    assert_eq!(n(&case, "select count(*) from ox.column_catalog"), 0);
    assert!(relation(&case, "late_csv").is_err());
    // The case still takes a file afterwards.
    let r = load_file(
        &mut case,
        &loose_fw_csv(d.path()),
        FileOptions::default(),
        &analyst(),
    )
    .unwrap();
    assert_eq!(r.relations[0].rows, 3);
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

/// Found by the step 1 check: a small CRLF file failed to load with an
/// `engine` error. The bytes `a,b\r\n1,2\r\n...` load; what failed was the
/// same text written another way (UTF-16, as Windows PowerShell 5.1's `>`
/// writes it, or with each CR doubled, as a text-mode write of `\r\n` on
/// Windows gives), which DuckDB cannot sniff. That is the file's fault, not
/// the engine's: `invalid`, naming the file, and nothing left behind.
#[test]
fn small_crlf_csv_loads_or_is_invalid() {
    let (d, mut case) = fresh();
    for (name, bytes) in [
        (
            "utf16.csv",
            b"\xff\xfea\x00,\x00b\x00\r\x00\n\x001\x00,\x002\x00\r\x00\n\x00".to_vec(),
        ),
        ("crcr.csv", b"a,b\r\r\n1,2\r\r\n3,4\r\r\n".to_vec()),
    ] {
        let p = d.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        let (c, m) = code(load_file(&mut case, &p, FileOptions::default(), &analyst()));
        assert_eq!(c, "invalid", "{name}: {m}");
        assert!(m.contains(name), "{name}: {m}");
        assert_eq!(n(&case, "select count(*) from ox.source"), 0);
    }

    let p = d.path().join("crlf.csv");
    std::fs::write(&p, b"a,b\r\n1,2\r\n3,4\r\n5,6\r\n").unwrap();
    let r = load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    assert_eq!(r.relations[0].rows, 3);
    assert_eq!(
        n(
            &case,
            "select count(*) from crlf_csv where b in ('2', '4', '6')"
        ),
        3
    );
    // No carriage return is left on the last column's text.
    assert_eq!(
        n(
            &case,
            "select count(*) from crlf_csv where contains(b, chr(13))"
        ),
        0
    );
}

/// 2^70 does not fit a BIGINT: a companion would hold NULL for it, and the
/// cell would read as failed though its text is a good number.
#[test]
fn huge_integers_stay_text() {
    let (d, mut case) = fresh();
    let p = d.path().join("huge.csv");
    std::fs::write(
        &p,
        "h,u\n1180591620717411303424,1\n-1180591620717411303424,2\n",
    )
    .unwrap();
    load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    let cols = columns(&case, "huge_csv").unwrap();
    let h = cols.iter().find(|c| c.name == "h").unwrap();
    assert_eq!((h.typed_column.as_deref(), h.sql_type), (None, None));
    assert_eq!(
        n(
            &case,
            "select count(*) from huge_csv where h = '1180591620717411303424'"
        ),
        1
    );
}

/// A copy of the two-host run built under `from`, copied to `to`, its run
/// id `run_id` and its Hayabusa dataset's view renamed `view`.
fn run_with_view(from: &Path, to: &Path, run_id: &str, view: &str) -> std::path::PathBuf {
    let original = oxl_fixtures::two_hosts(from);
    common::copy_tree(&original, to);
    let path = to.join("duckdb").join("datasets.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    json["run_id"] = run_id.into();
    json["generation"] = format!("{run_id}-g1").into();
    let hayabusa = json["datasets"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|ds| ds["view"] == "ext_hayabusa_csv_timeline")
        .unwrap();
    hayabusa["view"] = view.into();
    hayabusa["raw_view"] = format!("raw_{view}").into();
    std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    to.to_path_buf()
}

/// Both loaders fold a relation name's case the same way, `str::to_lowercase`,
/// so whichever loads second, two names that differ only in case collide.
/// A run's view can be any text; a file's relation name is made of
/// `[a-z0-9_]`, so a file meets a non-ASCII name only through a character
/// whose lower case is ASCII, such as the Kelvin sign (U+212A), lower case `k`.
#[test]
fn collision_fold_is_the_same_in_both_loaders() {
    // Run after run: É and é.
    let (d, mut case) = fresh();
    let first = run_with_view(
        &d.path().join("a"),
        &d.path().join("a2"),
        "20260104000000-dddd",
        "Événement",
    );
    oxl_engine::load_run(&mut case, &first, &analyst()).unwrap();
    let second = run_with_view(
        &d.path().join("b"),
        &d.path().join("b2"),
        "20260105000000-eeee",
        "événement",
    );
    let (c, m) = code(oxl_engine::load_run(&mut case, &second, &analyst()));
    assert_eq!(c, "refused", "{m}");

    // A file after a run, under the name asked for: refused, as the run
    // after the file would be.
    let (d, mut case) = fresh();
    let kelvin = run_with_view(
        &d.path().join("a"),
        &d.path().join("a2"),
        "20260104000000-dddd",
        "\u{212A}fw",
    );
    oxl_engine::load_run(&mut case, &kelvin, &analyst()).unwrap();
    let fw = loose_fw_csv(d.path());
    let asked = FileOptions {
        name: Some("kfw".into()),
        ..Default::default()
    };
    let (c, m) = code(load_file(&mut case, &fw, asked, &analyst()));
    assert_eq!(c, "refused", "{m}");

    // Under the name derived from the file's: the next free one.
    let derived = run_with_view(
        &d.path().join("b"),
        &d.path().join("b2"),
        "20260105000000-eeee",
        "\u{212A}fw_csv",
    );
    let other = d.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let mut case = oxl_fixtures::new_case(&other);
    oxl_engine::load_run(&mut case, &derived, &analyst()).unwrap();
    let p = other.join("kfw.csv");
    std::fs::copy(&fw, &p).unwrap();
    let r = load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    assert_eq!(r.relations[0].name, "kfw_csv_2");
}
