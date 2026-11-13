//! The open case and the rules every command goes through.
//!
//! **One writer.** The case is open on one read-write connection, the only
//! one that changes it. A write takes the write gate first; a write that
//! finds it taken answers `busy` rather than queueing behind a load. Every
//! successful write sends `case-changed`.
//!
//! **Readers.** `READERS` clones of the writer (`Case::try_clone`) are made
//! when the case opens. A read registers with the query registry first,
//! then checks a reader out (waiting while all are out, and giving up as
//! soon as it is superseded or cancelled), runs, and gives it back; so reads
//! run beside each other and beside a write, see what each write committed,
//! and keep the order they arrived in.
//!
//! **Closing** stops every registered query and lets none register until it
//! is done (`QueryRegistry::begin_close`), waits for every reader to come
//! back, and drops the readers before the writer, so that when `close`
//! returns no connection to the file is left and it can be opened again.
//! Quitting closes through `close_within`, which bounds the wait.

use crate::error::CmdError;
use crate::events::{
    send, CaseChanged, EventSink, JobFinished, JobStarted, CASE_CHANGED, JOB_FINISHED, JOB_STARTED,
    SETTINGS_DAMAGED,
};
use crate::queries::{QueryRegistry, Ticket};
use crate::settings::{app_actor, os_user, SettingsStore};
use oxl_engine::{Actor, Case};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::time::Duration;

/// Reader connections per open case.
pub const READERS: usize = 8;

/// How often a query waiting for a reader looks whether it was superseded
/// or cancelled meanwhile.
const WAIT_POLL: Duration = Duration::from_millis(25);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while a lock was held leaves nothing half-done that the next
    // holder could see: every guarded value is replaced whole.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

struct Pool {
    free: Vec<Case>,
    /// Readers checked out now.
    out: usize,
    /// Set by `close`: no reader is handed out again.
    closed: bool,
}

/// The reader connections of one open case.
pub struct ReaderPool {
    pool: Mutex<Pool>,
    /// Signalled when a reader comes back or the pool closes.
    changed: Condvar,
}

/// A reader checked out of the pool; it goes back when dropped.
pub struct Reader<'a> {
    pool: &'a ReaderPool,
    case: Option<Case>,
}

impl std::ops::Deref for Reader<'_> {
    type Target = Case;
    fn deref(&self) -> &Case {
        // Present from checkout until drop.
        self.case
            .as_ref()
            .expect("a checked-out reader holds its case")
    }
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        if let Some(case) = self.case.take() {
            let mut pool = lock(&self.pool.pool);
            pool.free.push(case);
            pool.out -= 1;
            self.pool.changed.notify_all();
        }
    }
}

impl ReaderPool {
    /// `READERS` clones of `writer`.
    fn open(writer: &Case) -> oxl_engine::Result<ReaderPool> {
        let free = (0..READERS)
            .map(|_| writer.try_clone())
            .collect::<oxl_engine::Result<Vec<Case>>>()?;
        Ok(ReaderPool {
            pool: Mutex::new(Pool {
                free,
                out: 0,
                closed: false,
            }),
            changed: Condvar::new(),
        })
    }

