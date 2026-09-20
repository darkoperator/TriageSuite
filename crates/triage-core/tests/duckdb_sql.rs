//! Escaping and naming. A string literal and an identifier are quoted by
//! different characters and must never share a function: a path containing a
//! double quote is harmless inside a literal and a column named with an
//! apostrophe is harmless inside an identifier, and swapping the two
//! produces SQL that is at best broken and at worst wrong.

use std::collections::BTreeMap;
use std::path::PathBuf;
use triage_core::attribution::Identity;
use triage_core::output::duckdb::build::{
    build, BuildRequest, HostOutputs, MergedFile, ToolOutputs,
};
use triage_core::output::duckdb::inventory::{
    Dataset, DatasetFile, DroppedOverride, EffectiveType, FileIdentity, FileRole, Inventory,
    InventoryOnly, MetadataColumns, Status, SCHEMA_VERSION,
};
use triage_core::output::duckdb::render::render_sql;
use triage_core::output::duckdb::sql::{quote_ident, quote_literal, view_name, NameAllocator};
use triage_core::output::duckdb::types::{ColumnType, DatasetColumnTypes, SqlType, TimeSemantics};
use triage_core::output::published::{DatasetKey, OutputFormat, PublishedFile};

#[test]
fn a_string_literal_doubles_single_quotes_and_leaves_double_quotes_alone() {
    assert_eq!(quote_literal("plain"), "'plain'");
    assert_eq!(quote_literal("it's"), "'it''s'");
    assert_eq!(quote_literal("say \"hi\""), "'say \"hi\"'");
    assert_eq!(
        quote_literal("/tmp/o'brien/out.csv"),
        "'/tmp/o''brien/out.csv'"
    );
}

