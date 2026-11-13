//! The app state with no window: reads under slots and query ids, the one
//! writer, the events, and the case and load commands over them.
//!
//! The timing tests hold `common::serial()` so that no two compete for the
//! machine, and each runs under `common::within`, so a hang fails.

mod common;

use common::{calibrate, counting, exec, loaded, serial, state, wait_running, within, ENDLESS};
use oxl_app::cmd;
use oxl_app::events::{CASE_CHANGED, JOB_FINISHED, JOB_STARTED};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(60);

#[test]
fn a_new_query_supersedes_its_slot() {
    let _serial = serial();
    let l = loaded();
    let state = l.state.clone();
    within(DEADLINE, move || {
        let first = {
            let state = state.clone();
            std::thread::spawn(move || state.read("t1:page", "q1", |c| c.scalar_i64(ENDLESS)))
        };
        wait_running(&state, "q1", Duration::from_secs(5));
        let start = Instant::now();
        let second = state.read("t1:page", "q2", |c| c.scalar_i64("SELECT 42"));
        assert_eq!(second, Ok(42));
        let first = first.join().unwrap();
        assert_eq!(first.unwrap_err().code, "superseded");
        let took = start.elapsed();
        assert!(took < Duration::from_secs(2), "superseded after {took:?}");
        assert!(state.queries.running().is_empty());
    });
}

#[test]
fn different_slots_run_together() {
    let _serial = serial();
    let l = loaded();
    let state = l.state.clone();
    within(DEADLINE, move || {
        let n = calibrate(&state, Duration::from_millis(500));
        // Alone, each query takes about 500 ms.
        let alone = Instant::now();
        state
            .read("t1:page", "alone", |c| c.scalar_i64(&counting(n)))
            .unwrap();
        let alone = alone.elapsed();
        assert!(alone >= Duration::from_millis(300), "alone took {alone:?}");

        let barrier = Arc::new(std::sync::Barrier::new(3));
        let start = Instant::now();
        let runs: Vec<_> = [("t1:page", "p1"), ("t1:hist", "h1")]
            .into_iter()
            .map(|(slot, id)| {
                let state = state.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    state.read(slot, id, |c| c.scalar_i64(&counting(n)))
                })
            })
            .collect();
        barrier.wait();
        for run in runs {
            assert!(run.join().unwrap().is_ok());
        }
        let both = start.elapsed();
        assert!(
            both < Duration::from_millis(900),
            "both took {both:?} (alone {alone:?})"
        );
    });
}

