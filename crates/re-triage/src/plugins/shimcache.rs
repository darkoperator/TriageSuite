//! RECmd AppCompatCache plugin support. The Win10 ShimCache parser lives in the
//! shared `triage-appcompat` crate and the RECmd literal timestamp format in
//! `triage_core::timestamp`; this module re-exports the parser for the plugin.

pub use triage_appcompat::{filetime_to_utc, parse_win10, ShimEntry};

#[cfg(test)]
mod tests {
    use super::*;
    use triage_core::timestamp::dt_to_recmd_literal;

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