#[test]
fn an_identifier_doubles_double_quotes_and_leaves_single_quotes_alone() {
    assert_eq!(quote_ident("plain"), "\"plain\"");
    assert_eq!(quote_ident("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(quote_ident("it's"), "\"it's\"");
}

#[test]
fn a_view_name_is_lowercased_and_sanitized() {
    assert_eq!(
        view_name("AmcacheTriage", "device_containers"),
        "amcachetriage_device_containers"
    );
    assert_eq!(view_name("PETriage", "Timeline"), "petriage_timeline");
    assert_eq!(view_name("X", "a b/c"), "x_a_b_c");
}

/// A name may not start with a digit, so it is prefixed rather than having
/// the digit dropped -- dropping it could collide two distinct datasets.
#[test]
fn a_view_name_starting_with_a_digit_is_prefixed() {
    assert_eq!(view_name("7Zip", "x"), "_7zip_x");
}

/// Metadata names are allocated against every header in the dataset, case
/// insensitively, because DuckDB resolves identifiers case insensitively.
#[test]
fn a_metadata_name_that_collides_is_suffixed() {
    let mut alloc = NameAllocator::new(["_TRIAGE_HOST".to_string(), "other".to_string()]);
    assert_eq!(alloc.allocate("_triage_host"), "_triage_host_1");
}

#[test]
fn allocation_keeps_advancing_past_taken_suffixes() {
    let mut alloc = NameAllocator::new(["_triage_host".to_string(), "_triage_host_1".to_string()]);
    assert_eq!(alloc.allocate("_triage_host"), "_triage_host_2");
}

/// Two allocations of the same desired name must not both succeed: the
/// allocator takes what it hands out.
#[test]
fn an_allocated_name_is_itself_taken() {
    let mut alloc = NameAllocator::new(Vec::<String>::new());
    assert_eq!(alloc.allocate("c__text"), "c__text");
    assert_eq!(alloc.allocate("c__text"), "c__text_1");
}

/// The SQL spellings are load-bearing: they are pasted into a TRY_CAST.
/// BIGINT is signed and cannot hold u64::MAX, which is why UBIGINT exists
/// as a separate variant rather than a flag.
#[test]
fn sql_types_spell_themselves_for_a_try_cast() {
    assert_eq!(SqlType::Timestamp.sql(), "TIMESTAMP");
    assert_eq!(SqlType::BigInt.sql(), "BIGINT");
    assert_eq!(SqlType::UBigInt.sql(), "UBIGINT");
    assert_eq!(SqlType::Boolean.sql(), "BOOLEAN");
}

/// Zone semantics are declared, never inferred, and the inventory spells
/// them out so a consumer never has to assume UTC.
#[test]
fn time_semantics_name_themselves() {
    assert_eq!(TimeSemantics::Utc.as_str(), "utc");
    assert_eq!(TimeSemantics::Local.as_str(), "local");
    assert_eq!(TimeSemantics::Unknown.as_str(), "unknown");
}

/// The inventory round-trips: `duckdb regenerate` reads back a file this
/// same code wrote, so a field that serializes but does not deserialize
/// would break relocation and nothing else would notice.
#[test]
fn an_inventory_round_trips_through_json() {
    let inv = Inventory::empty(
        "20260917120000123",
        "20260917120000123-4f1a9c02b7d3e650",
        "2026-09-17T12:00:00.0000000Z",
        std::path::Path::new("/abs/out"),
        Status::RunRejected,
    );
    let text = serde_json::to_string_pretty(&inv).expect("serialize");
    let back: Inventory = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(back.schema_version, SCHEMA_VERSION);
    assert_eq!(back.status, Status::RunRejected);
    assert_eq!(back.generation, inv.generation);
    assert!(back.datasets.is_empty());
    assert!(text.contains("\"status\": \"run-rejected\""));
}

/// The empty-inventory round trip above never constructs `Dataset`,
/// `DatasetFile`, `FileRole`, `FileIdentity`, `EffectiveType`,
/// `MetadataColumns`, `InventoryOnly` or `DroppedOverride`, so a serde
/// mistake in any of those eight types would compile and pass silently.
/// `FileIdentity` is the highest risk: it is an internally tagged enum, and
/// a tag or rename mistake surfaces only on an actual round trip.
#[test]
fn a_fully_populated_inventory_round_trips_through_json() {
    let mut effective_types = BTreeMap::new();
    effective_types.insert(
        "LastWriteTime".to_string(),
        EffectiveType::new(
            SqlType::Timestamp,
            "LastWriteTime_text".to_string(),
            Some(TimeSemantics::Utc),
        ),
    );
    effective_types.insert(
        "RecordId".to_string(),
        EffectiveType::new(SqlType::UBigInt, "RecordId_text".to_string(), None),
    );

    let files = vec![
        // FileRole::Primary, identity = User { name }, derived_into = None.
        DatasetFile {
            path: PathBuf::from("HOST-A/AmcacheTriage/device_containers.csv"),
            host: "HOST-A".to_string(),
            identity: Some(FileIdentity::User {
                name: "alice".to_string(),
            }),
            role: FileRole::Primary,
            included_in_view: true,
            derived_into: None,
            header: Some(vec!["LastWriteTime".to_string(), "RecordId".to_string()]),
            warnings: Vec::new(),
        },
        // FileRole::Slice, identity = System, derived_into = Some(..), and a
        // non-empty warnings vector.
        DatasetFile {
            path: PathBuf::from("HOST-A/VeloTriage/users/alice/proclog.csv"),
            host: "HOST-A".to_string(),
            identity: Some(FileIdentity::System),
            role: FileRole::Slice,
            included_in_view: false,
            derived_into: Some(PathBuf::from("HOST-A/VeloTriage/proclog.csv")),
            header: Some(vec!["Pid".to_string()]),
            warnings: vec!["slice folded into merged file".to_string()],
        },
        // FileRole::Merged, identity = None -- load-bearing: a merged file
        // spans every user whose slice fed it.
        DatasetFile {
            path: PathBuf::from("HOST-A/VeloTriage/proclog.csv"),
            host: "HOST-A".to_string(),
            identity: None,
            role: FileRole::Merged,
            included_in_view: true,
            derived_into: None,
            header: Some(vec!["Pid".to_string(), "TriageUser".to_string()]),
            warnings: Vec::new(),
        },
        // identity = Unknown, header = None (unreadable).
        DatasetFile {
            path: PathBuf::from("HOST-A/AmcacheTriage/unreadable.csv"),
            host: "HOST-A".to_string(),
            identity: Some(FileIdentity::Unknown),
            role: FileRole::Primary,
            included_in_view: true,
            derived_into: None,
            header: None,
            warnings: vec!["header could not be read".to_string()],
        },
    ];

    let dataset = Dataset {
        view: "amcachetriage_device_containers".to_string(),
        raw_view: "amcachetriage_device_containers_raw".to_string(),
        tool: "AmcacheTriage".to_string(),
        dataset_id: "device_containers".to_string(),
        source: "internal".to_string(),
        metadata_columns: MetadataColumns {
            run_id: "_TRIAGE_RUN_ID".to_string(),
            host: "_TRIAGE_HOST".to_string(),
            identity: "_TRIAGE_IDENTITY".to_string(),
            output_file: "_TRIAGE_OUTPUT_FILE".to_string(),
        },
        evidence_path_column: Some("KeyLastWriteTimestamp".to_string()),
        columns: vec!["LastWriteTime".to_string(), "RecordId".to_string()],
        effective_types,
        files,
    };

    let mut inv = Inventory::empty(
        "20260917120000123",
        "20260917120000123-4f1a9c02b7d3e650",
        "2026-09-17T12:00:00.0000000Z",
        std::path::Path::new("/abs/out"),
        Status::Ok,
    );
    inv.datasets.push(dataset);
    inv.inventory_only.push(InventoryOnly {
        tool: "PETriage".to_string(),
        dataset_id: "raw_export".to_string(),
        format: "json".to_string(),
        path: PathBuf::from("HOST-A/PETriage/raw_export.json"),
        reason: "json-only dataset; nothing to build a view over".to_string(),
    });
    inv.dropped_overrides.push(DroppedOverride {
        view: "amcachetriage_device_containers".to_string(),
        dataset_id: "device_containers".to_string(),
        column: "StaleColumn".to_string(),
        declared_type: "BIGINT".to_string(),
    });

    let text = serde_json::to_string_pretty(&inv).expect("serialize");
    // A silent tag or rename mistake on the internally tagged FileIdentity
    // enum would still parse; only the literal spelling proves it.
    assert!(text.contains("\"kind\": \"user\""));
    assert!(text.contains("\"kind\": \"system\""));
    assert!(text.contains("\"kind\": \"unknown\""));

    let back: Inventory = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(back, inv);
    assert_eq!(
        back.datasets[0].files[0].identity,
        Some(FileIdentity::User {
            name: "alice".to_string()
        })
    );
}

fn one_dataset_inventory() -> Inventory {
    let mut inv = Inventory::empty(
        "RUN1",
        "RUN1-abc",
        "2026-09-17T12:00:00.0000000Z",
        std::path::Path::new("/abs/o'brien"),
        Status::Ok,
    );
    let mut types = BTreeMap::new();
    types.insert(
        "KeyLastWriteTimestamp".to_string(),
        EffectiveType::new(
            SqlType::Timestamp,
            "KeyLastWriteTimestamp__text".to_string(),
            Some(TimeSemantics::Utc),
        ),
    );
    inv.datasets.push(Dataset {
        view: "amcachetriage_device_containers".into(),
        raw_view: "raw_amcachetriage_device_containers".into(),
        tool: "AmcacheTriage".into(),
        dataset_id: "device_containers".into(),
        source: "internal".into(),
        metadata_columns: MetadataColumns {
            run_id: "_triage_run_id".into(),
            host: "_triage_host".into(),
            identity: "_triage_identity".into(),
            output_file: "_triage_output_file".into(),
        },
        evidence_path_column: Some("SourceFile".into()),
        columns: vec!["KeyName".into(), "KeyLastWriteTimestamp".into()],
        effective_types: types,
        files: vec![DatasetFile {
            path: std::path::PathBuf::from("HOST-A/a.csv"),
            host: "HOST-A".into(),
            identity: Some(FileIdentity::User {
                name: "jdoe".into(),
            }),
            role: FileRole::Primary,
            included_in_view: true,
            derived_into: None,
            header: Some(vec!["KeyName".into(), "KeyLastWriteTimestamp".into()]),
            warnings: Vec::new(),
        }],
    });
    inv
}

/// Every reader option is spelled out. hive_partitioning=false in
/// particular is mandatory: an explicit file list under a `key=value`
/// directory still injects a phantom column without it.
#[test]
fn a_rendered_read_csv_pins_every_reader_option() {
    let sql = render_sql(&one_dataset_inventory());
    assert!(sql.contains("hive_partitioning=false"), "{sql}");
    assert!(sql.contains("all_varchar=true"), "{sql}");
    assert!(sql.contains("union_by_name=true"), "{sql}");
    assert!(sql.contains("header=true"), "{sql}");
    assert!(sql.contains("ignore_errors=false"), "{sql}");
}

/// Relative inventory paths are rendered absolute under out_root, and the
/// apostrophe in the root is escaped as a SQL literal.
#[test]
fn paths_are_absolute_and_escaped() {
    let sql = render_sql(&one_dataset_inventory());
    assert!(sql.contains("'/abs/o''brien/HOST-A/a.csv'"), "{sql}");
}

/// The typed view replaces the column in place and adds a text companion,
/// so a conversion failure stays distinguishable from a blank cell.
#[test]
fn the_typed_view_keeps_the_original_text() {
    let sql = render_sql(&one_dataset_inventory());
    assert!(
        sql.contains("TRY_CAST(\"KeyLastWriteTimestamp\" AS TIMESTAMP)"),
        "{sql}"
    );
    assert!(
        sql.contains("\"KeyLastWriteTimestamp\" AS \"KeyLastWriteTimestamp__text\""),
        "{sql}"
    );
    assert!(sql.contains("-- KeyLastWriteTimestamp: utc"), "{sql}");
}

/// A run with nothing queryable renders comments only -- never an empty
/// file, which would be indistinguishable from a truncated write.
#[test]
fn an_empty_inventory_renders_comments_only() {
    let inv = Inventory::empty(
        "RUN1",
        "RUN1-abc",
        "2026-09-17T12:00:00.0000000Z",
        std::path::Path::new("/abs/out"),
        Status::RunRejected,
    );
    let sql = render_sql(&inv);
    assert!(sql.contains("run-rejected"), "{sql}");
    assert!(!sql.contains("CREATE OR REPLACE VIEW"), "{sql}");
    assert!(sql.contains("generation: RUN1-abc"), "{sql}");
}

/// A file excluded from the view is not scanned. Including a merged file
/// and the slices it was built from would double every row.
#[test]
fn an_excluded_file_is_not_scanned() {
    let mut inv = one_dataset_inventory();
    inv.datasets[0].files[0].included_in_view = false;
    let sql = render_sql(&inv);
    assert!(!sql.contains("HOST-A/a.csv"), "{sql}");
}

fn csv_at(dir: &std::path::Path, rel: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, body).expect("write");
    path
}

