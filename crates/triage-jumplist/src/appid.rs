use std::collections::HashMap;
use std::sync::OnceLock;

const APPIDS_TXT: &str = include_str!("../../../resources/jumplist/AppIDs.txt");

/// Parse one `"appid"|"description"` line. Returns None for blank/malformed lines.
fn parse_line(line: &str) -> Option<(String, String)> {
    let (a, d) = line.split_once('|')?;
    let appid = a.trim().trim_matches('"').to_lowercase();
    let desc = d.trim().trim_matches('"').to_string();
    if appid.is_empty() {
        None
    } else {
        Some((appid, desc))
    }
}

fn builtin() -> &'static HashMap<String, String> {
    static T: OnceLock<HashMap<String, String>> = OnceLock::new();
    T.get_or_init(|| APPIDS_TXT.lines().filter_map(parse_line).collect())
}

/// Built-in lookup (case-insensitive). Miss -> "".
pub fn describe(appid: &str) -> String {
    builtin()
        .get(&appid.to_lowercase())
        .cloned()
        .unwrap_or_default()
}

/// A lookup table seeded from the built-in set, accepting user overrides
/// (from --appIds): new ids are added, existing ids are updated.
pub struct AppIdTable {
    map: HashMap<String, String>,
}

impl AppIdTable {
    pub fn with_builtin() -> Self {
        Self {
            map: builtin().clone(),
        }
    }
    pub fn add_user_entry(&mut self, appid: &str, description: &str) {
        self.map
            .insert(appid.to_lowercase(), description.to_string());
    }
    /// Load `"appid"|"description"` lines from a user file's contents.
    /// Returns the number of entries loaded.
    pub fn load_user_file(&mut self, contents: &str) -> usize {
        let mut n = 0;
        for line in contents.lines() {
            if let Some((a, d)) = parse_line(line) {
                self.map.insert(a, d);
                n += 1;
            }
        }
        n
    }
    pub fn describe(&self, appid: &str) -> String {
        self.map
            .get(&appid.to_lowercase())
            .cloned()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_appid_resolves() {
        // grep -i '0a1d19afe5a80f80' resources/jumplist/AppIDs.txt
        // => "0A1D19AFE5A80F80"|"FileZilla 2.2.32"
        assert_eq!(describe("0a1d19afe5a80f80"), "FileZilla 2.2.32");
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(describe("0A1D19AFE5A80F80"), describe("0a1d19afe5a80f80"));
        assert!(!describe("0A1D19AFE5A80F80").is_empty());
    }

    #[test]
    fn unknown_appid_is_empty() {
        assert_eq!(describe("ffffffffffffffff"), "");
    }

    #[test]
    fn user_overrides_extend_and_update() {
        let mut t = AppIdTable::with_builtin();
        t.add_user_entry("deadbeefdeadbeef", "My Custom App");
        assert_eq!(t.describe("deadbeefdeadbeef"), "My Custom App");
        // existing appid description can be updated by a user entry
        t.add_user_entry("0a1d19afe5a80f80", "Overridden");
        assert_eq!(t.describe("0a1d19afe5a80f80"), "Overridden");
    }

    #[test]
    fn load_user_file_counts_entries() {
        let mut t = AppIdTable::with_builtin();
        let n = t
            .load_user_file("\"aabbccddaabbccdd\"|\"App One\"\n\"11223344aabbccdd\"|\"App Two\"\n");
        assert_eq!(n, 2);
        assert_eq!(t.describe("aabbccddaabbccdd"), "App One");
    }
}
