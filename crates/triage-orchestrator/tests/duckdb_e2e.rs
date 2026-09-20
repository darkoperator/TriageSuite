//! The generated SQL is only worth anything if DuckDB actually loads it, so
//! these tests run the real binary and assert on schemas and values, never
//! on a bare row count -- a count of the right size over the wrong columns
//! is exactly the failure that would otherwise slip through.

use std::path::Path;
use triage_core::attribution::Identity;
use triage_core::output::duckdb::build::{
    build, BuildRequest, HostOutputs, MergedFile, ToolOutputs,
};
use triage_core::output::duckdb::render::render_sql;
use triage_core::output::duckdb::types::{ColumnType, DatasetColumnTypes, SqlType, TimeSemantics};
use triage_core::output::published::{DatasetKey, OutputFormat, PublishedFile};

/// Run `sql` then `query`, returning stdout as pipe-separated rows.
fn duckdb_query(binary: &Path, sql: &str, query: &str) -> String {
    let script = format!("{sql}\n{query}\n");
    let output = std::process::Command::new(binary)
        .args(["-noheader", "-list", "-nullvalue", "", "-c", &script])
        .output()
        .expect("run duckdb");
    assert!(
        output.status.success(),
        "duckdb failed:\nstdout: {}\nstderr: {}\nsql:\n{script}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write_csv(root: &Path, rel: &str, body: &str) -> std::path::PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, body).expect("write");
    path
}

fn published(path: std::path::PathBuf, dataset: &'static str, identity: Identity) -> PublishedFile {
    PublishedFile {
        path,
        format: OutputFormat::Csv,
        dataset: DatasetKey::Static(dataset),
        identity,
    }
}

fn sql_for(root: &Path, hosts: &[HostOutputs]) -> String {
    let inv = build(BuildRequest {
        run_id: "RUN1",
        generation: "RUN1-abc",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts,
    });
    render_sql(&inv)
}

const TYPES: &[DatasetColumnTypes] = &[DatasetColumnTypes {
    dataset_id: "ds",
    columns: &[
        ColumnType {
            column: "Big",
            sql_type: SqlType::UBigInt,
            time_semantics: None,
        },
        ColumnType {
            column: "Signed",
            sql_type: SqlType::BigInt,
            time_semantics: None,
        },
        ColumnType {
            column: "When",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        },
    ],
}];

fn typed_host(root: &Path) -> Vec<HostOutputs> {
    // Row 1: u64::MAX, a value that fits UBIGINT but not BIGINT, and a 1601
    // timestamp with seven fractional digits.
    // Row 2: a non-numeric value and a blank, to separate a conversion
    // failure from a missing value.
    let path = write_csv(
        root,
        "H/ds.csv",
        "Big,Signed,When\n\
         18446744073709551615,18446744073709551615,1601-01-01 00:00:00.1234567\n\
         notanumber,,\n",
    );
    vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![published(path, "ds", Identity::System)],
            merged: Vec::new(),
            column_types: TYPES,
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }]
}

/// Criterion 11: the view's schema is what the inventory promised.
#[test]
fn the_typed_view_has_the_declared_schema() {
    let Some(duckdb) = triage_testkit::duckdb_binary() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let sql = sql_for(dir.path(), &typed_host(dir.path()));
    let out = duckdb_query(
        &duckdb,
        &sql,
        "SELECT column_name, column_type FROM (DESCRIBE SELECT * FROM t_ds) \
         WHERE column_name IN ('Big','Signed','When','Big__text') ORDER BY column_name;",
    );
    assert_eq!(
        out, "Big|UBIGINT\nBig__text|VARCHAR\nSigned|BIGINT\nWhen|TIMESTAMP",
        "{sql}"
    );
}

/// Criterion 12: u64::MAX survives UBIGINT and is NULL under BIGINT, with
/// its text preserved either way.
#[test]
fn u64_max_fits_ubigint_and_not_bigint() {
    let Some(duckdb) = triage_testkit::duckdb_binary() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let sql = sql_for(dir.path(), &typed_host(dir.path()));
    let out = duckdb_query(
        &duckdb,
        &sql,
        "SELECT \"Big\", \"Signed\" IS NULL, \"Signed__text\" \
         FROM t_ds WHERE \"Big__text\" = '18446744073709551615';",
    );
    assert_eq!(out, "18446744073709551615|true|18446744073709551615");
}

/// Criterion 13: a conversion failure and a missing value are different rows.
#[test]
fn a_conversion_failure_is_distinguishable_from_a_blank() {
    let Some(duckdb) = triage_testkit::duckdb_binary() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let sql = sql_for(dir.path(), &typed_host(dir.path()));
    let failures = duckdb_query(
        &duckdb,
        &sql,
        "SELECT count(*) FROM t_ds WHERE \"Big\" IS NULL AND \"Big__text\" IS NOT NULL;",
    );
    assert_eq!(failures, "1", "notanumber is a conversion failure");
    let blanks = duckdb_query(
        &duckdb,
        &sql,
        "SELECT count(*) FROM t_ds WHERE \"Signed\" IS NULL AND \"Signed__text\" IS NULL;",
    );
    assert_eq!(blanks, "1", "the empty cell is a missing value");
}