/// A merged Velo file and the slices it was built from hold the same rows.
/// Scanning both would double every row, and DISTINCT would hide genuinely
/// duplicated evidence, so the slices are inventoried and excluded.
#[test]
fn a_merged_file_excludes_the_slices_it_consumed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let merged = csv_at(root, "H/cat/ds.csv", "A,TriageUser\n1,jdoe\n");
    let slice = csv_at(root, "H/cat/PerUser/ds_jdoe.csv", "A\n1\n");

    let hosts = vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![PublishedFile {
                path: slice.clone(),
                format: OutputFormat::Csv,
                dataset: DatasetKey::Static("ds"),
                identity: Identity::User("jdoe".into()),
            }],
            merged: vec![MergedFile {
                merged_path: merged.clone(),
                source_paths: vec![slice.clone()],
                format: OutputFormat::Csv,
            }],
            column_types: &[],
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }];
    let inv = build(BuildRequest {
        run_id: "R",
        generation: "R-1",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });

    let dataset = &inv.datasets[0];
    let merged_entry = dataset
        .files
        .iter()
        .find(|f| f.path.ends_with("ds.csv"))
        .expect("merged entry");
    let slice_entry = dataset
        .files
        .iter()
        .find(|f| f.path.ends_with("ds_jdoe.csv"))
        .expect("slice entry");
    assert!(merged_entry.included_in_view);
    assert!(merged_entry.identity.is_none(), "a merged file spans users");
    assert!(!slice_entry.included_in_view);
    assert_eq!(
        slice_entry.derived_into.as_deref(),
        Some(std::path::Path::new("H/cat/ds.csv"))
    );
}