    /// A free reader for the query `ticket` registered, waiting while every
    /// one is out: `no_case` once the pool is closed, and the query's stop
    /// code as soon as it is superseded or cancelled while it waits.
    fn checkout(&self, ticket: &Ticket<'_>) -> Result<Reader<'_>, CmdError> {
        loop {
            // The registry's lock is never taken under the pool's.
            if let Some(stopped) = ticket.stopped() {
                return Err(stopped);
            }
            let pool = lock(&self.pool);
            let mut pool = if pool.closed || !pool.free.is_empty() {
                pool
            } else {
                self.changed
                    .wait_timeout(pool, WAIT_POLL)
                    .unwrap_or_else(|p| p.into_inner())
                    .0
            };
            if pool.closed {
                return Err(CmdError::no_case());
            }
            if let Some(case) = pool.free.pop() {
                pool.out += 1;
                return Ok(Reader {
                    pool: self,
                    case: Some(case),
                });
            }
        }
    }

    /// Hand out no more readers, wait for every one out to come back, and
    /// close them all.
    fn close(&self) {
        let mut pool = lock(&self.pool);
        pool.closed = true;
        self.changed.notify_all();
        while pool.out > 0 {
            pool = self.changed.wait(pool).unwrap_or_else(|p| p.into_inner());
        }
        let readers = std::mem::take(&mut pool.free);
        drop(pool);
        drop(readers);
    }
}

/// The case the window has open.
pub struct OpenCase {
    pub path: PathBuf,
    pub case_id: String,
    /// The app's generation when this case opened.
    pub generation: u64,
    /// Declared before the writer, so that even dropped without `close`
    /// the readers go first.
    readers: ReaderPool,
    writer: Mutex<Option<Case>>,
}

impl OpenCase {
    /// Close the readers (waiting for those out), then the writer.
    fn close(&self) {
        self.readers.close();
        drop(lock(&self.writer).take());
    }
}

/// Everything the commands share.
pub struct AppState {
    open: Mutex<Option<Arc<OpenCase>>>,
    pub queries: QueryRegistry,
    pub settings: SettingsStore,
    /// Held by the write that is running, and by an open or close.
    write_gate: Mutex<()>,
    /// Rises with every open, close and successful write.
    generation: AtomicU64,
    events: Arc<dyn EventSink>,
    /// Pauses a read at a `Point`, so a unit test can interleave a close.
    #[cfg(test)]
    hooks: Mutex<Vec<(Point, Hook)>>,
}

/// What a test runs where it pauses a read.
#[cfg(test)]
type Hook = Box<dyn FnOnce() + Send>;

/// Where a test may pause a read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Point {
    /// The case is found; the query is not registered yet.
    BeforeEnter,
    /// The query has a reader and has not started its work.
    AfterCheckout,
    /// An export has written its partial file and checked its target once,
    /// and has not yet taken the write gate for the second check and the
    /// rename.
    BeforeRename,
}

/// How often an export waiting for the write gate looks whether it was
/// stopped meanwhile.
const GATE_POLL: Duration = Duration::from_millis(10);

impl AppState {
    pub fn new(settings: SettingsStore, events: Arc<dyn EventSink>) -> AppState {
        AppState {
            open: Mutex::new(None),
            queries: QueryRegistry::default(),
            settings,
            write_gate: Mutex::new(()),
            generation: AtomicU64::new(0),
            events,
            #[cfg(test)]
            hooks: Mutex::new(Vec::new()),
        }
    }

    /// Run the test hook set for `at`, once. Nothing outside tests.
    pub(crate) fn hook(&self, at: Point) {
        #[cfg(test)]
        {
            let hook = {
                let mut hooks = lock(&self.hooks);
                let found = hooks.iter().position(|(p, _)| *p == at);
                found.map(|i| hooks.remove(i).1)
            };
            if let Some(hook) = hook {
                hook();
            }
        }
        #[cfg(not(test))]
        let _ = at;
    }

    #[cfg(test)]
    pub(crate) fn on(&self, at: Point, hook: impl FnOnce() + Send + 'static) {
        lock(&self.hooks).push((at, Box::new(hook)));
    }

    /// The app's state with its settings read from `dir`. A damaged
    /// settings file is set aside and announced once (`settings-damaged`);
    /// that event may come before the window listens, so `settings_get`
    /// also reports it until the settings are next saved.
    pub fn with_settings_dir(dir: &Path, events: Arc<dyn EventSink>) -> AppState {
        let (settings, damaged) = SettingsStore::load(dir);
        let state = AppState::new(settings, events);
        if let Some(damaged) = damaged {
            state.emit(SETTINGS_DAMAGED, &damaged);
        }
        state
    }

    fn emit(&self, event: &str, payload: &impl serde::Serialize) {
        send(self.events.as_ref(), event, payload);
    }

    /// Who the app's changes are recorded under (`settings::app_actor`).
    pub fn actor(&self) -> Actor {
        app_actor(&self.settings.get(), os_user().as_deref())
    }

    /// The app's generation now.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// The open case; `no_case` when there is none.
    pub fn current(&self) -> Result<Arc<OpenCase>, CmdError> {
        lock(&self.open).clone().ok_or_else(CmdError::no_case)
    }

