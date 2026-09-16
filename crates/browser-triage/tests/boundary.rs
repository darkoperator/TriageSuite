//! Every browser decoder swept over the extremes of its input type.
//!
//! The values here come out of SQLite cells in a profile that a subject
//! controls: `visits.transition`, `downloads.state`, `logins.date_created`.
//! SQLite's dynamic typing means any of them can hold any integer regardless
//! of the column's declared type, so "the schema says it is a small enum" is
//! not a constraint on what arrives.
//!
//! `webkit_or_time_t` is the reason this file exists: it called `abs()` on a
//! `date_created` read straight from a cell, and `i64::MIN.abs()` overflows.
//! The sweep below now covers that input on every decoder, not just the one
//! that was caught.

use browser_triage::chromium;
use browser_triage::firefox;
use triage_testkit::boundary::{assert_total, EXTREME_I64};

#[test]
fn every_chromium_decoder_is_total() {
    assert_total("chromium::transition_core", EXTREME_I64, |v| {
        chromium::transition_core(v)
    });
    assert_total("chromium::transition_qualifiers", EXTREME_I64, |v| {
        chromium::transition_qualifiers(v)
    });
    assert_total("chromium::download_state", EXTREME_I64, |v| {
        chromium::download_state(v)
    });
    assert_total("chromium::danger_type", EXTREME_I64, |v| {
        chromium::danger_type(v)
    });
    assert_total("chromium::interrupt_reason", EXTREME_I64, |v| {
        chromium::interrupt_reason(v)
    });
    assert_total("chromium::time_t_or_none", EXTREME_I64, |v| {
        chromium::time_t_or_none(Some(v)).to_string()
    });
    assert_total("chromium::logins::webkit_or_time_t", EXTREME_I64, |v| {
        let (ts, legacy) = chromium::logins::webkit_or_time_t(v);
        (ts.to_string(), legacy)
    });
}

#[test]
fn every_firefox_decoder_is_total() {
    assert_total("firefox::visit_type", EXTREME_I64, |v| {
        firefox::visit_type(Some(v))
    });
    assert_total("firefox::visit_source", EXTREME_I64, |v| {
        firefox::visit_source(Some(v))
    });
    assert_total("firefox::bookmark_type", EXTREME_I64, |v| {
        firefox::bookmark_type(Some(v))
    });
    assert_total("firefox::download_metadata_state", EXTREME_I64, |v| {
        firefox::download_metadata_state(Some(v))
    });
}

/// A value no table names must still reach the analyst with its number
/// attached. Reporting `Unknown (<n>)` rather than an empty cell is what keeps
/// a new Chromium enum value visible instead of looking like absent evidence.
#[test]
fn an_unrecognized_value_is_reported_with_its_number() {
    assert_eq!(
        chromium::download_state(i64::MIN),
        format!("Unknown ({})", i64::MIN)
    );
    assert_eq!(chromium::danger_type(9999), "Unknown (9999)");
    assert_eq!(
        firefox::visit_source(Some(i64::MAX)),
        format!("Unknown ({})", i64::MAX)
    );
    // An absent cell is absent, not unknown: nothing was recorded to decode.
    assert_eq!(firefox::visit_source(None), "");
}

/// The sign bit is the one a decode table forgets. Chromium writes
/// SERVER_REDIRECT with the high bit set, which sign-extends into the `i64`
/// SQLite hands back, so a negative transition is ordinary evidence rather
/// than corruption.
#[test]
fn a_negative_transition_still_decodes_its_qualifier_bits() {
    let server_redirect_link = -0x8000_0000i64 | 0x0100_0000;
    let quals = chromium::transition_qualifiers(server_redirect_link);
    assert!(quals.contains("Server Redirect"), "got {quals}");
    assert!(quals.contains("Forward Back"), "got {quals}");
    assert_eq!(chromium::transition_core(server_redirect_link), "Link");
}
