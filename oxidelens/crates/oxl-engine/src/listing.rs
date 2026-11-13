//! One page of a list, and the page-size rule every paged call shares.
//!
//! Every list the engine pages (groups, marks, history, merged rows, tags,
//! notes) comes back as a `Listing`, so a caller reads the items and whether
//! more follow the same way everywhere. Results that carry more than a list
//! (`Page`, `Stack`, `Histogram`, `FindResult`, `SqlResult`) keep their own
//! structs, each with its own `truncated`.

use crate::error::{EngineError, Result};
use crate::query::MAX_LIMIT;

/// Up to a page of `T`, and whether more follow it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Listing<T> {
    pub items: Vec<T>,
    /// More items exist past this page.
    pub truncated: bool,
}

impl<T> Listing<T> {
    /// The first `limit` of `fetched`, which was fetched with a limit of
    /// `limit + 1`: one extra item says the page is not the last.
    pub(crate) fn from_fetched(mut fetched: Vec<T>, limit: u32) -> Listing<T> {
        let limit = limit as usize;
        let truncated = fetched.len() > limit;
        fetched.truncate(limit);
        Listing {
            items: fetched,
            truncated,
        }
    }
}

/// The page-size rule every paged call shares: 1 to `MAX_LIMIT` items.
/// `Invalid` otherwise; a page of nothing is a mistake, not an empty page.
pub(crate) fn check_limit(limit: u32) -> Result<()> {
    if limit == 0 {
        return Err(EngineError::Invalid(
            "a page is at least 1 row, not 0".to_string(),
        ));
    }
    if limit > MAX_LIMIT {
        return Err(EngineError::Invalid(format!(
            "a page is at most {MAX_LIMIT} rows, not {limit}"
        )));
    }
    Ok(())
}

/// `limit + 1`, as the SQL `LIMIT` that `from_fetched` expects.
pub(crate) fn fetch_limit(limit: u32) -> i64 {
    i64::from(limit) + 1
}
