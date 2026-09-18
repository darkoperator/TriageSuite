use serde::Serialize;
use triage_core::attribution::Identity;
use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::layout::OutputLayoutMode;
use triage_core::output::router::{OutputRouter, RouterOptions};

const DATASETS: &[DatasetSpec] = &[DatasetSpec {
    id: "main",
    default_basename: "Reliability_Output",
    framing: JsonFraming::Ndjson,
    csv_only: false,
    override_suffix: None,
}];

#[derive(Serialize)]
struct Row {
    #[serde(rename = "Name")]
    name: &'static str,
}

fn options(root: &std::path::Path) -> RouterOptions {
    RouterOptions {
        csv_root: Some(root.join("csv")),
        json_root: Some(root.join("json")),
        csvf: None,
        jsonf: None,
        pretty: false,
        overwrite: false,
        run_stamp: None,
        layout_mode: OutputLayoutMode::Flat,
    }
}

fn overwriting_options(root: &std::path::Path) -> RouterOptions {
    RouterOptions {
        overwrite: true,
        ..options(root)
    }
}

#[test]
fn static_and_dynamic_outputs_publish_only_on_finish() {
    let temp = tempfile::tempdir().unwrap();
    let mut router = OutputRouter::new("Reliability", DATASETS, options(temp.path())).unwrap();
    router.set_identity(Identity::System);
    router.write("main", &Row { name: "one" }).unwrap();
    router
        .write_dynamic_row("Runtime", &["A".into()], &["two".into()])
        .unwrap();

    assert!(!temp
        .path()
        .join("csv/Reliability_Output_system.csv")
        .exists());
    assert!(!temp.path().join("csv/Runtime_system.csv").exists());
    assert_eq!(router.finish().into_outcome().unwrap(), 2);
    assert!(temp
        .path()
        .join("csv/Reliability_Output_system.csv")
        .exists());
    assert!(temp.path().join("csv/Runtime_system.csv").exists());
}

#[test]
fn schema_failure_aborts_publish_and_cleans_temporary_files() {
    let temp = tempfile::tempdir().unwrap();
    let mut router = OutputRouter::new("Reliability", &[], options(temp.path())).unwrap();
    router.set_identity(Identity::System);
    router
        .write_dynamic_row("Runtime", &["A".into()], &["one".into()])
        .unwrap();
    assert!(matches!(
        router.write_dynamic_row("Runtime", &["B".into()], &["two".into()]),
        Err(TriageError::Output { .. })
    ));
    assert!(router.finish().into_outcome().is_err());
    assert!(!temp.path().join("csv/Runtime_system.csv").exists());
    assert_eq!(
        std::fs::read_dir(temp.path().join("csv")).unwrap().count(),
        0
    );
}

/// A `finish()` that aborts after an earlier write failure deletes every
/// staged file and publishes none of them, so it must report an **empty**
/// published set -- even where a destination it opened is occupied.
///
/// The occupant here is a previous run's file, which is why existence cannot
/// stand in for publication: `--overwrite` lets the router open a destination
/// that is already taken, and the file still sitting there afterwards belongs
/// to whoever wrote it last, not to this run. `triage_orchestrator`'s Velo
/// merge reclaims a category-root file only if the router published it, so a
/// published set padded out by an existence test hands the merge a previous
/// run's merged output to fold in as this run's system-scope slice.
#[test]
fn an_aborted_finish_publishes_nothing_even_where_a_previous_runs_file_exists() {
    let temp = tempfile::tempdir().unwrap();
    let stale = temp.path().join("csv/Reliability_Output_system.csv");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "Name\nprevious run\n").unwrap();

    let mut router =
        OutputRouter::new("Reliability", DATASETS, overwriting_options(temp.path())).unwrap();
    router.set_identity(Identity::System);
    // Opens both sinks for `main` -- including the destination the previous
    // run's file is sitting on, which `--overwrite` permits.
    router.write("main", &Row { name: "one" }).unwrap();
    // A second dynamic row with a different schema fails, which puts the
    // router into its aborted state.
    router
        .write_dynamic_row("Runtime", &["A".into()], &["one".into()])
        .unwrap();
    assert!(matches!(
        router.write_dynamic_row("Runtime", &["B".into()], &["two".into()]),
        Err(TriageError::Output { .. })
    ));

    let report = router.finish();
    assert!(
        report.outcome.is_err(),
        "an aborted transaction must still report failure"
    );
    assert_eq!(
        report.published_paths(),
        Vec::<std::path::PathBuf>::new(),
        "nothing was renamed into place, so nothing may be reported as published"
    );
    assert_eq!(
        std::fs::read_to_string(&stale).unwrap(),
        "Name\nprevious run\n",
        "the previous run's file must be left exactly as it was"
    );
}