#[test]
fn ten_rapid_queries_leave_only_the_last() {
    let _serial = serial();
    let l = loaded();
    let state = l.state.clone();
    within(DEADLINE, move || {
        let n = calibrate(&state, Duration::from_millis(500));
        let mut runs = Vec::new();
        for k in 0..10 {
            let id = format!("q{k}");
            let run = {
                let state = state.clone();
                let id = id.clone();
                std::thread::spawn(move || {
                    state.read("t1:page", &id, |c| c.scalar_i64(&counting(n)))
                })
            };
            wait_running(&state, &id, Duration::from_secs(5));
            runs.push(run);
            if k < 9 {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let codes: Vec<String> = runs
            .into_iter()
            .map(|r| match r.join().unwrap() {
                Ok(_) => "ok".to_string(),
                Err(e) => e.code,
            })
            .collect();
        let mut expected = vec!["superseded".to_string(); 9];
        expected.push("ok".to_string());
        assert_eq!(codes, expected);
    });
}

#[test]
fn cancel_before_start_never_runs() {
    let _serial = serial();
    let l = loaded();
    let state = l.state.clone();
    within(DEADLINE, move || {
        state.queries.cancel(&["q9".to_string()]);
        let ran = AtomicBool::new(false);
        let answer = state.read("t1:page", "q9", |c| {
            ran.store(true, Ordering::SeqCst);
            c.scalar_i64("SELECT 1")
        });
        assert_eq!(answer.unwrap_err().code, "interrupted");
        assert!(!ran.load(Ordering::SeqCst), "the cancelled query ran");
        // The cancel is spent: the same id asked again runs.
        assert_eq!(
            state.read("t1:page", "q9", |c| c.scalar_i64("SELECT 1")),
            Ok(1)
        );
    });
}

#[test]
fn cancel_interrupts_a_running_query() {
    let _serial = serial();
    let l = loaded();
    let state = l.state.clone();
    within(DEADLINE, move || {
        let run = {
            let state = state.clone();
            std::thread::spawn(move || state.read("t1:page", "q1", |c| c.scalar_i64(ENDLESS)))
        };
        wait_running(&state, "q1", Duration::from_secs(5));
        let start = Instant::now();
        cmd::query_cancel(&state, &["q1".to_string()]).unwrap();
        assert_eq!(run.join().unwrap().unwrap_err().code, "interrupted");
        assert!(start.elapsed() < Duration::from_secs(2));
        // The reader it ran on serves the next query.
        assert_eq!(
            state.read("t1:page", "q2", |c| c.scalar_i64("SELECT 7")),
            Ok(7)
        );
    });
}

#[test]
fn a_second_write_is_busy() {
    let _serial = serial();
    let l = loaded();
    let (state, events) = (l.state.clone(), l.events.clone());
    let run = l.run.clone();
    within(DEADLINE, move || {
        let (started_tx, started) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let first = {
            let state = state.clone();
            std::thread::spawn(move || {
                state.write(&["marks"], |_case| {
                    started_tx.send(()).unwrap();
                    released.recv().unwrap();
                    Ok(1)
                })
            })
        };
        started.recv().unwrap();
        let second = state.write(&["marks"], |_case| Ok(2));
        assert_eq!(second.unwrap_err().code, "busy");
        // A load is a write too, and announces no job it cannot start.
        assert_eq!(cmd::load::load_run(&state, &run).unwrap_err().code, "busy");
        assert!(events.named(JOB_STARTED).is_empty());
        // Opening or closing a case waits for no write either.
        assert_eq!(cmd::case::case_close(&state).unwrap_err().code, "busy");
        release.send(()).unwrap();
        assert_eq!(first.join().unwrap(), Ok(1));
        assert_eq!(state.write(&["marks"], |_case| Ok(3)), Ok(3));
    });
}

#[test]
fn writes_emit_case_changed() {
    let _serial = serial();
    let l = loaded();
    let (state, events) = (l.state.clone(), l.events.clone());
    state.write(&["marks"], |_case| Ok(())).unwrap();
    state.write(&["marks", "tags"], |_case| Ok(())).unwrap();
    // A failed write changes nothing and says nothing.
    let failed = state.write(&["marks"], |_case| -> oxl_engine::Result<()> {
        Err(oxl_engine::EngineError::Invalid("no".into()))
    });
    assert_eq!(failed.unwrap_err().code, "invalid");
    let changed = events.named(CASE_CHANGED);
    assert_eq!(changed.len(), 2, "{changed:?}");
    assert_eq!(changed[0]["kinds"], json!(["marks"]));
    assert_eq!(changed[1]["kinds"], json!(["marks", "tags"]));
    let g0 = changed[0]["generation"].as_u64().unwrap();
    let g1 = changed[1]["generation"].as_u64().unwrap();
    assert_eq!(g1, g0 + 1);
    assert_eq!(state.generation(), g1);
}

#[test]
fn reads_continue_during_a_write() {
    let _serial = serial();
    let l = loaded();
    let state = l.state.clone();
    within(DEADLINE, move || {
        let (open_tx, open) = mpsc::channel();
        let done = Arc::new(AtomicBool::new(false));
        let write = {
            let state = state.clone();
            let done = done.clone();
            std::thread::spawn(move || {
                state.write(&["meta"], |case| {
                    exec(case, "BEGIN TRANSACTION")?;
                    exec(
                        case,
                        "UPDATE ox.meta SET value = value WHERE key = 'name' RETURNING 1",
                    )?;
                    open_tx.send(()).unwrap();
                    std::thread::sleep(Duration::from_secs(1));
                    done.store(true, Ordering::SeqCst);
                    exec(case, "COMMIT")
                })
            })
        };
        open.recv().unwrap();
        let start = Instant::now();
        let grid: oxl_engine::GridState =
            serde_json::from_value(json!({"relation": "evtxtriage_events"})).unwrap();
        let options: oxl_engine::PageOptions = serde_json::from_value(json!({})).unwrap();
        let page = state
            .read("t1:page", "p1", |c| {
                oxl_engine::page_view(c, &grid, 5, 0, &options)
            })
            .unwrap();
        let took = start.elapsed();
        assert!(
            !done.load(Ordering::SeqCst),
            "the read waited for the write"
        );
        assert!(took < Duration::from_millis(900), "the read took {took:?}");
        assert_eq!(page.row_ids.len(), 5);
        assert_eq!(write.join().unwrap(), Ok(()));
    });
}

#[test]
fn no_case_is_its_own_code() {
    let (events, state) = state();
    let read = state.read("t1:page", "q1", |c| c.scalar_i64("SELECT 1"));
    assert_eq!(read.unwrap_err().code, "no_case");
    let write = state.write(&["marks"], |_case| Ok(()));
    assert_eq!(write.unwrap_err().code, "no_case");
    let d = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::two_hosts(d.path());
    assert_eq!(
        cmd::load::load_run(&state, &run).unwrap_err().code,
        "no_case"
    );
    // Closing when nothing is open is not an error.
    assert_eq!(cmd::case::case_close(&state), Ok(()));
    assert!(events.events().is_empty(), "{:?}", events.events());
}

#[test]
fn a_load_is_a_job() {
    let _serial = serial();
    let d = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::two_hosts(d.path());
    let (events, state) = state();
    cmd::case::case_new(&state, &d.path().join("case.duckdb"), "WS01 triage").unwrap();
    let report = cmd::load::load_run(&state, &run).unwrap();
    assert!(!report.relations.is_empty());
    let names = events.names();
    assert_eq!(
        names,
        vec![JOB_STARTED, JOB_FINISHED, CASE_CHANGED],
        "{names:?}"
    );
    let label = run.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(
        events.named(JOB_STARTED)[0],
        json!({"kind": "load_run", "label": label})
    );
    assert_eq!(
        events.named(JOB_FINISHED)[0],
        json!({"kind": "load_run", "ok": true, "message": null})
    );
    assert_eq!(events.named(CASE_CHANGED)[0]["kinds"], json!(["sources"]));

    // A load that fails is a job that failed, and changes nothing.
    events.clear();
    let missing = d.path().join("missing.csv");
    let failed = cmd::load::load_file(&state, &missing, Default::default());
    assert!(failed.is_err());
    let names = events.names();
    assert_eq!(names, vec![JOB_STARTED, JOB_FINISHED], "{names:?}");
    let finished = &events.named(JOB_FINISHED)[0];
    assert_eq!(finished["kind"], json!("load_file"));
    assert_eq!(finished["ok"], json!(false));
    assert_eq!(finished["message"], json!(failed.unwrap_err().message));
}

#[test]
fn opening_a_case_adds_it_to_recent() {
    let _serial = serial();
    let l = loaded();
    let state = l.state.clone();
    let path = l.case.to_string_lossy().into_owned();
    // `case_new` opened it, so it is already first.
    assert_eq!(
        cmd::settings::settings_get(&state).unwrap().settings.recent,
        vec![path.clone()]
    );

    cmd::case::case_close(&state).unwrap();
    assert_eq!(
        state
            .read("t1:page", "q1", |c| c.scalar_i64("SELECT 1"))
            .unwrap_err()
            .code,
        "no_case"
    );
    // Closed means released: the file opens again in this process.
    let summary = cmd::case::case_open(&state, &l.case).unwrap();
    assert_eq!(summary.name, "WS01 triage");
    assert_eq!(summary.format, oxl_engine::CASE_FORMAT);
    assert_eq!(summary.case_id.len(), 32);
    assert_eq!(summary.display_zone, "UTC");
    assert_eq!(summary.hosts, vec!["SRV01".to_string(), "WS01".to_string()]);
    assert!(summary
        .relations
        .iter()
        .any(|r| r.name == "evtxtriage_events"));
    assert_eq!(summary.sources.len(), 1);
    assert_eq!(
        cmd::settings::settings_get(&state).unwrap().settings.recent,
        vec![path]
    );
    assert_eq!(
        state.read("t1:page", "q1", |c| c.scalar_i64("SELECT 1")),
        Ok(1)
    );
}

#[test]
fn settings_update_checks_and_saves() {
    let d = tempfile::tempdir().unwrap();
    let (store, damaged) = oxl_app::settings::SettingsStore::load(d.path());
    assert_eq!(damaged, None);
    let state = oxl_app::state::AppState::new(store, Arc::new(common::Recorder::default()));
    let mut s = cmd::settings::settings_get(&state).unwrap().settings;
    s.font_size = 15;
    s.analyst = Some("jdoe".into());
    let saved = cmd::settings::settings_update(&state, s.clone()).unwrap();
    assert_eq!(saved.settings, s);
    assert_eq!(state.actor(), oxl_engine::Actor::Analyst("jdoe".into()));
    let (reread, _) = oxl_app::settings::SettingsStore::load(d.path());
    assert_eq!(reread.get(), s);
    s.font_size = 30;
    let refused = cmd::settings::settings_update(&state, s).unwrap_err();
    assert_eq!(refused.code, "invalid");
    assert_eq!(
        cmd::settings::settings_get(&state)
            .unwrap()
            .settings
            .font_size,
        15
    );
}

#[test]
fn a_damaged_settings_file_is_announced_once() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("settings.json"), b"{damaged").unwrap();
    let recorder = Arc::new(common::Recorder::default());
    let state = oxl_app::state::AppState::with_settings_dir(d.path(), recorder.clone());
    let damaged = recorder.named(oxl_app::events::SETTINGS_DAMAGED);
    assert_eq!(damaged.len(), 1, "{:?}", recorder.events());
    let moved = damaged[0]["file"].as_str().unwrap().to_string();
    assert!(moved.starts_with("settings.json.damaged-"), "{moved}");
    assert!(d.path().join(&moved).is_file());
    let reply = cmd::settings::settings_get(&state).unwrap();
    assert_eq!(reply.settings, oxl_app::settings::Settings::default());
    assert_eq!(serde_json::to_value(&reply.damaged).unwrap(), damaged[0]);
}