/// Criterion 14: 1601 survives as a TIMESTAMP -- TIMESTAMP_NS would null it
/// -- and the seventh fractional digit is preserved in the text column.
#[test]
fn a_1601_timestamp_converts_and_keeps_its_seventh_digit() {
    let Some(duckdb) = triage_testkit::duckdb_binary() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let sql = sql_for(dir.path(), &typed_host(dir.path()));
    let out = duckdb_query(
        &duckdb,
        &sql,
        "SELECT \"When\", \"When__text\" FROM t_ds WHERE \"When\" IS NOT NULL;",
    );
    assert_eq!(
        out,
        "1601-01-01 00:00:00.123456|1601-01-01 00:00:00.1234567"
    );
}

/// Criterion 15: two hosts with different headers both load, with the
/// missing column NULL rather than an error.
#[test]
fn hosts_with_different_headers_union_by_name() {
    let Some(duckdb) = triage_testkit::duckdb_binary() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let a = write_csv(root, "A/ds.csv", "X,Y\n1,2\n");
    let b = write_csv(root, "B/ds.csv", "X,Z\n3,4\n");
    let hosts = vec![
        HostOutputs {
            host: "A".into(),
            tools: vec![ToolOutputs {
                binary_name: "T".into(),
                published: vec![published(a, "ds", Identity::System)],
                merged: Vec::new(),
                column_types: &[],
                dynamic_column_types: &[],
            }],
            external: Vec::new(),
        },
        HostOutputs {
            host: "B".into(),
            tools: vec![ToolOutputs {
                binary_name: "T".into(),
                published: vec![published(b, "ds", Identity::System)],
                merged: Vec::new(),
                column_types: &[],
                dynamic_column_types: &[],
            }],
            external: Vec::new(),
        },
    ];
    let sql = sql_for(root, &hosts);
    let out = duckdb_query(
        &duckdb,
        &sql,
        "SELECT _triage_host, X, Y, Z FROM t_ds ORDER BY _triage_host;",
    );
    assert_eq!(out, "A|1|2|\nB|3||4");
}

/// Criterion 16: hive_partitioning=false, or a `key=value` directory injects
/// a phantom column even from an explicit file list.
#[test]
fn a_key_value_directory_injects_no_phantom_column() {
    let Some(duckdb) = triage_testkit::duckdb_binary() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let path = write_csv(root, "host=A/ds.csv", "X\n1\n");
    let hosts = vec![HostOutputs {
        host: "host=A".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![published(path, "ds", Identity::System)],
            merged: Vec::new(),
            column_types: &[],
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }];
    let sql = sql_for(root, &hosts);
    let out = duckdb_query(
        &duckdb,
        &sql,
        "SELECT count(*) FROM (DESCRIBE SELECT * FROM t_ds) WHERE column_name = 'host';",
    );
    assert_eq!(out, "0", "{sql}");
}

/// Criterion 17: a merged Velo file and its slices yield the merged rows
/// once, with TriageUser populated and no per-file identity.
#[test]
fn a_merged_dataset_counts_its_rows_once() {
    let Some(duckdb) = triage_testkit::duckdb_binary() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let merged = write_csv(root, "H/ds.csv", "A,TriageUser\n1,jdoe\n2,asmith\n");
    let slice_a = write_csv(root, "H/PerUser/ds_jdoe.csv", "A\n1\n");
    let slice_b = write_csv(root, "H/PerUser/ds_asmith.csv", "A\n2\n");
    let hosts = vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![
                published(slice_a.clone(), "ds", Identity::User("jdoe".into())),
                published(slice_b.clone(), "ds", Identity::User("asmith".into())),
            ],
            merged: vec![MergedFile {
                merged_path: merged,
                source_paths: vec![slice_a, slice_b],
                format: OutputFormat::Csv,
            }],
            column_types: &[],
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }];
    let sql = sql_for(root, &hosts);
    let out = duckdb_query(
        &duckdb,
        &sql,
        "SELECT count(*), count(DISTINCT TriageUser), count(_triage_identity) FROM t_ds;",
    );
    assert_eq!(
        out, "2|2|0",
        "merged rows once, no per-file identity\n{sql}"
    );
}

