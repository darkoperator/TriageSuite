//! Boundary-value sweeps for evidence decoders.
//!
//! Every decoder in the suite takes an integer that came out of an evidence
//! file: a SQLite cell, a registry value, a struct field in a binary artifact.
//! Nothing constrains that integer. It can be `0`, negative where the format
//! says unsigned, or `i64::MIN` because someone crafted it that way.
//!
//! `chromium/logins.rs` called `.abs()` on one of those, and `i64::MIN.abs()`
//! overflows: with overflow checks on — which is the default in the test
//! profile — a single crafted `date_created` aborted the whole parse, and in
//! release it wrapped to a wrong answer wearing a confident label. The decoder
//! had tests. None of them passed it an extreme.
//!
//! The rule this module exists to enforce: **a decoder is total.** Every input
//! in its integer type produces a value or an explicit absence, never a panic.
//! Point it at each decode entry point and it sweeps the values that break the
//! arithmetic — the type's limits, the signs, the sentinels:
//!
//! ```
//! use triage_testkit::boundary::{assert_total, EXTREME_I64};
//!
//! fn decode(value: i64) -> String {
//!     format!("{}", value.unsigned_abs())
//! }
//! assert_total("decode", EXTREME_I64, decode);
//! ```

use std::fmt::Debug;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// The `i64` values that break decoder arithmetic.
///
/// `MIN` overflows negation and `abs`; `MIN + 1` is the value that survives it,
/// so a fix that only special-cases `MIN` still gets tested. `-1` and `0` are
/// the sentinels artifacts actually use for "unset", and `0` is also the one
/// input a truncating division treats differently from a flooring one. `MAX`
/// overflows any multiply into finer units — the `* 100` that turns FILETIME
/// ticks into nanoseconds, the `* 1000` that turns micros into nanos.
pub const EXTREME_I64: &[i64] = &[
    i64::MIN,
    i64::MIN + 1,
    -1_000_000_000_000_000_000,
    -2,
    -1,
    0,
    1,
    2,
    1_000_000_000_000_000_000,
    i64::MAX - 1,
    i64::MAX,
];

/// The `u64` values that break decoder arithmetic. `1 << 63` and above are the
/// range a signed reader misreads as negative, which is how an unsigned field
/// read into an `i64` column turns into a pre-epoch timestamp.
pub const EXTREME_U64: &[u64] = &[
    0,
    1,
    2,
    i64::MAX as u64 - 1,
    i64::MAX as u64,
    i64::MAX as u64 + 1,
    1 << 63,
    u64::MAX - 1,
    u64::MAX,
];

/// The `i32` values that break decoder arithmetic (same reasoning as
/// [`EXTREME_I64`], for the 32-bit fields in binary artifact headers).
pub const EXTREME_I32: &[i32] = &[
    i32::MIN,
    i32::MIN + 1,
    -2,
    -1,
    0,
    1,
    2,
    i32::MAX - 1,
    i32::MAX,
];

/// The `u32` values that break decoder arithmetic.
pub const EXTREME_U32: &[u32] = &[
    0,
    1,
    2,
    i32::MAX as u32,
    i32::MAX as u32 + 1,
    1 << 31,
    u32::MAX - 1,
    u32::MAX,
];

/// Assert `decode` is total over `inputs`: no panic, no overflow abort.
///
/// The sweep does not check *what* comes back — a decoder's own tests pin its
/// values against the upstream definition. It checks that an answer comes back
/// at all, for inputs no test author thinks to write down. Every offending
/// input is reported together, so one run tells you whether you have one bug or
/// a whole unguarded branch.
///
/// `name` names the decoder in the failure message; use the function's own
/// path (`chromium::logins::webkit_or_time_t`) so a failure points at the file.
pub fn assert_total<I, T>(name: &str, inputs: &[I], decode: impl Fn(I) -> T)
where
    I: Copy + Debug,
{
    let mut failed = Vec::new();
    for &input in inputs {
        // AssertUnwindSafe: `decode` is a pure function of `input` and nothing
        // observes it after a panic -- the sweep reports and aborts.
        if catch_unwind(AssertUnwindSafe(|| decode(input))).is_err() {
            failed.push(format!("{input:?}"));
        }
    }
    assert!(
        failed.is_empty(),
        "{name} panicked on {} of {} boundary input(s): {}\n\
         A decoder reads attacker-controlled evidence and must be total: return \
         an explicit absence for a value it cannot decode, never panic. \
         (unsigned_abs instead of abs, checked_/wrapping_ arithmetic, \
         div_euclid instead of /.)",
        failed.len(),
        inputs.len(),
        failed.join(", "),
    );
}

/// [`assert_total`] for a decoder that takes two integers, swept over the
/// cross product of both input sets.
pub fn assert_total2<A, B, T>(name: &str, a: &[A], b: &[B], decode: impl Fn(A, B) -> T)
where
    A: Copy + Debug,
    B: Copy + Debug,
{
    let mut failed = Vec::new();
    for &x in a {
        for &y in b {
            if catch_unwind(AssertUnwindSafe(|| decode(x, y))).is_err() {
                failed.push(format!("({x:?}, {y:?})"));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "{name} panicked on {} of {} boundary input pair(s): {}",
        failed.len(),
        a.len() * b.len(),
        failed.join(", "),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_total_decoder_passes() {
        assert_total("unsigned_abs", EXTREME_I64, |v: i64| v.unsigned_abs());
        assert_total("saturating", EXTREME_U64, |v: u64| v.saturating_mul(100));
        assert_total2("pair", EXTREME_I32, EXTREME_U32, |a: i32, b: u32| {
            (a as i64) + (b as i64)
        });
    }

    /// The exact shape of the logins.rs bug: `abs()` on a value read from an
    /// evidence cell. Overflow checks are on in the test profile, so this
    /// panics -- which is the point.
    #[test]
    fn the_abs_overflow_is_caught() {
        let result = catch_unwind(AssertUnwindSafe(|| {
            assert_total("abs", EXTREME_I64, |v: i64| v.abs());
        }));
        assert!(result.is_err(), "sweep should have failed on i64::MIN");
    }

    #[test]
    fn every_extreme_set_contains_its_type_limits() {
        assert!(EXTREME_I64.contains(&i64::MIN) && EXTREME_I64.contains(&i64::MAX));
        assert!(EXTREME_U64.contains(&0) && EXTREME_U64.contains(&u64::MAX));
        assert!(EXTREME_I32.contains(&i32::MIN) && EXTREME_I32.contains(&i32::MAX));
        assert!(EXTREME_U32.contains(&0) && EXTREME_U32.contains(&u32::MAX));
    }
}