#[test]
fn a_damaged_settings_file_is_reported_until_the_next_save() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("settings.json"), b"{damaged").unwrap();
    // Nothing listens when the app starts: the window asks later.
    let state =
        oxl_app::state::AppState::with_settings_dir(d.path(), Arc::new(oxl_app::events::NoSink));
    for _ in 0..2 {
        let reply = cmd::settings::settings_get(&state).unwrap();
        let damaged = reply.damaged.expect("the damaged file is reported");
        let file = damaged.file.expect("it was moved aside");
        assert!(file.starts_with("settings.json.damaged-"), "{file}");
        assert!(d.path().join(&file).is_file());
        assert!(!damaged.reason.is_empty());
        assert!(!damaged.reason.contains("{damaged"), "{}", damaged.reason);
    }
    // A refused update reports it still.
    let mut s = oxl_app::settings::Settings {
        font_size: 99,
        ..Default::default()
    };
    assert!(cmd::settings::settings_update(&state, s.clone()).is_err());
    assert!(cmd::settings::settings_get(&state)
        .unwrap()
        .damaged
        .is_some());
    // A successful one ends the report.
    s.font_size = 13;
    let saved = cmd::settings::settings_update(&state, s).unwrap();
    assert_eq!(saved.damaged, None);
    assert_eq!(cmd::settings::settings_get(&state).unwrap().damaged, None);
}

