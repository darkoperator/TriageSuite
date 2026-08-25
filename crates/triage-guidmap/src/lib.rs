//! The EZ GuidMapping table (14,464 entries) plus a brace/case-insensitive
//! lookup. RECmd's ETW plugin and WxTTriage both resolve GUIDs through this.
//!
//! The table lives in `table.rs` (auto-generated; kept sorted ascending by GUID
//! for binary search). Each consumer decides its own not-found behavior, so the
//! lookup returns `Option` rather than a formatted fallback string.

mod table;

pub use table::GUID_MAP;

/// Look up a GUID's description. Accepts the GUID with or without surrounding
/// braces and in any case (e.g. `{6D809377-...}` or `6d809377-...`). Returns
/// `None` when the GUID is not in the table.
pub fn description_for(guid: &str) -> Option<&'static str> {
    let stripped = guid
        .trim_start_matches('{')
        .trim_end_matches('}')
        .to_lowercase();
    GUID_MAP
        .binary_search_by(|(k, _)| (*k).cmp(stripped.as_str()))
        .ok()
        .map(|i| GUID_MAP[i].1)
}

#[cfg(test)]
mod lib_tests {
    use super::*;

    #[test]
    fn lookup_is_brace_and_case_insensitive() {
        // Pick a stable entry from the table for the spot check.
        let braced = "{00000300-0000-0000-c000-000000000046}";
        let bare_upper = "00000300-0000-0000-C000-000000000046";
        assert_eq!(description_for(braced), Some("StdOleLink"));
        assert_eq!(description_for(bare_upper), Some("StdOleLink"));
    }

    #[test]
    fn unknown_guid_is_none() {
        assert_eq!(
            description_for("{ffffffff-ffff-ffff-ffff-ffffffffffff}"),
            None
        );
    }
}