/// An override naming a column the tool does not emit is dropped and
/// recorded. The column does not become VARCHAR -- it does not exist, so no
/// typed column and no text companion appear for it.
#[test]
fn an_override_for_an_absent_column_is_dropped_and_recorded() {
    const TYPES: &[DatasetColumnTypes] = &[DatasetColumnTypes {
        dataset_id: "ds",
        columns: &[
            ColumnType {
                column: "Present",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "Absent",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
        ],
    }];

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let file = csv_at(root, "H/ds.csv", "Present\n1\n");
    let hosts = vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![PublishedFile {
                path: file,
                format: OutputFormat::Csv,
                dataset: DatasetKey::Static("ds"),
                identity: Identity::System,
            }],
            merged: Vec::new(),
            column_types: TYPES,
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }];
    let inv = build(BuildRequest {
        run_id: "R",
        generation: "R-1",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });

    let dataset = &inv.datasets[0];
    assert!(dataset.effective_types.contains_key("Present"));
    assert!(!dataset.effective_types.contains_key("Absent"));
    assert_eq!(inv.dropped_overrides.len(), 1);
    assert_eq!(inv.dropped_overrides[0].column, "Absent");
}

/// Metadata names are allocated against the dataset's real headers, so a
/// dataset that already has a _triage_host column does not end up with two.
#[test]
fn a_colliding_metadata_name_is_allocated_around() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let file = csv_at(root, "H/ds.csv", "_triage_host,A\nx,1\n");
    let hosts = vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![PublishedFile {
                path: file,
                format: OutputFormat::Csv,
                dataset: DatasetKey::Static("ds"),
                identity: Identity::Unknown,
            }],
            merged: Vec::new(),
            column_types: &[],
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }];
    let inv = build(BuildRequest {
        run_id: "R",
        generation: "R-1",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });
    assert_eq!(inv.datasets[0].metadata_columns.host, "_triage_host_1");
}