    /// The write gate; `busy` while another write, open or close holds it.
    fn gate(&self) -> Result<MutexGuard<'_, ()>, CmdError> {
        match self.write_gate.try_lock() {
            Ok(guard) => Ok(guard),
            Err(TryLockError::Poisoned(p)) => Ok(p.into_inner()),
            Err(TryLockError::WouldBlock) => Err(CmdError::busy()),
        }
    }

    /// Run `work` on a reader as query `query_id` in `slot` (see
    /// `QueryRegistry::run`): `no_case` when no case is open.
    pub fn read<T>(
        &self,
        slot: &str,
        query_id: &str,
        work: impl FnOnce(&Case) -> oxl_engine::Result<T>,
    ) -> Result<T, CmdError> {
        let open = self.current()?;
        self.hook(Point::BeforeEnter);
        // Registered before waiting for a reader, so the slot's order is the
        // order requests arrived in; and only while the case this read found
        // is still the open one, so a read of a closed case touches nothing.
        let ticket = self.queries.enter_case(open.generation, slot, query_id)?;
        let reader = open.readers.checkout(&ticket)?;
        self.hook(Point::AfterCheckout);
        ticket.run(&reader, work)
    }

    /// `read`, for work whose result is something it did outside the case
    /// (an export's file): `flag` is set as soon as the query is stopped
    /// (superseded, cancelled, or its case closing), for the work to look
    /// at between statements, and when the work completes its result is the
    /// answer even if the query was stopped after it
    /// (`Ticket::run_to_completion`). Work that fails once the query was
    /// stopped answers the stop code.
    pub fn read_to_completion<T>(
        &self,
        slot: &str,
        query_id: &str,
        flag: Arc<AtomicBool>,
        work: impl FnOnce(&Case) -> oxl_engine::Result<T>,
    ) -> Result<T, CmdError> {
        let open = self.current()?;
        let ticket = self
            .queries
            .enter_case_with_flag(open.generation, slot, query_id, flag)?;
        let reader = open.readers.checkout(&ticket)?;
        ticket.run_to_completion(&reader, work)
    }

    /// Run `run` announced as a job of `kind`: `job-started` before it,
    /// `job-finished` after it, with its error's message when it failed.
    pub fn announced<T>(
        &self,
        kind: &str,
        label: &str,
        run: impl FnOnce() -> Result<T, CmdError>,
    ) -> Result<T, CmdError> {
        self.emit(
            JOB_STARTED,
            &JobStarted {
                kind: kind.to_string(),
                label: label.to_string(),
            },
        );
        let result = run();
        self.emit(
            JOB_FINISHED,
            &JobFinished {
                kind: kind.to_string(),
                ok: result.is_ok(),
                message: result.as_ref().err().map(|e| e.message.clone()),
            },
        );
        result
    }

    /// Run `work` on the writer: `busy` while another write runs, `no_case`
    /// when no case is open. On success the window hears `case-changed`
    /// with `kinds`.
    pub fn write<T>(
        &self,
        kinds: &[&str],
        work: impl FnOnce(&mut Case) -> oxl_engine::Result<T>,
    ) -> Result<T, CmdError> {
        let _gate = self.gate()?;
        let (value, generation) = self.on_writer(work)?;
        self.changed(kinds, generation);
        Ok(value)
    }

    /// `write`, announced as a job of `kind`: `job-started` before the work,
    /// `job-finished` after it, then (on success) `case-changed`. A job that
    /// cannot start (`busy`, `no_case`) announces nothing.
    pub fn job<T>(
        &self,
        kind: &str,
        label: &str,
        kinds: &[&str],
        work: impl FnOnce(&mut Case) -> oxl_engine::Result<T>,
    ) -> Result<T, CmdError> {
        let _gate = self.gate()?;
        self.current()?;
        let (value, generation) = self.announced(kind, label, || self.on_writer(work))?;
        self.changed(kinds, generation);
        Ok(value)
    }

    /// The write gate, waited for while a write runs, for an export that
    /// holds a reader: `stop` is looked at every `GATE_POLL`, and once it is
    /// set (the export's query was cancelled or superseded, or its case is
    /// closing) the wait gives up with `invalid: cancelled`, which the
    /// registry answers as the stop code. A close holds the gate while it
    /// waits for every reader, so a blocking wait here would never end.
    pub fn wait_for_gate(&self, stop: &AtomicBool) -> oxl_engine::Result<MutexGuard<'_, ()>> {
        loop {
            if stop.load(Ordering::SeqCst) {
                return Err(oxl_engine::EngineError::Invalid("cancelled".to_string()));
            }
            match self.write_gate.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(p)) => return Ok(p.into_inner()),
                Err(TryLockError::WouldBlock) => std::thread::sleep(GATE_POLL),
            }
        }
    }

    /// Run `work` on the writer with the write gate held, waiting for a
    /// write that is running to end rather than answering `busy`, and
    /// without announcing a change: for work that must not interleave with a
    /// write but changes nothing in the case (an exported view's last check
    /// of its target, and its rename). The caller must hold no reader, or a
    /// close (which holds the gate while it waits for readers) could not
    /// end. `no_case` when no case is open.
    pub fn serialized<T>(
        &self,
        work: impl FnOnce(&Case) -> oxl_engine::Result<T>,
    ) -> Result<T, CmdError> {
        let _gate = lock(&self.write_gate);
        let open = self.current()?;
        let writer = lock(&open.writer);
        let case = writer.as_ref().ok_or_else(CmdError::no_case)?;
        Ok(work(case)?)
    }

    /// Run `work` on the writer; the caller holds the gate. The new
    /// generation on success.
    fn on_writer<T>(
        &self,
        work: impl FnOnce(&mut Case) -> oxl_engine::Result<T>,
    ) -> Result<(T, u64), CmdError> {
        let open = self.current()?;
        let mut writer = lock(&open.writer);
        let case = writer.as_mut().ok_or_else(CmdError::no_case)?;
        let value = work(case)?;
        Ok((value, self.next_generation()))
    }

    fn changed(&self, kinds: &[&str], generation: u64) {
        self.emit(
            CASE_CHANGED,
            &CaseChanged {
                kinds: kinds.iter().map(|k| k.to_string()).collect(),
                generation,
            },
        );
    }

    /// Close the open case and open the one `make` opens (or creates), with
    /// its readers; `then` reads what the caller answers with on the new
    /// writer before any command can reach it. `busy` while a write runs.
    /// When `make` fails, no case is open.
    pub fn open_with<T>(
        &self,
        make: impl FnOnce() -> oxl_engine::Result<Case>,
        then: impl FnOnce(&Case) -> oxl_engine::Result<T>,
    ) -> Result<T, CmdError> {
        let _gate = self.gate()?;
        self.close_held();
        let writer = make()?;
        let answer = then(&writer)?;
        let case_id = writer.case_id()?;
        let readers = ReaderPool::open(&writer)?;
        let open = OpenCase {
            path: writer.path().to_path_buf(),
            case_id,
            generation: self.next_generation(),
            readers,
            writer: Mutex::new(Some(writer)),
        };
        // The registry knows the case before any read can find it.
        self.queries.open_case(open.generation);
        *lock(&self.open) = Some(Arc::new(open));
        Ok(answer)
    }

    /// Close the open case, if any: its running queries answer
    /// `interrupted`, and when this returns no connection to its file is
    /// left. `busy` while a write runs.
    pub fn close(&self) -> Result<(), CmdError> {
        let _gate = self.gate()?;
        self.close_held();
        Ok(())
    }

    /// `close`, with the gate held by the caller.
    fn close_held(&self) {
        let Some(open) = lock(&self.open).take() else {
            return;
        };
        // Stop everything registered and let nothing new register, under the
        // registry's lock, so no work starts on a reader this close waits for.
        self.queries.begin_close();
        open.close();
        self.queries.end_close();
        self.next_generation();
    }
}