/// The success path: every destination the router opened is published, and
/// `finish()` names each of them.
#[test]
fn a_successful_finish_reports_every_destination_it_published() {
    let temp = tempfile::tempdir().unwrap();
    let mut router = OutputRouter::new("Reliability", DATASETS, options(temp.path())).unwrap();
    router.set_identity(Identity::System);
    router.write("main", &Row { name: "one" }).unwrap();
    router
        .write_dynamic_row("Runtime", &["A".into()], &["two".into()])
        .unwrap();

    let report = router.finish();
    let mut expected = vec![
        temp.path().join("csv/Reliability_Output_system.csv"),
        temp.path().join("json/Reliability_Output_system.json"),
        temp.path().join("csv/Runtime_system.csv"),
        temp.path().join("json/Runtime_system.json"),
    ];
    expected.sort();
    assert_eq!(report.published_paths(), expected);
    for path in &expected {
        assert!(path.is_file(), "{path:?} was reported published");
    }
    // `published_paths()` borrows `report` as a whole, so it must run before
    // this `unwrap()` partially moves `report.outcome` out.
    assert_eq!(report.outcome.unwrap(), 2);
}

/// The router is the only thing that knows which dataset and which identity
/// a published path belongs to. Re-deriving it downstream by parsing the
/// filename would mean reimplementing `dataset_filename` and `velo_basename`
/// and drifting from them silently.
///
/// Both write paths are covered: a static dataset carries its `DatasetSpec`
/// id, a dynamic one carries the runtime basename, and each file names the
/// identity that was current when it was opened.
#[test]
fn every_published_file_names_its_dataset_and_identity() {
    use triage_core::output::published::{DatasetKey, OutputFormat};

    let temp = tempfile::tempdir().unwrap();
    let mut router = OutputRouter::new("Reliability", DATASETS, options(temp.path())).unwrap();
    router.set_identity(Identity::System);
    router.write("main", &Row { name: "one" }).unwrap();
    router
        .write_dynamic_row("Runtime", &["A".into()], &["two".into()])
        .unwrap();

    let report = router.finish();

    let csv_main = report
        .published
        .iter()
        .find(|f| f.path.ends_with("Reliability_Output_system.csv"))
        .expect("the static dataset's CSV must be published");
    assert_eq!(csv_main.dataset, DatasetKey::Static("main"));
    assert_eq!(csv_main.identity, Identity::System);
    assert_eq!(csv_main.format, OutputFormat::Csv);

    let json_dynamic = report
        .published
        .iter()
        .find(|f| f.path.ends_with("Runtime_system.json"))
        .expect("the dynamic dataset's JSON must be published");
    assert_eq!(json_dynamic.dataset, DatasetKey::Dynamic("Runtime".into()));
    assert_eq!(json_dynamic.format, OutputFormat::Json);

    // The helper is exactly the old field, so the manifest keeps reporting
    // the same paths in the same order.
    let mut expected = vec![
        temp.path().join("csv/Reliability_Output_system.csv"),
        temp.path().join("json/Reliability_Output_system.json"),
        temp.path().join("csv/Runtime_system.csv"),
        temp.path().join("json/Runtime_system.json"),
    ];
    expected.sort();
    assert_eq!(report.published_paths(), expected);
    // `published_paths()` borrows `report` as a whole, so it must run before
    // this `unwrap()` partially moves `report.outcome` out.
    assert_eq!(report.outcome.unwrap(), 2);
}
