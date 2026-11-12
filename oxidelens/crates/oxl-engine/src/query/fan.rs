//! One statement per relation, several at a time.
//!
//! A pivot asks every relation the same question. Most relations hold fewer
//! rows than one of DuckDB's row groups, and DuckDB hands a row group to one
//! thread, so asked one after another the questions each use a single core
//! while the rest idle. `in_order` asks several at once, each on its own
//! connection to the case, and still hands the answers back in order.

use crate::error::Result;
use duckdb::Connection;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

/// The most connections one call works on at once. Measured over 164
/// relations on a 32-thread machine: 4 was slower, 16 no faster, and 32
/// slower again, since DuckDB's own threads are busy with the same scans.
const MAX_WORKERS: usize = 8;

/// Run `work` on each of `items`, several at a time, each on a connection of
/// its own to `conn`'s database, and hand each result to `deliver` in item
/// order, as soon as it and every one before it is done. `deliver` returning
/// false stops the rest: no later result is delivered, and work not yet
/// started is not started. Returns whether every item was delivered. The
/// first error in item order is returned, after the results before it were
/// delivered.
///
/// `work` runs on other threads and sees only what the database has
/// committed; `deliver` runs on the caller's.
pub(super) fn in_order<I, T>(
    conn: &Connection,
    items: &[I],
    work: impl Fn(&Connection, &I) -> Result<T> + Sync,
    mut deliver: impl FnMut(&I, T) -> bool,
) -> Result<bool>
where
    I: Sync,
    T: Send,
{
    let connections = (0..items.len().min(MAX_WORKERS))
        .map(|_| conn.try_clone())
        .collect::<std::result::Result<Vec<_>, _>>()?;
    // The next item nobody has taken, and whether to stop taking any.
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let (done, results) = mpsc::channel::<(usize, Result<T>)>();

    std::thread::scope(|scope| {
        for connection in connections {
            let (done, next, stop, work) = (done.clone(), &next, &stop, &work);
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else {
                        break;
                    };
                    if done.send((index, work(&connection, item))).is_err() {
                        break;
                    }
                }
            });
        }
        // Only the workers hold a sender now, so `results` ends when they do.
        drop(done);

        // Results that arrived before an earlier item's.
        let mut early = BTreeMap::new();
        let mut due = 0;
        for (index, result) in results {
            early.insert(index, result);
            while let Some(result) = early.remove(&due) {
                let more = match result {
                    Ok(value) => deliver(&items[due], value),
                    Err(e) => {
                        stop.store(true, Ordering::Relaxed);
                        return Err(e);
                    }
                };
                if !more {
                    stop.store(true, Ordering::Relaxed);
                    return Ok(false);
                }
                due += 1;
            }
        }
        Ok(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EngineError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::Mutex;
    use std::time::Duration;

    fn conn() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    /// What the statement `SELECT <n> * 2` returns on `conn`.
    fn doubled(conn: &Connection, n: &i64) -> Result<i64> {
        Ok(conn.query_row("SELECT ? * 2", [n], |r| r.get(0))?)
    }

    /// Found on the first real run: a find asked 164 relations one after
    /// another, each scan on one core of 32, and took 2.5 s over them.
    #[test]
    fn items_are_worked_on_at_the_same_time() {
        // The first item's work does not finish until the second item's has
        // started, which can only happen when the two overlap.
        let (started, second_started) = mpsc::channel::<()>();
        let (started, second_started) = (Mutex::new(started), Mutex::new(second_started));
        let work = |conn: &Connection, n: &i64| -> Result<i64> {
            match *n {
                0 => second_started
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(20))
                    .map_err(|_| {
                        EngineError::Invalid("the second item never ran alongside the first".into())
                    })?,
                1 => started.lock().unwrap().send(()).unwrap(),
                _ => {}
            }
            doubled(conn, n)
        };
        let mut got = Vec::new();
        let done = in_order(&conn(), &[0, 1, 2, 3], work, |_, n| {
            got.push(n);
            true
        });
        assert!(done.unwrap());
        assert_eq!(got, [0, 2, 4, 6]);
    }

    #[test]
    fn results_are_delivered_in_item_order_whichever_finishes_first() {
        let items: Vec<i64> = (0..40).collect();
        // Earlier items take longer, so later ones finish first.
        let work = |conn: &Connection, n: &i64| -> Result<i64> {
            std::thread::sleep(Duration::from_millis(u64::try_from(40 - *n).unwrap()));
            doubled(conn, n)
        };
        let mut got = Vec::new();
        let done = in_order(&conn(), &items, work, |item, n| {
            got.push((*item, n));
            true
        });
        assert!(done.unwrap());
        assert_eq!(got, items.iter().map(|n| (*n, n * 2)).collect::<Vec<_>>());
    }

    #[test]
    fn stopping_delivers_nothing_more_and_starts_little_more() {
        let items: Vec<i64> = (0..1000).collect();
        let worked = AtomicUsize::new(0);
        let work = |conn: &Connection, n: &i64| -> Result<i64> {
            worked.fetch_add(1, Ordering::SeqCst);
            doubled(conn, n)
        };
        let mut got = Vec::new();
        let done = in_order(&conn(), &items, work, |_, n| {
            got.push(n);
            got.len() < 3
        });
        assert!(!done.unwrap());
        assert_eq!(got, [0, 2, 4]);
        // Work already under way when the stop came may finish; the rest of
        // the thousand is never started.
        assert!(worked.load(Ordering::SeqCst) < 500);
    }

    #[test]
    fn the_first_error_in_item_order_is_returned_after_what_came_before_it() {
        let items: Vec<i64> = (0..20).collect();
        let work = |conn: &Connection, n: &i64| -> Result<i64> {
            if *n == 5 || *n == 9 {
                return Err(EngineError::Invalid(format!("item {n}")));
            }
            doubled(conn, n)
        };
        let mut got = Vec::new();
        let failed = in_order(&conn(), &items, work, |_, n| {
            got.push(n);
            true
        })
        .unwrap_err();
        assert_eq!(failed.to_string(), "invalid: item 5");
        assert_eq!(got, [0, 2, 4, 6, 8]);
    }

    #[test]
    fn no_items_is_no_work() {
        let none: [i64; 0] = [];
        assert!(in_order(&conn(), &none, doubled, |_, _| true).unwrap());
    }
}
