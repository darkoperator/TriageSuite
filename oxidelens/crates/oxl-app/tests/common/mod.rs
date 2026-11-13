//! Helpers shared by the app's integration tests: an app state with no
//! window, an emitter that records every event, and a case with the
//! two-host run loaded through the app's own commands.

// Each test file compiles this module on its own and uses only some of it.
#![allow(dead_code)]

use oxl_app::events::EventSink;
use oxl_app::settings::SettingsStore;
use oxl_app::state::AppState;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Records every event the app sends, in order.
#[derive(Default)]
pub struct Recorder {
    events: Mutex<Vec<(String, Value)>>,
}

impl EventSink for Recorder {
    fn emit(&self, event: &str, payload: Value) {
        self.events
            .lock()
            .unwrap()
            .push((event.to_string(), payload));
    }
}

impl Recorder {
    pub fn events(&self) -> Vec<(String, Value)> {
        self.events.lock().unwrap().clone()
    }

    /// The payloads of every `event`, in order.
    pub fn named(&self, event: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|(name, _)| name == event)
            .map(|(_, payload)| payload)
            .collect()
    }

    /// The names of every event, in order.
    pub fn names(&self) -> Vec<String> {
        self.events().into_iter().map(|(name, _)| name).collect()
    }

    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
}

/// An app state with settings in memory and a recorder for its events.
pub fn state() -> (Arc<Recorder>, Arc<AppState>) {
    let recorder = Arc::new(Recorder::default());
    let state = AppState::new(SettingsStore::in_memory(), recorder.clone());
    (recorder, Arc::new(state))
}

/// A case at `<dir>/case.duckdb` with the two-host run loaded through the
/// app's commands, open in a new app state. Keep the directory alive as
/// long as the state.
pub struct Loaded {
    pub dir: tempfile::TempDir,
    pub case: PathBuf,
    pub run: PathBuf,
    pub events: Arc<Recorder>,
    pub state: Arc<AppState>,
}

pub fn loaded() -> Loaded {
    let dir = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::two_hosts(dir.path());
    let case = dir.path().join("case.duckdb");
    let (events, state) = state();
    oxl_app::cmd::case::case_new(&state, &case, "WS01 triage").unwrap();
    oxl_app::cmd::load::load_run(&state, &run).unwrap();
    events.clear();
    Loaded {
        dir,
        case,
        run,
        events,
        state,
    }
}

/// The lock every timing test holds, so that no two of them compete for
/// the machine within this test binary.
pub fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

/// Run `f` on a thread of its own and fail the test if it has not returned
/// within `limit`, so a hang fails instead of blocking the suite.
pub fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(limit) {
        Ok(value) => {
            let _ = worker.join();
            value
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("the test did not finish within {limit:?}")
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            // The worker panicked; surface its panic.
            match worker.join() {
                Err(panic) => std::panic::resume_unwind(panic),
                Ok(()) => panic!("the test's worker ended without an answer"),
            }
        }
    }
}

/// Wait until query `id` is running, or fail after `limit`.
pub fn wait_running(state: &AppState, id: &str, limit: Duration) {
    let start = Instant::now();
    while !state.queries.running().iter().any(|q| q == id) {
        assert!(
            start.elapsed() < limit,
            "query {id} did not start within {limit:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Run `sql` for its effect: a statement that returns no row is fine.
pub fn exec(case: &oxl_engine::Case, sql: &str) -> oxl_engine::Result<()> {
    match case.scalar_i64(sql) {
        Ok(_) => Ok(()),
        Err(oxl_engine::EngineError::Db(duckdb::Error::QueryReturnedNoRows)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// A query that takes far longer than any test waits: it must be stopped.
pub const ENDLESS: &str = "SELECT count(*) FROM range(10000000000) WHERE range % 7 = 3";

/// A query over `n` rows.
pub fn counting(n: u64) -> String {
    format!("SELECT count(*) FROM range({n}) WHERE range % 7 = 3")
}

/// The row count for which `counting` takes about `target` on this machine,
/// alone, measured through the app's own read path.
pub fn calibrate(state: &AppState, target: Duration) -> u64 {
    let mut n: u64 = 10_000_000;
    loop {
        let start = Instant::now();
        state
            .read("calibrate", "calibrate", |c| c.scalar_i64(&counting(n)))
            .unwrap();
        let took = start.elapsed();
        if took >= Duration::from_millis(150) || n >= 50_000_000_000 {
            let scale = target.as_secs_f64() / took.as_secs_f64().max(0.001);
            return ((n as f64) * scale) as u64;
        }
        n *= 4;
    }
}
