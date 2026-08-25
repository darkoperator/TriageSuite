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
    assert_eq!(router.finish().unwrap(), 2);
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
    assert!(router.finish().is_err());
    assert!(!temp.path().join("csv/Runtime_system.csv").exists());
    assert_eq!(
        std::fs::read_dir(temp.path().join("csv")).unwrap().count(),
        0
    );
}
