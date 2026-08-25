//! RECmd AppCompatCache plugin support. The Win10 ShimCache parser now lives in
//! the shared `triage-appcompat` crate; this module re-exports it and keeps the
//! RECmd-specific literal timestamp formatter used by the plugin.

pub use triage_appcompat::{filetime_to_utc, parse_win10, ShimEntry};

use chrono::DateTime;

/// Format a DateTime<Utc> as RECmd's literal "yyyy-MM-dd HH:mm:ss.fffffff" (7 digits).
pub fn dt_to_recmd_literal(dt: DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{ticks:07}", dt.format("%Y-%m-%d %H:%M:%S"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recmd_literal_7_digits() {
        let ft: u64 = 0x01dad8578959b0c1;
        let dt = filetime_to_utc(ft).unwrap();
        assert_eq!(dt_to_recmd_literal(dt), "2024-07-17 14:42:23.8961857");
    }

    #[test]
    fn recmd_literal_entry1() {
        let ft: u64 = 0x01dad7ea3405716e;
        let dt = filetime_to_utc(ft).unwrap();
        assert_eq!(dt_to_recmd_literal(dt), "2024-07-17 01:39:45.5941998");
    }
}