/// A JSON file is inventoried but never scanned: JsonSink omits top-level
/// nulls, so NDJSON schema inference differs per host.
#[test]
fn a_json_file_is_inventory_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let json = root.join("H/ds.json");
    std::fs::create_dir_all(json.parent().expect("parent")).expect("mkdir");
    std::fs::write(&json, "{}\n").expect("write");
    let hosts = vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![PublishedFile {
                path: json,
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
        run_id: "R",
        generation: "R-1",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });
    assert!(inv.datasets.is_empty());
    assert_eq!(inv.inventory_only.len(), 1);
    assert_eq!(inv.inventory_only[0].reason, "json-not-viewed");
    assert_eq!(inv.status, Status::NoCsvOutput);
}

/// A published path that is no longer on disk was renamed out from under
/// the run -- the Velo reclaim does exactly this. It is recorded and
/// excluded rather than rendered into SQL that cannot load.
#[test]
fn a_published_path_that_vanished_is_excluded_with_a_warning() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let present = csv_at(root, "H/ds.csv", "A\n1\n");
    let hosts = vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![
                PublishedFile {
                    path: present,
                    format: OutputFormat::Csv,
                    dataset: DatasetKey::Static("ds"),
                    identity: Identity::System,
                },
                PublishedFile {
                    path: root.join("H/gone.csv"),
                    format: OutputFormat::Csv,
                    dataset: DatasetKey::Static("ds"),
                    identity: Identity::System,
                },
            ],
            merged: Vec::new(),
            column_types: &[],
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }];
    let inv = build(BuildRequest {
        run_id: "R",
        generation: "R-1",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });
    let gone = inv.datasets[0]
        .files
        .iter()
        .find(|f| f.path.ends_with("gone.csv"))
        .expect("gone entry");
    assert!(!gone.included_in_view);
    assert!(gone.header.is_none());
    assert!(!gone.warnings.is_empty());
}

/// Blank out every single-quoted SQL string literal, keeping newlines so
/// line numbering survives. A newline inside a literal is ordinary data; a
/// newline inside a `--` comment is an escape, and only the second is what
/// these tests are about.
fn without_string_literals(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut in_literal = false;
    for c in sql.chars() {
        match (in_literal, c) {
            (false, '\'') => in_literal = true,
            (false, _) => out.push(c),
            (true, '\'') => in_literal = false,
            (true, '\n') => out.push('\n'),
            (true, _) => {}
        }
    }
    out
}