/// How long quitting waits for the case to close before the app exits
/// anyway.
pub const EXIT_CLOSE_LIMIT: Duration = Duration::from_secs(2);

/// Run `close` on a thread of its own and wait for it at most `limit`;
/// whether it finished. Quitting uses it so that the window's event loop
/// never waits on a reader: `AppState::close` interrupts every query, and a
/// reader held by work no interrupt reaches costs the wait, not the exit.
/// A close still running when the process ends has written nothing; the
/// case's last committed state is what the next open finds.
pub fn close_within(close: impl FnOnce() + Send + 'static, limit: Duration) -> bool {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        close();
        let _ = done.send(());
    });
    finished.recv_timeout(limit).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NoSink;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const ENDLESS: &str = "SELECT count(*) FROM range(10000000000) WHERE range % 7 = 3";

    /// A new, empty case open in a new state.
    fn opened() -> (tempfile::TempDir, Arc<AppState>) {
        let d = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new(SettingsStore::in_memory(), Arc::new(NoSink)));
        crate::cmd::case::case_new(&state, &d.path().join("case.duckdb"), "WS01").unwrap();
        (d, state)
    }

    /// Run `f` on a thread and fail if it takes longer than `limit`.
    fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(limit)
            .unwrap_or_else(|_| panic!("did not finish within {limit:?}"))
    }

    /// Pause the next read at `at` until the returned sender sends; the
    /// first receiver hears when it got there.
    fn pause(state: &AppState, at: Point) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (reached_tx, reached) = mpsc::channel();
        let (go, go_rx) = mpsc::channel::<()>();
        state.on(at, move || {
            reached_tx.send(()).unwrap();
            go_rx.recv().unwrap();
        });
        (reached, go)
    }

    /// Close `state` on a thread; the receiver hears when it returned.
    fn closing(state: &Arc<AppState>) -> mpsc::Receiver<Result<(), CmdError>> {
        let (tx, rx) = mpsc::channel();
        {
            let state = state.clone();
            std::thread::spawn(move || {
                let _ = tx.send(state.close());
            });
        }
        // Close takes the case away at once, then waits for its readers.
        let start = Instant::now();
        while state_open(state) {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "close did not begin"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(100));
        rx
    }

    fn state_open(state: &AppState) -> bool {
        lock(&state.open).is_some()
    }

    #[test]
    fn a_read_arriving_as_the_case_closes_never_runs() {
        let (_d, state) = opened();
        within(Duration::from_secs(30), move || {
            let (reached, go) = pause(&state, Point::BeforeEnter);
            let ran = Arc::new(AtomicBool::new(false));
            let read = {
                let (state, ran) = (state.clone(), ran.clone());
                std::thread::spawn(move || {
                    state.read("t1:page", "q1", |c| {
                        ran.store(true, Ordering::SeqCst);
                        c.scalar_i64(ENDLESS)
                    })
                })
            };
            reached.recv().unwrap();
            let closed = closing(&state);
            go.send(()).unwrap();
            assert_eq!(read.join().unwrap().unwrap_err().code, "no_case");
            assert!(
                !ran.load(Ordering::SeqCst),
                "the read ran after close began"
            );
            let closed = closed
                .recv_timeout(Duration::from_secs(5))
                .expect("close returned");
            assert_eq!(closed, Ok(()));
        });
    }

    #[test]
    fn a_read_holding_a_reader_as_the_case_closes_never_runs() {
        let (_d, state) = opened();
        within(Duration::from_secs(30), move || {
            let (reached, go) = pause(&state, Point::AfterCheckout);
            let ran = Arc::new(AtomicBool::new(false));
            let read = {
                let (state, ran) = (state.clone(), ran.clone());
                std::thread::spawn(move || {
                    state.read("t1:page", "q1", |c| {
                        ran.store(true, Ordering::SeqCst);
                        c.scalar_i64(ENDLESS)
                    })
                })
            };
            reached.recv().unwrap();
            let closed = closing(&state);
            go.send(()).unwrap();
            let answer = read.join().unwrap().unwrap_err().code;
            assert!(
                answer == "interrupted" || answer == "no_case",
                "answered {answer}"
            );
            assert!(
                !ran.load(Ordering::SeqCst),
                "the read ran after close began"
            );
            let closed = closed
                .recv_timeout(Duration::from_secs(5))
                .expect("close returned");
            assert_eq!(closed, Ok(()));
        });
    }

    #[test]
    fn a_stale_read_does_not_supersede_the_next_cases_query() {
        let (d, state) = opened();
        within(Duration::from_secs(30), move || {
            // A read of the first case pauses before it registers.
            let (reached, go) = pause(&state, Point::BeforeEnter);
            let ran = Arc::new(AtomicBool::new(false));
            let stale = {
                let (state, ran) = (state.clone(), ran.clone());
                std::thread::spawn(move || {
                    state.read("t1:page", "old", |c| {
                        ran.store(true, Ordering::SeqCst);
                        c.scalar_i64("SELECT 1")
                    })
                })
            };
            reached.recv().unwrap();
            // That case closes and the next one opens.
            state.close().unwrap();
            crate::cmd::case::case_new(&state, &d.path().join("next.duckdb"), "SRV01").unwrap();
            // A query of the new case is running on the same slot.
            let (started_tx, started) = mpsc::channel();
            let (finish, finished) = mpsc::channel::<()>();
            let fresh = {
                let state = state.clone();
                std::thread::spawn(move || {
                    state.read("t1:page", "new", |c| {
                        started_tx.send(()).unwrap();
                        finished.recv().unwrap();
                        c.scalar_i64("SELECT 5")
                    })
                })
            };
            started.recv().unwrap();
            // The stale read resumes: it answers no_case and touches nothing.
            go.send(()).unwrap();
            assert_eq!(stale.join().unwrap().unwrap_err().code, "no_case");
            assert!(!ran.load(Ordering::SeqCst));
            finish.send(()).unwrap();
            assert_eq!(fresh.join().unwrap(), Ok(5));
            let _keep = d;
        });
    }

    /// Run `work` as query `id` on a thread, wait until it has started, cancel
    /// it, and answer what it answered. `work` runs through
    /// `read_to_completion` when `to_completion`, with the query's flag,
    /// which the cancel sets; through `read` otherwise, with a flag the test
    /// sets once the cancel is in.
    fn cancelled_while_running(
        state: &Arc<AppState>,
        id: &'static str,
        to_completion: bool,
        work: impl FnOnce(&AtomicBool) -> oxl_engine::Result<u32> + Send + 'static,
    ) -> Result<u32, CmdError> {
        let (started_tx, started) = mpsc::channel();
        let flag = Arc::new(AtomicBool::new(false));
        let run = {
            let (state, flag) = (state.clone(), flag.clone());
            std::thread::spawn(move || {
                let job = |_c: &Case| {
                    started_tx.send(()).unwrap();
                    work(&flag)
                };
                if to_completion {
                    state.read_to_completion("export", id, flag.clone(), job)
                } else {
                    state.read("export", id, job)
                }
            })
        };
        started.recv().unwrap();
        state.queries.cancel(&[id.to_string()]);
        if to_completion {
            assert!(flag.load(Ordering::SeqCst), "the cancel set the flag");
        } else {
            flag.store(true, Ordering::SeqCst);
        }
        run.join().unwrap()
    }

    /// Wait for the flag, as an export looks at it between chunks.
    fn until_set(flag: &AtomicBool) {
        let start = Instant::now();
        while !flag.load(Ordering::SeqCst) {
            assert!(start.elapsed() < Duration::from_secs(10), "never cancelled");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn work_that_completes_after_a_cancel_answers_its_result() {
        let (_d, state) = opened();
        within(Duration::from_secs(30), move || {
            // The file is renamed into place after the cancel arrived: the
            // answer is the report, because the file is there.
            let done = cancelled_while_running(&state, "e1", true, |flag| {
                until_set(flag);
                Ok(7)
            });
            assert_eq!(done, Ok(7));
            // Work that stops for the cancel answers `interrupted`, never
            // its own error (an export's `invalid: cancelled`, or DuckDB's
            // interrupt).
            let stopped = cancelled_while_running(&state, "e2", true, |flag| {
                until_set(flag);
                Err(oxl_engine::EngineError::Invalid("cancelled".into()))
            });
            assert_eq!(stopped.unwrap_err().code, "interrupted");
            // A plain read stopped the same way never answers what it read.
            let read = cancelled_while_running(&state, "e3", false, |flag| {
                until_set(flag);
                Ok(7)
            });
            assert_eq!(read.unwrap_err().code, "interrupted");
        });
    }

    #[test]
    fn closing_for_exit_is_bounded() {
        let (_d, state) = opened();
        within(Duration::from_secs(30), move || {
            // A reader held by work no interrupt reaches.
            let (held_tx, held) = mpsc::channel();
            let read = {
                let state = state.clone();
                std::thread::spawn(move || {
                    state.read("t1:page", "q1", |_c| {
                        held_tx.send(()).unwrap();
                        std::thread::sleep(Duration::from_secs(8));
                        Ok(())
                    })
                })
            };
            held.recv().unwrap();
            let start = Instant::now();
            let finished = {
                let state = state.clone();
                close_within(
                    move || {
                        let _ = state.close();
                    },
                    Duration::from_secs(2),
                )
            };
            let took = start.elapsed();
            assert!(!finished, "close cannot finish while the reader is held");
            assert!(took < Duration::from_secs(3), "close_within took {took:?}");
            let _ = read.join();
        });
    }

    #[test]
    fn closing_for_exit_interrupts_queries() {
        let (_d, state) = opened();
        within(Duration::from_secs(30), move || {
            let read = {
                let state = state.clone();
                std::thread::spawn(move || state.read("t1:page", "q1", |c| c.scalar_i64(ENDLESS)))
            };
            let start = Instant::now();
            while !state.queries.running().iter().any(|q| q == "q1") {
                assert!(start.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(5));
            }
            let finished = {
                let state = state.clone();
                close_within(
                    move || {
                        let _ = state.close();
                    },
                    Duration::from_secs(2),
                )
            };
            assert!(finished, "the interrupted query let close finish");
            assert_eq!(read.join().unwrap().unwrap_err().code, "interrupted");
            assert!(state.current().is_err());
        });
    }
}