/// Criterion 18: no inventory path escapes out_root, and a file that
/// genuinely lies outside it is still recorded -- not dropped, not silently
/// stored absolute -- but flagged.
///
/// The original version of this test only ever fed `build()` files strictly
/// under `root`, so `is_relative()` and the no-`..` check could not have
/// failed even if the path-containment logic were entirely absent -- a
/// happy-path smoke test wearing a security guard's name. This version adds
/// a fixture whose published file lives in a wholly separate tempdir, so the
/// path genuinely cannot be made relative to `out_root`, and asserts on what
/// `build()` does with it: the file stays in the inventory, but its absolute
/// path is named in `Inventory::warnings` rather than stored silently.
#[test]
fn every_inventoried_path_stays_under_the_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let inv = build(BuildRequest {
        run_id: "RUN1",
        generation: "RUN1-abc",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &typed_host(root),
    });
    for dataset in &inv.datasets {
        for file in &dataset.files {
            assert!(file.path.is_relative(), "{:?}", file.path);
            assert!(
                !file.path.components().any(|c| c.as_os_str() == ".."),
                "{:?}",
                file.path
            );
        }
    }

    // A published file that genuinely lies outside out_root, from a wholly
    // separate tempdir -- `strip_prefix` cannot succeed on it no matter what
    // out_root is.
    let outside_dir = tempfile::tempdir().expect("outside tempdir");
    let outside_path = write_csv(outside_dir.path(), "elsewhere.csv", "X\n1\n");
    let hosts = vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![published(
                outside_path.clone(),
                "elsewhere",
                Identity::System,
            )],
            merged: Vec::new(),
            column_types: &[],
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }];
    let inv2 = build(BuildRequest {
        run_id: "RUN1",
        generation: "RUN1-abc",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });
    let outside_display = outside_path.display().to_string();
    assert!(
        inv2.warnings.iter().any(|w| w.contains(&outside_display)),
        "expected a warning naming the out-of-root path {outside_display:?}, got {:?}",
        inv2.warnings
    );

    // The ruling was KEEP, not exclude: losing real evidence is worse than
    // any path-hygiene concern, so the file must still be present in the
    // dataset and still marked included_in_view. A regression that silently
    // dropped or excluded out-of-root files would pass the warning assertion
    // above while destroying evidence, so that must be checked separately.
    let dataset = inv2
        .datasets
        .iter()
        .find(|d| d.dataset_id == "elsewhere")
        .unwrap_or_else(|| panic!("expected an 'elsewhere' dataset, got {:?}", inv2.datasets));
    let file = dataset
        .files
        .iter()
        .find(|f| f.path == outside_path)
        .unwrap_or_else(|| {
            panic!(
                "expected the out-of-root file to still be recorded in dataset.files, got {:?}",
                dataset.files
            )
        });
    assert!(
        file.included_in_view,
        "the out-of-root file must still be included in the view, not excluded: {file:?}"
    );
}

/// Criterion 18b: an out-of-root JSON-published file is warned about too.
///
/// `InventoryOnly` records (a JSON publish, or a non-CSV external output)
/// route through the identical `relative()` fallback as `DatasetFile`, but
/// they never enter `dataset.files`, so `out_of_root_warnings` cannot see
/// them -- this exercises the separate code path added to cover that gap.
#[test]
fn an_out_of_root_json_publish_is_warned_about() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let outside_dir = tempfile::tempdir().expect("outside tempdir");
    let outside_json = outside_dir.path().join("report.json");
    std::fs::write(&outside_json, "{}\n").expect("write");
    let hosts = vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![PublishedFile {
                path: outside_json.clone(),
                format: OutputFormat::Json,
                dataset: DatasetKey::Static("ds"),
                identity: Identity::System,
            }],
            merged: Vec::new(),
            column_types: &[],
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }];
    let inv = build(BuildRequest {
        run_id: "RUN1",
        generation: "RUN1-abc",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });
    assert_eq!(
        inv.inventory_only.len(),
        1,
        "expected exactly one inventory-only record, got {:?}",
        inv.inventory_only
    );
    let outside_display = outside_json.display().to_string();
    assert!(
        inv.warnings.iter().any(|w| w.contains(&outside_display)),
        "expected a warning naming the out-of-root JSON path {outside_display:?}, got {:?}",
        inv.warnings
    );
}

// ---------------------------------------------------------------------
// The matched pair, and the run that writes it.
// ---------------------------------------------------------------------

use assert_cmd::Command;
use triage_core::output::duckdb::inventory::{Inventory, Status};
use triage_orchestrator::duckdb::{generation_id, write_pair};
use triage_testkit::synthetic::write_collection;

/// A minimal, genuinely valid Version-17 SCCA prefetch file, and a minimal,
/// genuinely valid Shell Link. Both are lifted from `velo_layout.rs`, which
/// documents them in full; they are duplicated rather than shared because
/// `triage_testkit::synthetic` deliberately carries no parser-specific
/// fixtures.
///
/// `synthetic::write_collection` alone -- which the brief for this task
/// named -- carries no artifact any tool parses, so a run over it publishes
/// nothing and every inventory it produces is empty. These two give the run
/// something to actually inventory: one system-wide dataset (PETriage) and
/// one per-user dataset (LETriage, attributed to `jdoe` by its path), so the
/// Velo layout's per-user slices and category merge are both exercised.
fn build_prefetch() -> Vec<u8> {
    const EXE_NAME: &str = "TEST.EXE";
    const FI: usize = 84;
    const FILENAME_STRINGS_OFFSET: usize = FI + 68;

    let filename_strings: Vec<u8> = format!("{EXE_NAME}\0")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let total = FILENAME_STRINGS_OFFSET + filename_strings.len();
    let mut buf = vec![0u8; total];

    buf[0..4].copy_from_slice(&17u32.to_le_bytes());
    buf[4..8].copy_from_slice(b"SCCA");
    buf[12..16].copy_from_slice(&(total as u32).to_le_bytes());

    let name_utf16: Vec<u8> = EXE_NAME.encode_utf16().flat_map(u16::to_le_bytes).collect();
    buf[16..16 + name_utf16.len()].copy_from_slice(&name_utf16);
    buf[76..80].copy_from_slice(&0x1234_5678u32.to_le_bytes());

    buf[FI + 16..FI + 20].copy_from_slice(&(FILENAME_STRINGS_OFFSET as u32).to_le_bytes());
    buf[FI + 20..FI + 24].copy_from_slice(&(filename_strings.len() as u32).to_le_bytes());
    buf[FI + 36..FI + 44].copy_from_slice(&133_333_333_330_000_000u64.to_le_bytes());
    buf[FI + 60..FI + 64].copy_from_slice(&1u32.to_le_bytes());

    buf[FILENAME_STRINGS_OFFSET..total].copy_from_slice(&filename_strings);
    buf
}

