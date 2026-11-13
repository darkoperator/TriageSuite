//! Every read the window asks for runs here, under a slot and a query id.
//!
//! A slot names what a query fills on screen (`t1:page`, `t1:hist`): a new
//! query on a slot makes the one already there stale, so that one is
//! interrupted and answers `superseded`. `cancel` stops queries by id, and
//! they answer `interrupted`; an id cancelled before its query starts is
//! remembered, and that query answers `interrupted` without running.
//!
//! A query is registered (`enter`) before it waits for a connection, so the
//! order of a slot's queries is the order they arrived in, not the order
//! they got a connection: a query superseded or cancelled while it waits
//! never runs, and when it gets a connection it runs only if nothing stopped
//! it meanwhile (`Ticket::run`, which checks and starts under one lock).
//!
//! DuckDB clears an interrupt when the next statement starts, so one that
//! lands between two statements of a call is lost. Once a running query is
//! stopped, a thread of its own interrupts it again every `REPEAT` until the
//! call returns, and is joined before the connection goes back to the pool,
//! so it never interrupts the next query on that connection.
//!
//! Why a query was stopped is the registry's own record, not DuckDB's error
//! text (which carries no code for an interrupt): a query the registry
//! stopped answers `superseded` or `interrupted` however its call ended,
//! because its answer is stale either way and the window must not draw it.
//!
//! While a case closes (`begin_close` to `end_close`) nothing new registers
//! (`no_case`) and everything registered is stopped, so no work starts on a
//! connection the close is waiting for.

use crate::error::CmdError;
use duckdb::InterruptHandle;
use oxl_engine::Case;
use std::collections::VecDeque;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

/// How often a stopped query is interrupted again until it returns.
pub const REPEAT: Duration = Duration::from_millis(50);
/// Ids cancelled before their query started that are remembered; the
/// oldest is forgotten first.
pub const MAX_PENDING: usize = 256;
/// Characters in a query id or a slot name.
pub const MAX_ID: usize = 64;

/// Why the registry stopped a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Superseded,
    Cancelled,
}

impl Stop {
    fn error(self) -> CmdError {
        match self {
            Stop::Superseded => CmdError::superseded(),
            Stop::Cancelled => CmdError::interrupted(),
        }
    }
}

/// The thread that keeps interrupting a stopped query.
struct Interrupter {
    done: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

impl Interrupter {
    fn start(handle: Arc<InterruptHandle>) -> Interrupter {
        let (done, finished) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || loop {
            handle.interrupt();
            // A message or a dropped sender both mean the call returned.
            if finished.recv_timeout(REPEAT) != Err(RecvTimeoutError::Timeout) {
                break;
            }
        });
        Interrupter { done, thread }
    }

    /// End the thread and wait for it: after this, nothing interrupts the
    /// connection on this query's behalf.
    fn finish(self) {
        drop(self.done);
        let _ = self.thread.join();
    }
}

/// One registered query: waiting for a connection (`handle` is `None`), or
/// running on one.
struct Entry {
    token: u64,
    slot: String,
    id: String,
    handle: Option<Arc<InterruptHandle>>,
    stop: Option<Stop>,
    interrupter: Option<Interrupter>,
}

impl Entry {
    /// Stop this query for `why`, unless it was already stopped (the first
    /// reason is the one it answers with). A running query is interrupted
    /// until it returns; a waiting one simply never starts.
    fn stop(&mut self, why: Stop) {
        if self.stop.is_none() {
            self.stop = Some(why);
            if let Some(handle) = &self.handle {
                self.interrupter = Some(Interrupter::start(Arc::clone(handle)));
            }
        }
    }
}

#[derive(Default)]
struct Inner {
    next_token: u64,
    /// Every registered query, waiting or running.
    entries: Vec<Entry>,
    /// Ids cancelled before their query started, oldest first.
    pending: VecDeque<String>,
    /// A case is closing: nothing registers.
    closing: bool,
    /// The generation of the open case (`open_case`), `None` from the moment
    /// it begins to close: a read of any other case registers nothing.
    case: Option<u64>,
}

/// The queries registered now, by slot and id.
#[derive(Default)]
pub struct QueryRegistry {
    inner: Mutex<Inner>,
}

/// Refuse a query id or slot name the window should never send.
pub(crate) fn check_name(what: &str, value: &str) -> Result<(), CmdError> {
    let fits = !value.is_empty()
        && value.len() <= MAX_ID
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'.'));
    if fits {
        Ok(())
    } else {
        Err(CmdError::invalid(format!(
            "a {what} is 1 to {MAX_ID} characters of A-Z a-z 0-9 _ - : ."
        )))
    }
}