#[test]
fn a_request_waiting_for_a_reader_keeps_its_order() {
    let _serial = serial();
    let l = loaded();
    let state = l.state.clone();
    within(DEADLINE, move || {
        // Every reader busy on a slot of its own.
        let busy: Vec<String> = (0..oxl_app::state::READERS)
            .map(|k| format!("busy{k}"))
            .collect();
        let holders: Vec<_> = busy
            .iter()
            .map(|id| {
                let (state, id) = (state.clone(), id.clone());
                std::thread::spawn(move || {
                    let slot = format!("t{id}:page");
                    state.read(&slot, &id, |c| c.scalar_i64(ENDLESS))
                })
            })
            .collect();
        for id in &busy {
            wait_running(&state, id, Duration::from_secs(5));
        }
        // A, then B, on one slot, both waiting for a reader.
        let ran_a = Arc::new(AtomicBool::new(false));
        let a = {
            let (state, ran_a) = (state.clone(), ran_a.clone());
            std::thread::spawn(move || {
                state.read("t1:page", "A", |c| {
                    ran_a.store(true, Ordering::SeqCst);
                    c.scalar_i64("SELECT 1")
                })
            })
        };
        std::thread::sleep(Duration::from_millis(200));
        let b = {
            let state = state.clone();
            std::thread::spawn(move || state.read("t1:page", "B", |c| c.scalar_i64("SELECT 2")))
        };
        std::thread::sleep(Duration::from_millis(200));
        // Free the pool.
        cmd::query_cancel(&state, &busy).unwrap();
        for holder in holders {
            assert_eq!(holder.join().unwrap().unwrap_err().code, "interrupted");
        }
        assert_eq!(b.join().unwrap(), Ok(2));
        assert_eq!(a.join().unwrap().unwrap_err().code, "superseded");
        assert!(!ran_a.load(Ordering::SeqCst), "the older request ran");
    });
}

#[test]
fn query_cancel_refuses_bad_ids() {
    let (_events, state) = state();
    assert_eq!(
        cmd::query_cancel(&state, &["".to_string()])
            .unwrap_err()
            .code,
        "invalid"
    );
    let many: Vec<String> = (0..300).map(|n| format!("q{n}")).collect();
    assert_eq!(
        cmd::query_cancel(&state, &many).unwrap_err().code,
        "invalid"
    );
}