fn build_lnk() -> Vec<u8> {
    const CLASS_ID: [u8; 16] = [
        0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    let mut buf = vec![0u8; 0x4C];
    buf[0..4].copy_from_slice(&0x0000_004Cu32.to_le_bytes());
    buf[4..20].copy_from_slice(&CLASS_ID);
    buf
}

/// A synthetic collection that actually parses. Not the gate-passing
/// variant: every run over it passes `--no-validate`, matching `e2e.rs` and
/// `zip_e2e.rs`, because the gate's placeholder hives and EVTX would be
/// handed to the parsers and fail.
fn write_parsable_collection(dir: &Path, host: &str) {
    write_collection(dir, host);
    let uploads = dir.join("uploads/auto/C%3A");
    let pf_dir = uploads.join("Windows/Prefetch");
    std::fs::create_dir_all(&pf_dir).expect("mkdir prefetch");
    std::fs::write(pf_dir.join("TEST.EXE-12345678.pf"), build_prefetch()).expect("write pf");

    let lnk_dir = uploads.join("Users/jdoe/AppData/Roaming/Microsoft/Windows/Recent");
    std::fs::create_dir_all(&lnk_dir).expect("mkdir recent");
    std::fs::write(lnk_dir.join("test.lnk"), build_lnk()).expect("write lnk");
}

/// Criterion 22: both files carry the same generation, so a consumer can
/// detect a torn pair.
#[test]
fn the_pair_shares_one_generation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let generation = generation_id("RUN1");
    let inv = Inventory::empty(
        "RUN1",
        &generation,
        "2026-09-17T12:00:00.0000000Z",
        root,
        Status::Ok,
    );
    write_pair(root, &inv).expect("write pair");

    let json = std::fs::read_to_string(root.join("duckdb/datasets.json")).expect("json");
    let sql = std::fs::read_to_string(root.join("duckdb/views.sql")).expect("sql");
    assert!(json.contains(&generation), "{json}");
    assert!(sql.contains(&generation), "{sql}");
}

/// Criterion 21: a later run's artifacts replace an earlier run's entirely.
/// Leaving the earlier pair in place would make a rejected run look like the
/// successful one before it.
#[test]
fn a_second_generation_replaces_the_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let first = generation_id("RUN1");
    let mut inv = Inventory::empty("RUN1", &first, "t", root, Status::Ok);
    inv.warnings.push("first".into());
    write_pair(root, &inv).expect("first write");

    let second = generation_id("RUN2");
    let inv2 = Inventory::empty("RUN2", &second, "t", root, Status::RunRejected);
    write_pair(root, &inv2).expect("second write");

    let json = std::fs::read_to_string(root.join("duckdb/datasets.json")).expect("json");
    let sql = std::fs::read_to_string(root.join("duckdb/views.sql")).expect("sql");
    assert!(json.contains(&second), "{json}");
    assert!(!json.contains(&first), "the first generation must be gone");
    assert!(!json.contains("first"), "the first warning must be gone");
    assert!(sql.contains("run-rejected"), "{sql}");
}

/// A generation id is unique per call, so two runs in the same second do not
/// produce the same one.
#[test]
fn generation_ids_do_not_repeat() {
    let a = generation_id("RUN1");
    let b = generation_id("RUN1");
    assert_ne!(a, b);
    assert!(a.starts_with("RUN1-"), "{a}");
}

fn run_orchestrator(args: &[&str]) -> std::process::Output {
    Command::cargo_bin("TriageSuite")
        .expect("binary")
        .args(args)
        .output()
        .expect("run")
}

fn inventory_at(out: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(out.join("duckdb/datasets.json"))
        .expect("duckdb/datasets.json must exist");
    serde_json::from_str(&text).expect("valid inventory json")
}

/// Criterion 19: every output layout the CLI offers produces a loadable
/// views.sql. The paths differ wildly between them -- the Velo tree nests
/// per-user slices under a category directory, the native one does not --
/// and the view layer is built from what the router published rather than
/// from the shape of a path, so both must work identically.
///
/// `OutputLayoutMode::Nested` is the third shape but has no orchestrator
/// flag; it is covered by the triage-core tests.
#[test]
fn every_cli_layout_produces_loadable_views() {
    for layout in ["velo", "native"] {
        let td = tempfile::tempdir().expect("tempdir");
        let capture = td.path().join("Collection-H1");
        write_parsable_collection(&capture, "H1");
        let out = td.path().join(format!("out-{layout}"));

        let o = run_orchestrator(&[
            "run",
            "--no-validate",
            capture.to_str().expect("utf8"),
            "--out",
            out.to_str().expect("utf8"),
            "--csv",
            "--overwrite",
            "--layout",
            layout,
        ]);
        assert!(o.status.success(), "{layout} run failed: {o:?}");

        let inv = inventory_at(&out);
        assert_eq!(inv["status"], "ok", "{layout}: {inv}");
        assert!(
            !inv["datasets"].as_array().expect("datasets").is_empty(),
            "{layout} produced no datasets: {inv}"
        );
        assert!(
            inv["dropped_overrides"]
                .as_array()
                .expect("dropped_overrides")
                .is_empty(),
            "{layout} dropped an override: {inv}"
        );

        if let Some(duckdb) = triage_testkit::duckdb_binary() {
            let sql = std::fs::read_to_string(out.join("duckdb/views.sql")).expect("sql");
            // Loading every view is the assertion; the count is incidental.
            duckdb_query(&duckdb, &sql, "SELECT 1;");
        }
    }
}