/// A registered query. It is unregistered when it is run or dropped.
pub struct Ticket<'a> {
    registry: &'a QueryRegistry,
    token: u64,
}

impl Ticket<'_> {
    /// Why the query was stopped, as the error it answers with; `None` while
    /// it may still run.
    pub fn stopped(&self) -> Option<CmdError> {
        let inner = self.registry.lock();
        inner
            .entries
            .iter()
            .find(|e| e.token == self.token)
            .and_then(|e| e.stop)
            .map(Stop::error)
    }

    /// Run `work` on `conn`, unless the query was stopped while it waited
    /// (its stop code, `work` never called) or a close began (`no_case`).
    /// Checking and starting happen under one lock, so a supersede, cancel
    /// or close after that point interrupts the work instead.
    pub fn run<T>(
        self,
        conn: &Case,
        work: impl FnOnce(&Case) -> oxl_engine::Result<T>,
    ) -> Result<T, CmdError> {
        {
            let mut inner = self.registry.lock();
            let closing = inner.closing;
            let Some(entry) = inner.entries.iter_mut().find(|e| e.token == self.token) else {
                return Err(CmdError::no_case());
            };
            if let Some(why) = entry.stop {
                return Err(why.error());
            }
            if closing {
                return Err(CmdError::no_case());
            }
            entry.handle = Some(conn.interrupt_handle());
        }
        let result = work(conn);
        match self.finish() {
            Some(why) => Err(why.error()),
            None => result.map_err(CmdError::from),
        }
    }

    /// Unregister the query; why it was stopped, if it was. Its interrupter
    /// is finished first.
    fn finish(&self) -> Option<Stop> {
        let entry = {
            let mut inner = self.registry.lock();
            let at = inner.entries.iter().position(|e| e.token == self.token)?;
            inner.entries.swap_remove(at)
        };
        if let Some(interrupter) = entry.interrupter {
            interrupter.finish();
        }
        entry.stop
    }
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        // After `finish` the entry is gone and this does nothing; a query
        // that never ran, or whose work panicked, is unregistered here.
        self.finish();
    }
}

