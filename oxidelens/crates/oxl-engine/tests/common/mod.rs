//! Helpers shared by the engine's integration tests.

// Each test file compiles this module on its own and uses only some of it.
#![allow(dead_code)]

use oxl_engine::{load_run, Actor, Case};

pub fn analyst() -> Actor {
    Actor::Analyst("tester".into())
}

pub fn agent() -> Actor {
    Actor::Agent("claude".into())
}

/// A new case with the two-host run loaded. Keep the directory alive for as
/// long as the case: it holds both the case file and the run.
pub fn loaded() -> (tempfile::TempDir, Case) {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(d.path());
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    (d, case)
}

/// The first column of the first row of `sql`, as an integer.
pub fn n(case: &Case, sql: &str) -> i64 {
    case.scalar_i64(sql)
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}