/// Criterion 20: a JSON-only run has nothing to build a view over, and says
/// so rather than writing an empty file that looks like a truncated write.
#[test]
fn a_json_only_run_says_so() {
    let td = tempfile::tempdir().expect("tempdir");
    let capture = td.path().join("Collection-H1");
    write_parsable_collection(&capture, "H1");
    let out = td.path().join("out");

    let o = run_orchestrator(&[
        "run",
        "--no-validate",
        capture.to_str().expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
        "--json",
        "--overwrite",
    ]);
    assert!(o.status.success(), "run failed: {o:?}");

    let inv = inventory_at(&out);
    assert_eq!(inv["status"], "no-csv-output", "{inv}");
    assert!(inv["datasets"].as_array().expect("datasets").is_empty());
    assert!(
        !inv["inventory_only"]
            .as_array()
            .expect("inventory_only")
            .is_empty(),
        "the JSON files must still be inventoried: {inv}"
    );

    let sql = std::fs::read_to_string(out.join("duckdb/views.sql")).expect("sql");
    assert!(!sql.contains("CREATE OR REPLACE VIEW"), "{sql}");
    assert!(sql.contains("no-csv-output"), "{sql}");
}

/// Criterion 21: a rejected run over a reused --out replaces the previous
/// run's artifacts. Leaving them would make a refused input look like the
/// successful run before it -- the reader has no way to tell.
///
/// The refusal is the pre-flight gate, driven exactly as
/// `validate_e2e.rs::run_without_no_validate_skips_a_deficient_capture_and_records_why`
/// drives it: a bare `synthetic::write_collection` -- no event logs, no
/// hives -- run *without* `--no-validate`, which that test already proves
/// exits 3 and records zero hosts. A path that is not a collection at all
/// would be refused a step earlier, by `input::prepare`; the gate is the
/// later of the two rejection paths and the one `run()` reaches with an
/// empty host list, so it is the one worth pinning here.
#[test]
fn a_rejected_run_replaces_an_earlier_runs_views() {
    let td = tempfile::tempdir().expect("tempdir");
    let capture = td.path().join("Collection-H1");
    write_parsable_collection(&capture, "H1");
    let out = td.path().join("out");

    let first = run_orchestrator(&[
        "run",
        "--no-validate",
        capture.to_str().expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
        "--csv",
        "--overwrite",
    ]);
    assert!(first.status.success(), "first run failed: {first:?}");
    let before = inventory_at(&out);
    assert_eq!(before["status"], "ok");
    assert!(
        !before["datasets"].as_array().expect("datasets").is_empty(),
        "the first run must leave views to replace: {before}"
    );
    let old_generation = before["generation"]
        .as_str()
        .expect("generation")
        .to_string();

    let deficient = td.path().join("Collection-DEFICIENT");
    write_collection(&deficient, "DEFICIENT");
    let second = run_orchestrator(&[
        "run",
        deficient.to_str().expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
        "--csv",
        "--overwrite",
    ]);
    assert!(!second.status.success(), "a refused input must not succeed");
    assert_eq!(
        second.status.code(),
        Some(3),
        "the gate's refusal is exit 3: {second:?}"
    );

    let after = inventory_at(&out);
    assert_eq!(after["status"], "run-rejected", "{after}");
    assert_ne!(after["generation"], old_generation.as_str());
    let sql = std::fs::read_to_string(out.join("duckdb/views.sql")).expect("sql");
    assert!(
        !sql.contains("CREATE OR REPLACE VIEW"),
        "the earlier run's views must be gone:\n{sql}"
    );
}

/// Criterion 23: a collection that moved gets working SQL back from its own
/// inventory, because the inventory stores paths relative to the root and
/// only the rendered SQL is absolute.
#[test]
fn regenerate_rewrites_absolute_paths_under_a_new_root() {
    let Some(duckdb) = triage_testkit::duckdb_binary() else {
        return;
    };
    let old = tempfile::tempdir().expect("tempdir");
    let new = tempfile::tempdir().expect("tempdir");

    // Generate under the old root.
    let hosts = typed_host(old.path());
    let generation = triage_orchestrator::duckdb::generation_id("RUN1");
    let inv = build(BuildRequest {
        run_id: "RUN1",
        generation: &generation,
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: old.path(),
        hosts: &hosts,
    });
    triage_orchestrator::duckdb::write_pair(old.path(), &inv).expect("write");

    // Move the whole collection.
    for entry in std::fs::read_dir(old.path()).expect("read old") {
        let entry = entry.expect("entry");
        let target = new.path().join(entry.file_name());
        copy_tree(&entry.path(), &target);
    }

    triage_orchestrator::duckdb::regenerate(new.path()).expect("regenerate");

    let sql = std::fs::read_to_string(new.path().join("duckdb/views.sql")).expect("sql");
    assert!(
        sql.contains(&new.path().display().to_string()),
        "paths must be under the new root:\n{sql}"
    );
    assert!(
        !sql.contains(&old.path().display().to_string()),
        "no path may still point at the old root:\n{sql}"
    );
    assert!(!sql.contains(&generation), "a new generation is written");

    let out = duckdb_query(&duckdb, &sql, "SELECT count(*) FROM t_ds;");
    assert_eq!(out, "2");
}

