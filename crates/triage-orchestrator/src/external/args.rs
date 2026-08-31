//! Shared argv-building helpers. Pure functions, no I/O — every external tool's
//! argv is unit-testable without the real binary.

use std::ffi::OsString;

pub(super) fn os<S: AsRef<std::ffi::OsStr>>(s: S) -> OsString {
    s.as_ref().to_os_string()
}

/// Push `--flag <value>` only when the value is set and non-empty. An empty string
/// is treated as unset, so `min_level = ""` emits nothing rather than an empty argv
/// entry the tool would reject.
pub(super) fn push_opt(args: &mut Vec<OsString>, flag: &str, value: &Option<String>) {
    if let Some(v) = value {
        if !v.is_empty() {
            args.push(os(flag));
            args.push(os(v));
        }
    }
}

pub(super) fn push_flag(args: &mut Vec<OsString>, flag: &str, enabled: bool) {
    if enabled {
        args.push(os(flag));
    }
}
