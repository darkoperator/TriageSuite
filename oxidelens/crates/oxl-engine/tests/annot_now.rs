//! The history's `at` is `now_iso()`. `OXIDELENS_NOW` pins it, but the
//! variable is process-wide, so this test has a binary of its own.

mod common;

use common::{analyst, loaded, n};
use oxl_engine::Listing;
use oxl_engine::{add_note, add_tag, history, mark_suspicious};

#[test]
fn history_at_is_the_pinned_now() {
    let pinned = "2026-02-03T04:05:06.123456Z";
    std::env::set_var("OXIDELENS_NOW", pinned);
    let (_d, mut case) = loaded();
    let r = n(
        &case,
        "select _ox_row from evtxtriage_events where EventId = 4624 and _triage_host = 'WS01'",
    );
    mark_suspicious(&mut case, &[r], &analyst()).unwrap();
    add_tag(&mut case, &[r], "lateral", &analyst()).unwrap();
    let note = add_note(&mut case, r, "seen", &analyst()).unwrap();
    assert_eq!(note.written_at, pinned);
    let Listing { items: rows, .. } = history(&case, None, None, 3, 0).unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|h| h.at == pinned), "{rows:?}");
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.row_mark where marked_at = '2026-02-03T04:05:06.123456Z'"
        ),
        1
    );
}