impl QueryRegistry {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Register query `query_id` in `slot`, before it has a connection.
    /// Every query already in the slot, waiting or running, is stopped as
    /// superseded. `interrupted` for an id cancelled before it arrived;
    /// `no_case` while a case closes.
    pub fn enter(&self, slot: &str, query_id: &str) -> Result<Ticket<'_>, CmdError> {
        self.register(None, slot, query_id)
    }

    /// `enter` for a read of the case opened at generation `case`: `no_case`
    /// unless that case is the open one (`open_case`), checked under the
    /// same lock as the rest, so a read that captured a case before it
    /// closed never registers, supersedes nothing and consumes no pending
    /// cancel once another case is open.
    pub fn enter_case(
        &self,
        case: u64,
        slot: &str,
        query_id: &str,
    ) -> Result<Ticket<'_>, CmdError> {
        self.register(Some(case), slot, query_id)
    }

    fn register(
        &self,
        case: Option<u64>,
        slot: &str,
        query_id: &str,
    ) -> Result<Ticket<'_>, CmdError> {
        check_name("slot", slot)?;
        check_name("query id", query_id)?;
        let mut inner = self.lock();
        if inner.closing {
            return Err(CmdError::no_case());
        }
        if case.is_some() && inner.case != case {
            return Err(CmdError::no_case());
        }
        if let Some(at) = inner.pending.iter().position(|id| id == query_id) {
            inner.pending.remove(at);
            return Err(CmdError::interrupted());
        }
        for entry in inner.entries.iter_mut().filter(|e| e.slot == slot) {
            entry.stop(Stop::Superseded);
        }
        let token = inner.next_token;
        inner.next_token += 1;
        inner.entries.push(Entry {
            token,
            slot: slot.to_string(),
            id: query_id.to_string(),
            handle: None,
            stop: None,
            interrupter: None,
        });
        Ok(Ticket {
            registry: self,
            token,
        })
    }

    /// Run `work` on `conn` as query `query_id` in `slot`: `enter`, then
    /// `Ticket::run`. Any query already in the slot answers `superseded`;
    /// this one answers `superseded` or `interrupted` in turn if a newer
    /// query or a cancel stops it, and its own result otherwise.
    pub fn run<T>(
        &self,
        slot: &str,
        query_id: &str,
        conn: &Case,
        work: impl FnOnce(&Case) -> oxl_engine::Result<T>,
    ) -> Result<T, CmdError> {
        self.enter(slot, query_id)?.run(conn, work)
    }

    /// Stop the queries `query_ids` names: a registered one answers
    /// `interrupted` (interrupted if it is running); an id not registered is
    /// remembered (up to `MAX_PENDING`), so its query answers `interrupted`
    /// when it arrives.
    pub fn cancel(&self, query_ids: &[String]) {
        let mut inner = self.lock();
        for id in query_ids {
            let mut registered = false;
            for entry in inner.entries.iter_mut().filter(|e| &e.id == id) {
                entry.stop(Stop::Cancelled);
                registered = true;
            }
            if !registered && !inner.pending.contains(id) {
                if inner.pending.len() == MAX_PENDING {
                    inner.pending.pop_front();
                }
                inner.pending.push_back(id.clone());
            }
        }
    }

    /// A case begins to close: from now until `end_close` nothing
    /// registers, and every registered query is stopped (`interrupted`).
    pub fn begin_close(&self) {
        let mut inner = self.lock();
        inner.closing = true;
        inner.case = None;
        for entry in inner.entries.iter_mut() {
            entry.stop(Stop::Cancelled);
        }
    }

    /// The close is over; queries register again (a case's reads once
    /// `open_case` names it).
    pub fn end_close(&self) {
        self.lock().closing = false;
    }

    /// The case opened at generation `case` is the open one: its reads
    /// (`enter_case`) register from now on.
    pub fn open_case(&self, case: u64) {
        self.lock().case = Some(case);
    }

    /// The ids of the queries running now (on a connection), in the order
    /// they registered.
    pub fn running(&self) -> Vec<String> {
        let inner = self.lock();
        let mut entries: Vec<(u64, String)> = inner
            .entries
            .iter()
            .filter(|e| e.handle.is_some())
            .map(|e| (e.token, e.id.clone()))
            .collect();
        entries.sort();
        entries.into_iter().map(|(_, id)| id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert!(check_name("slot", "t1:page").is_ok());
        assert!(check_name("query id", "q-1_2.3").is_ok());
        assert_eq!(check_name("slot", "").unwrap_err().code, "invalid");
        assert_eq!(
            check_name("slot", &"a".repeat(65)).unwrap_err().code,
            "invalid"
        );
        assert_eq!(check_name("slot", "a b").unwrap_err().code, "invalid");
        assert_eq!(check_name("slot", "<x>").unwrap_err().code, "invalid");
    }

    #[test]
    fn pending_ids_are_capped_oldest_first() {
        let registry = QueryRegistry::default();
        let ids: Vec<String> = (0..MAX_PENDING + 5).map(|n| format!("q{n}")).collect();
        registry.cancel(&ids);
        registry.cancel(&["q100".to_string()]);
        let inner = registry.lock();
        assert_eq!(inner.pending.len(), MAX_PENDING);
        assert_eq!(inner.pending.front().map(String::as_str), Some("q5"));
        assert_eq!(
            inner.pending.iter().filter(|id| *id == "q100").count(),
            1,
            "an id is remembered once"
        );
    }

    #[test]
    fn a_waiting_query_is_superseded_without_running() {
        let registry = QueryRegistry::default();
        let first = registry.enter("t1:page", "a").unwrap();
        let second = registry.enter("t1:page", "b").unwrap();
        assert_eq!(first.stopped().unwrap().code, "superseded");
        assert!(second.stopped().is_none());
        // Nothing is registered once both tickets are gone.
        drop(first);
        drop(second);
        assert!(registry.lock().entries.is_empty());
    }

    #[test]
    fn nothing_registers_while_a_case_closes() {
        let registry = QueryRegistry::default();
        let waiting = registry.enter("t1:page", "a").unwrap();
        registry.begin_close();
        assert_eq!(waiting.stopped().unwrap().code, "interrupted");
        assert_eq!(
            registry.enter("t1:hist", "b").err().unwrap().code,
            "no_case"
        );
        registry.end_close();
        assert!(registry.enter("t1:hist", "c").is_ok());
    }

    #[test]
    fn a_read_of_another_case_registers_nothing() {
        let registry = QueryRegistry::default();
        registry.open_case(1);
        let first = registry.enter_case(1, "t1:page", "a").unwrap();
        registry.begin_close();
        registry.end_close();
        drop(first);
        registry.open_case(3);
        let current = registry.enter_case(3, "t1:page", "b").unwrap();
        registry.cancel(&["c".to_string()]);
        // A read of case 1 is refused before it touches anything.
        assert_eq!(
            registry.enter_case(1, "t1:page", "c").err().unwrap().code,
            "no_case"
        );
        assert!(current.stopped().is_none(), "the stale read superseded");
        assert!(registry.lock().pending.contains(&"c".to_string()));
        assert_eq!(registry.lock().entries.len(), 1);
    }
}