/// The injected statement may appear only on a line that is still a comment.
/// If it reaches a line the parser would execute, the `--` escape worked.
fn assert_injection_never_escapes_a_comment(sql: &str) {
    let stripped = without_string_literals(sql);
    let mut seen = false;
    for line in stripped.lines() {
        if !line.contains("DROP TABLE") {
            continue;
        }
        seen = true;
        assert!(
            line.trim_start().starts_with("--"),
            "injected statement reached executable SQL: {line:?}\n{sql}"
        );
    }
    assert!(
        seen,
        "the payload never made it into the SQL at all:\n{sql}"
    );
}

/// A dataset id is runtime data -- `DatasetKey::Dynamic` carries a basename
/// derived from evidence content -- so a newline in one must not terminate
/// the `--` comment that names it.
#[test]
fn a_dataset_id_with_a_newline_cannot_escape_its_comment() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let file = csv_at(root, "H/evil.csv", "A\n1\n");
    let hosts = vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![PublishedFile {
                path: file,
                format: OutputFormat::Csv,
                dataset: DatasetKey::Dynamic("ds\nDROP TABLE x;--".into()),
                identity: Identity::System,
            }],
            merged: Vec::new(),
            column_types: &[],
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }];
    let inv = build(BuildRequest {
        run_id: "R",
        generation: "R-1",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });
    let sql = render_sql(&inv);
    assert!(sql.contains("-- T / ds DROP TABLE x;--"), "{sql}");
    assert_injection_never_escapes_a_comment(&sql);
}

/// `out_root` is the operator's `--out` path and reaches the header comment
/// verbatim. A newline in it must not end that comment either.
#[test]
fn an_out_root_with_a_newline_cannot_escape_its_comment() {
    let mut inv = one_dataset_inventory();
    inv.out_root = PathBuf::from("/tmp/o\nDROP TABLE x;--");
    let sql = render_sql(&inv);
    assert!(
        sql.contains("-- out_root:   /tmp/o DROP TABLE x;--"),
        "{sql}"
    );
    assert_injection_never_escapes_a_comment(&sql);
}

/// A tool's declared types are a compile-time constant that every host
/// repeats. Recording them once per published file instead of once per
/// dataset would log the same dropped override twice and allocate a second
/// `__text` companion that shadows the first.
#[test]
fn a_declaration_repeated_across_hosts_is_recorded_once() {
    const TYPES: &[DatasetColumnTypes] = &[DatasetColumnTypes {
        dataset_id: "ds",
        columns: &[
            ColumnType {
                column: "Present",
                sql_type: SqlType::BigInt,
                time_semantics: None,
            },
            ColumnType {
                column: "Absent",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
        ],
    }];

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let hosts: Vec<HostOutputs> = ["H1", "H2"]
        .iter()
        .map(|host| {
            let file = csv_at(root, &format!("{host}/ds.csv"), "Present\n1\n");
            HostOutputs {
                host: (*host).into(),
                tools: vec![ToolOutputs {
                    binary_name: "T".into(),
                    published: vec![PublishedFile {
                        path: file,
                        format: OutputFormat::Csv,
                        dataset: DatasetKey::Static("ds"),
                        identity: Identity::System,
                    }],
                    merged: Vec::new(),
                    column_types: TYPES,
                    dynamic_column_types: &[],
                }],
                external: Vec::new(),
            }
        })
        .collect();
    let inv = build(BuildRequest {
        run_id: "R",
        generation: "R-1",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });

    assert_eq!(inv.dropped_overrides.len(), 1);
    assert_eq!(
        inv.datasets[0].effective_types["Present"].text_column,
        "Present__text"
    );
}

/// The Velo category file is both a merge output and, for some tools, a
/// published file in its own right. Staging it twice would put two scans of
/// one path in one view and count every row in it twice.
#[test]
fn a_path_that_is_both_published_and_merged_is_staged_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let merged = csv_at(root, "H/cat/ds.csv", "A,TriageUser\n1,jdoe\n");
    let slice = csv_at(root, "H/cat/PerUser/ds_jdoe.csv", "A\n1\n");

    let hosts = vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![
                PublishedFile {
                    path: slice.clone(),
                    format: OutputFormat::Csv,
                    dataset: DatasetKey::Static("ds"),
                    identity: Identity::User("jdoe".into()),
                },
                // The same path the merge names as its output.
                PublishedFile {
                    path: merged.clone(),
                    format: OutputFormat::Csv,
                    dataset: DatasetKey::Static("ds"),
                    identity: Identity::System,
                },
            ],
            merged: vec![MergedFile {
                merged_path: merged.clone(),
                source_paths: vec![slice.clone()],
                format: OutputFormat::Csv,
            }],
            column_types: &[],
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }];
    let inv = build(BuildRequest {
        run_id: "R",
        generation: "R-1",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });

    let entries: Vec<&DatasetFile> = inv
        .datasets
        .iter()
        .flat_map(|d| d.files.iter())
        .filter(|f| f.path == std::path::Path::new("H/cat/ds.csv"))
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(entries[0].included_in_view);
    assert_eq!(entries[0].role, FileRole::Merged);
    assert!(entries[0].identity.is_none(), "a merged file spans users");
    let included = inv
        .datasets
        .iter()
        .flat_map(|d| d.files.iter())
        .filter(|f| f.included_in_view)
        .count();
    assert_eq!(included, 1);
}