fn copy_tree(from: &Path, to: &Path) {
    if from.is_dir() {
        std::fs::create_dir_all(to).expect("mkdir");
        for entry in std::fs::read_dir(from).expect("read dir") {
            let entry = entry.expect("entry");
            copy_tree(&entry.path(), &to.join(entry.file_name()));
        }
    } else {
        std::fs::copy(from, to).expect("copy");
    }
}

/// Every declared override must name a column its tool really emits, AND a
/// declared column must actually `TRY_CAST` to something on real evidence.
///
/// A stale declaration is dropped at generation time rather than breaking
/// the SQL, which is the right runtime behaviour and precisely why it needs
/// a test: the degradation is silent, so nothing else would ever notice that
/// a renamed column had quietly untyped itself. The `dropped_overrides`
/// check alone only proves a declared column *name* exists in the CSV
/// header -- it says nothing about whether DuckDB can parse the values that
/// live there. Task 8 proved `TRY_CAST` against synthetic values; it never
/// exercised these ten crates' real emitted formats (amc's
/// 7-digit-fraction ISO-8601 with a `Z` suffix, the
/// `0001-01-01T00:00:00.0000000Z` parse-failure sentinel, ...). If DuckDB
/// rejected one of those, every value in that column would silently become
/// NULL -- with only the `__text` companion surviving -- and every other
/// test in this workspace would still pass. So this reuses the same
/// (expensive) run to also assert, per seeded tool, that at least one
/// declared column is non-NULL on at least one real row.
///
/// Capture-gated on purpose. A synthetic collection holds no Amcache hive,
/// no prefetch file and no jump list, so a run over one emits none of the
/// seeded datasets and this assertion would pass over an empty set.
#[test]
fn no_declared_override_names_a_column_its_dataset_lacks() {
    // The brief's own snippet used a bare `Path::new("test captures")`, which
    // only resolves if the test binary's CWD is the workspace root. Cargo
    // actually runs it from this crate's manifest dir, as every other
    // capture-gated test in this file's own crate already accounts for
    // (`validate_e2e.rs`, `velo_plugin_detail_file.rs`): go up two levels.
    //
    // ONE collection, not the whole tree. Pointing this at `test captures/`
    // ran all three collections and took 25 minutes once MFTriage and
    // EvtxTriage joined the seeded list -- they alone account for ~7.3M MFT
    // records and ~2.8M events across the three. A second host proves
    // nothing this test is about: the declarations are compile-time
    // constants, identical for every host, so the second and third runs
    // re-assert the first's result at full price.
    //
    // STCL1 specifically, and not either of its siblings, because it is the
    // only one of the three where all nine seeded tools emit records --
    // DESKTOP-OA8SHHC has no recycle-bin evidence (RBTriage: 0 records) and
    // STDC1 has none for RBTriage, PETriage or WxTTriage either. On the
    // other two the seeded-tool assertion below would fail, which is the
    // check working, not a reason to weaken it. STCL1 also carries the
    // smallest $MFT of the three (1.19M records against STDC1's 4.63M).
    let capture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test captures/Collection-STCL1_umbralabs_dev-2026-03-11T21_20_14Z");
    if triage_testkit::skip_if_missing(&capture, "DuckDB override drift check") {
        return;
    }

    let td = tempfile::tempdir().expect("tempdir");
    let out = td.path().join("out");
    // This list and the seeded-tool assertion list below MUST stay in sync:
    // a tool that gains a `column_types()` declaration without being added
    // here would have its drift go unchecked, which is exactly the silent
    // failure mode this test exists to catch. Without `--only`, a run over
    // the collection exercises every registered parser, not just the nine
    // this test asserts about, which is minutes of unrelated work this test
    // does not need to prove its point. Registry keys are
    // `crates/triage-orchestrator/src/registry.rs`'s `build()` match arms,
    // not each tool's `binary_name()` -- note `srum`, not `srume`.
    let o = run_orchestrator(&[
        "run",
        "--no-validate",
        capture.to_str().expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
        "--csv",
        "--overwrite",
        "--only",
        "amc,pe,le,jle,rb,srum,wxt,mft,evtx,re",
    ]);
    assert!(o.status.success(), "run failed: {o:?}");

    let inv = inventory_at(&out);
    let dropped = inv["dropped_overrides"]
        .as_array()
        .expect("dropped_overrides");
    assert!(
        dropped.is_empty(),
        "a declared override names a column its dataset does not emit:\n{:#}",
        inv["dropped_overrides"]
    );

    // The gate above is only meaningful if the seeded tools actually ran.
    let tools: Vec<&str> = inv["datasets"]
        .as_array()
        .expect("datasets")
        .iter()
        .filter_map(|d| d["tool"].as_str())
        .collect();
    for seeded in [
        "AmcacheTriage",
        "PETriage",
        "LETriage",
        "JLETriage",
        "RBTriage",
        "SrumETriage",
        "WxTTriage",
        "MFTriage",
        "EvtxTriage",
        "RETriage",
    ] {
        assert!(
            tools.contains(&seeded),
            "{seeded} emitted no dataset, so its overrides were never checked: {tools:?}"
        );
    }

    // The checks above only prove a declared column's NAME survived into
    // the CSV header. Now prove DuckDB can actually parse what is in it --
    // but "prove a declared column is non-NULL somewhere" is the wrong
    // property: a column that is legitimately blank on every row of a
    // dataset (SrumETriage's `ExeTimestamp` in `AppResourceUseInfo`, for
    // one -- confirmed against SrumECmd's own reference output, which
    // leaves it blank there too) would fail that check while being exactly
    // correct. The real property, which is the whole reason the `__text`
    // companion column exists, is narrower: a row is a CAST FAILURE only
    // when its text is non-NULL (there was a value) and its typed column is
    // NULL (TRY_CAST rejected it). Blank text casting to NULL is a MISSING
    // VALUE, not a failure, and must never trip this.
    //
    // Checks every declared column of every dataset in the inventory (all
    // seeded tools, not just one column each), reusing this test's own run.
    // Skipped (not run) when `duckdb` is not on PATH, exactly like every
    // other DuckDB-executing test in this file -- but `duckdb_binary()`
    // itself panics rather than skips under `TRIAGE_REQUIRE_DUCKDB=1`, so
    // this cannot silently stop proving anything in an environment that
    // demands it.
    if let Some(duckdb) = triage_testkit::duckdb_binary() {
        let sql = std::fs::read_to_string(out.join("duckdb/views.sql")).expect("sql");
        let datasets = inv["datasets"].as_array().expect("datasets");

        // Summed across every declared column of every dataset. A run that
        // cast nothing (e.g. because every declared column happened to be
        // blank everywhere) must not pass this by vacuously finding zero
        // cast failures: it has proven nothing about TRY_CAST actually
        // working, so it must fail on this floor instead.
        let mut total_non_null: u64 = 0;
        let mut columns_checked: u64 = 0;

        for dataset in datasets {
            let Some(effective_types) = dataset["effective_types"].as_object() else {
                continue;
            };
            if effective_types.is_empty() {
                continue;
            }
            let view = dataset["view"].as_str().expect("view");

            // (column, text_column, sql_type), in the exact order the
            // SELECT list below is built in -- `-noheader` output carries
            // no column names, so position is the only way back to a name.
            // `text_column` is read from `effective_types`, never assumed
            // to be `<column>__text`: the allocator renames it on collision.
            let columns: Vec<(&str, &str, &str)> = effective_types
                .iter()
                .map(|(name, v)| {
                    (
                        name.as_str(),
                        v["text_column"].as_str().expect("text_column"),
                        v["sql_type"].as_str().expect("sql_type"),
                    )
                })
                .collect();

            let select_list = columns
                .iter()
                .map(|(col, text_col, _)| {
                    format!(
                        "count(*) FILTER (WHERE \"{text_col}\" IS NOT NULL AND \"{col}\" IS NULL), \
                         count(\"{col}\")"
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let row = duckdb_query(
                &duckdb,
                &sql,
                &format!(r#"SELECT {select_list} FROM "{view}";"#),
            );
            let values: Vec<i64> = row
                .split('|')
                .map(|cell| {
                    cell.trim().parse::<i64>().unwrap_or_else(|_| {
                        panic!("{view}: non-numeric cell {cell:?} in row {row:?}")
                    })
                })
                .collect();
            assert_eq!(
                values.len(),
                columns.len() * 2,
                "{view}: expected {} values (a fail-count and a non-null-count per declared \
                 column), got {}: {row:?}",
                columns.len() * 2,
                values.len()
            );

            for (i, (col, text_col, sql_type)) in columns.iter().enumerate() {
                let fail_count = values[i * 2];
                let non_null_count = values[i * 2 + 1];
                if fail_count > 0 {
                    let sample = duckdb_query(
                        &duckdb,
                        &sql,
                        &format!(
                            r#"SELECT "{text_col}" FROM "{view}" WHERE "{text_col}" IS NOT NULL AND "{col}" IS NULL LIMIT 1;"#
                        ),
                    );
                    panic!(
                        "{view}: declared column \"{col}\" ({sql_type}) failed to TRY_CAST on \
                         {fail_count} row(s) that had a non-blank \"{text_col}\" value. One \
                         offending raw value: {sample:?}"
                    );
                }
                total_non_null += non_null_count.max(0) as u64;
                columns_checked += 1;
            }
        }

        assert!(
            total_non_null > 0,
            "{columns_checked} declared columns were checked across every seeded tool's \
             dataset, and not one had a single non-NULL typed value -- this run casts \
             nothing, so TRY_CAST success was never actually exercised"
        );
        eprintln!(
            "TRY_CAST non-vacuity: {total_non_null} non-null typed values across \
             {columns_checked} declared columns, zero cast failures"
        );
    }
}

/// `Status::GenerationFailed` is not decoration: serializing the inventory
/// really can fail, and when it does the pair must still be written saying
/// so. A silent give-up leaves a reused `--out` holding the previous run's
/// pair, looking current.
///
/// Unix-only because this needs a path that is not valid UTF-8, and only
/// Unix lets one exist. Nothing touches the filesystem with it -- the bad
/// bytes live in the inventory struct, not in a real directory name.
#[cfg(unix)]
#[test]
fn an_unserializable_inventory_still_writes_an_honest_pair() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let generation = generation_id("RUN1");
    let mut inv = Inventory::empty("RUN1", &generation, "t", root, Status::Ok);
    // Lone 0xFF: valid on any Unix filesystem, not valid UTF-8, and rejected
    // by serde_json when it serializes the PathBuf.
    inv.out_root = Path::new(OsStr::from_bytes(b"/evidence/\xff")).to_path_buf();
    assert!(
        serde_json::to_string(&inv).is_err(),
        "the premise of this test is that this inventory cannot serialize"
    );

    write_pair(root, &inv).expect("the pair is written anyway");

    let json = std::fs::read_to_string(root.join("duckdb/datasets.json")).expect("json");
    let sql = std::fs::read_to_string(root.join("duckdb/views.sql")).expect("sql");

    let parsed: Inventory = serde_json::from_str(&json).expect("the written json parses");
    assert_eq!(parsed.status, Status::GenerationFailed);
    assert_eq!(parsed.generation, generation, "still a matched pair");
    assert!(parsed.datasets.is_empty(), "no views are claimed");
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.contains("could not be serialized")),
        "the reason must be in the record: {:?}",
        parsed.warnings
    );
    assert!(sql.contains("generation-failed"), "{sql}");
    assert!(sql.contains(&generation), "{sql}");
}

/// The run's exit status is not affected by a DuckDB write failure of any
/// kind -- including the one above. `emit_duckdb_views` is called after the
/// manifest is written and only ever warns on stderr.
#[test]
fn a_duckdb_write_failure_cannot_change_the_run_exit_status() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("collection");
    write_parsable_collection(&src, "HOST1");
    let out = dir.path().join("out");

    // Make the DuckDB directory un-writable by planting a FILE where
    // `write_pair` needs a directory: create_dir_all then fails.
    std::fs::create_dir_all(&out).expect("mkdir out");
    std::fs::write(out.join("duckdb"), b"not a directory").expect("plant file");

    let output = run_orchestrator(&[
        "run",
        "--no-validate",
        src.to_str().expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
        "--csv",
        "--overwrite",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        out.join("run_manifest.json").is_file(),
        "the evidence record is still written"
    );
}

/// `EffectiveType::sql_type` is interpolated UNQUOTED into
/// `TRY_CAST(... AS {sql_type})`. On a fresh run it can only be one of four
/// constants, but `regenerate` reads it back off disk, where a tampered
/// inventory turns it into arbitrary SQL in a file an analyst runs against
/// evidence. It must be refused -- and refused cleanly: a nonzero exit, no
/// panic, and the existing views.sql left exactly as it was.
#[test]
fn regenerate_refuses_a_tampered_sql_type_and_rewrites_nothing() {
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path();

    let hosts = typed_host(root);
    let inv = build(BuildRequest {
        run_id: "RUN1",
        generation: &triage_orchestrator::duckdb::generation_id("RUN1"),
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });
    triage_orchestrator::duckdb::write_pair(root, &inv).expect("write");

    let json_path = root.join("duckdb/datasets.json");
    let sql_path = root.join("duckdb/views.sql");

    // Tamper: a payload that would close the TRY_CAST and run its own
    // statements if it were ever rendered.
    const PAYLOAD: &str = "TIMESTAMP) AS x; DROP TABLE y; --";
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json_path).expect("json")).expect("parse");
    let types = doc["datasets"][0]["effective_types"]
        .as_object_mut()
        .expect("effective_types");
    let column = types.keys().next().expect("a typed column").clone();
    types[&column]["sql_type"] = serde_json::Value::String(PAYLOAD.into());
    std::fs::write(&json_path, serde_json::to_string_pretty(&doc).expect("ser")).expect("write");

    // Snapshot BOTH files' exact bytes after tampering and before any
    // regenerate attempt. The tampered `datasets.json` is what must survive
    // untouched: refusing must not "helpfully" repair or rewrite it, and
    // `views.sql` must not have picked up the payload either.
    let json_after_tamper = std::fs::read(&json_path).expect("json");
    let sql_after_tamper = std::fs::read(&sql_path).expect("sql");

    let error = triage_orchestrator::duckdb::regenerate(root).expect_err("must be refused");
    let text = error.to_string();
    assert!(text.contains(&column), "names the column: {text}");
    assert!(text.contains(PAYLOAD), "names the offending value: {text}");

    assert_eq!(
        std::fs::read(&json_path).expect("json"),
        json_after_tamper,
        "datasets.json must not be rewritten by the direct regenerate() call"
    );
    assert_eq!(
        std::fs::read(&sql_path).expect("sql"),
        sql_after_tamper,
        "views.sql must not be rewritten by the direct regenerate() call"
    );
    assert!(
        !String::from_utf8_lossy(&sql_after_tamper).contains("DROP TABLE"),
        "and the payload never reached it"
    );

    // And through the CLI: a clean nonzero exit, never a panic, and still
    // neither file touched.
    let out = run_orchestrator(&[
        "duckdb",
        "regenerate",
        "--out",
        root.to_str().expect("utf8"),
    ]);
    assert_eq!(out.status.code(), Some(4), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not one of"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");

    assert_eq!(
        std::fs::read(&json_path).expect("json"),
        json_after_tamper,
        "datasets.json must not be rewritten by the CLI regenerate call"
    );
    assert_eq!(
        std::fs::read(&sql_path).expect("sql"),
        sql_after_tamper,
        "views.sql must not be rewritten by the CLI regenerate call"
    );
}
