//! `filetime_to_utc` decodes a FILETIME read out of the AppCompatCache blob,
//! whose length and contents are attacker-controlled. It keeps its own copy of
//! the FILETIME arithmetic rather than depending on `triage-core`, because this
//! crate is deliberately a leaf with one dependency; that copy has to be swept
//! here instead.

use triage_appcompat::filetime_to_utc;
use triage_testkit::boundary::{assert_total, EXTREME_U64};

#[test]
fn filetime_to_utc_is_total() {
    assert_total("filetime_to_utc", EXTREME_U64, filetime_to_utc);
}

#[test]
fn zero_is_unset_rather_than_the_1601_epoch() {
    assert!(filetime_to_utc(0).is_none());
    assert!(filetime_to_utc(u64::MAX).is_none());
}