/// Two merge records naming the same output are two records of one file.
#[test]
fn a_merged_path_recorded_twice_is_staged_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let merged = csv_at(root, "H/cat/ds.csv", "A,TriageUser\n1,jdoe\n");
    let one = csv_at(root, "H/cat/PerUser/ds_jdoe.csv", "A\n1\n");
    let two = csv_at(root, "H/cat/PerUser/ds_asmith.csv", "A\n2\n");

    let hosts = vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![
                PublishedFile {
                    path: one.clone(),
                    format: OutputFormat::Csv,
                    dataset: DatasetKey::Static("ds"),
                    identity: Identity::User("jdoe".into()),
                },
                PublishedFile {
                    path: two.clone(),
                    format: OutputFormat::Csv,
                    dataset: DatasetKey::Static("ds"),
                    identity: Identity::User("asmith".into()),
                },
            ],
            merged: vec![
                MergedFile {
                    merged_path: merged.clone(),
                    source_paths: vec![one],
                    format: OutputFormat::Csv,
                },
                MergedFile {
                    merged_path: merged.clone(),
                    source_paths: vec![two],
                    format: OutputFormat::Csv,
                },
            ],
            column_types: &[],
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }];
    let inv = build(BuildRequest {
        run_id: "R",
        generation: "R-1",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });

    let entries = inv.datasets[0]
        .files
        .iter()
        .filter(|f| f.path == std::path::Path::new("H/cat/ds.csv"))
        .count();
    assert_eq!(entries, 1);
    let included = inv.datasets[0]
        .files
        .iter()
        .filter(|f| f.included_in_view)
        .count();
    assert_eq!(included, 1);
}

/// The Velo reclaim can rename a merged file out from under the run. Its
/// slices stay excluded -- restoring them would double every row the moment
/// the merged file reappeared -- so the dataset contributes nothing, and
/// that has to be said out loud rather than left as an empty view.
#[test]
fn a_dataset_whose_merged_file_vanished_is_warned_about() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let merged = root.join("H/cat/ds.csv"); // never written
    let slice = csv_at(root, "H/cat/PerUser/ds_jdoe.csv", "A\n1\n");

    let hosts = vec![HostOutputs {
        host: "H".into(),
        tools: vec![ToolOutputs {
            binary_name: "T".into(),
            published: vec![PublishedFile {
                path: slice.clone(),
                format: OutputFormat::Csv,
                dataset: DatasetKey::Static("ds"),
                identity: Identity::User("jdoe".into()),
            }],
            merged: vec![MergedFile {
                merged_path: merged,
                source_paths: vec![slice],
                format: OutputFormat::Csv,
            }],
            column_types: &[],
            dynamic_column_types: &[],
        }],
        external: Vec::new(),
    }];
    let inv = build(BuildRequest {
        run_id: "R",
        generation: "R-1",
        generated_utc: "2026-09-17T12:00:00.0000000Z",
        out_root: root,
        hosts: &hosts,
    });

    let dataset = &inv.datasets[0];
    assert!(!dataset.files.is_empty());
    assert!(
        !dataset.files.iter().any(|f| f.included_in_view),
        "{:?}",
        dataset.files
    );
    assert_eq!(inv.warnings.len(), 1, "{:?}", inv.warnings);
    assert!(inv.warnings[0].contains("T / ds"), "{:?}", inv.warnings);
    // The run is not what failed here; one dataset is.
    assert_eq!(inv.status, Status::Ok);
}
