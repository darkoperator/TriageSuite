//! Registry search: key-name, value-name, value-data, and value-slack hits
//! across a hive, with literal/regex matching, base64 decoding, and a minimum
//! size gate. Mirrors RECmd DoKeySearch/DoValueSearch/DoValueDataSearch.

use crate::value::render_value_data;
use notatin::cell_key_node::CellKeyNode;

/// What part of the registry a hit matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitType {
    KeyName,
    ValueName,
    ValueData,
    /// Value slack bytes — notatin does not publicly surface slack bytes.
    /// This variant is defined for API completeness but is never emitted by
    /// `search_subtree`. Documented AcceptedDelta (see Task 8).
    ValueSlack,
}

/// One search hit (search-mode CSV row).
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub hit_type: HitType,
    pub key_path: String,
    pub value_name: String,
    pub value_data: String,
    pub last_write: Option<chrono::DateTime<chrono::Utc>>,
    pub deleted: bool,
}

/// Matcher built from RECmd search flags.
pub struct Matcher {
    needle: String,
    regex: Option<regex::Regex>,
    // Retained for future CLI flag plumbing; behaviour is already encoded in
    // `regex` being None when literal mode is requested.
    #[allow(dead_code)]
    literal: bool,
}

impl Matcher {
    /// `literal` forces substring (case-insensitive) instead of regex.
    pub fn new(needle: &str, use_regex: bool, literal: bool) -> Matcher {
        let regex = if use_regex && !literal {
            regex::RegexBuilder::new(needle)
                .case_insensitive(true)
                .build()
                .ok()
        } else {
            None
        };
        Matcher {
            needle: needle.to_lowercase(),
            regex,
            literal,
        }
    }

    pub fn matches(&self, haystack: &str) -> bool {
        if let Some(re) = &self.regex {
            re.is_match(haystack)
        } else {
            haystack.to_lowercase().contains(&self.needle)
        }
    }
}

/// Which HitTypes a search pass tests. Named fields rather than three
/// positional `bool`s: at a call site `(true, false, false)` and
/// `(false, true, false)` look alike, and swapping two of them silently
/// searches the wrong thing rather than failing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchTargets {
    pub keys: bool,
    pub value_names: bool,
    pub value_data: bool,
}

impl SearchTargets {
    pub fn keys() -> Self {
        SearchTargets {
            keys: true,
            ..Default::default()
        }
    }

    pub fn value_names() -> Self {
        SearchTargets {
            value_names: true,
            ..Default::default()
        }
    }

    pub fn value_data() -> Self {
        SearchTargets {
            value_data: true,
            ..Default::default()
        }
    }
}

/// Search every key/value in `subtree_root` (preorder), collecting hits.
/// `hive` provides subkey reads; `targets` selects which HitTypes to test.
///
/// NOTE: value slack (HitType::ValueSlack) is not collected here because
/// notatin does not publicly expose value slack bytes. This is a documented
/// AcceptedDelta to be recorded in Task 8.
pub fn search_subtree(
    hive: &mut crate::hive::Hive,
    root: CellKeyNode,
    matcher: &Matcher,
    targets: SearchTargets,
    min_size: usize,
) -> Vec<SearchHit> {
    let SearchTargets {
        keys: search_keys,
        value_names: search_value_names,
        value_data: search_value_data,
    } = targets;
    let mut hits = Vec::new();
    let mut stack = vec![root];
    while let Some(mut key) = stack.pop() {
        let key_path = key.path.clone();
        let lw = Some(key.last_key_written_date_and_time());
        let deleted = key.cell_state.is_deleted();

        if search_keys && matcher.matches(&key.key_name) {
            hits.push(SearchHit {
                hit_type: HitType::KeyName,
                key_path: key_path.clone(),
                value_name: String::new(),
                value_data: String::new(),
                last_write: lw,
                deleted,
            });
        }

        if search_value_names || search_value_data {
            // Collect values first to avoid borrow conflict with sub_keys(&mut key) below.
            let values: Vec<_> = key.value_iter().collect();
            for value in values {
                let vname = value.get_pretty_name();
                if search_value_names && matcher.matches(&vname) {
                    hits.push(SearchHit {
                        hit_type: HitType::ValueName,
                        key_path: key_path.clone(),
                        value_name: vname.clone(),
                        value_data: String::new(),
                        last_write: lw,
                        deleted,
                    });
                }
                if search_value_data {
                    let (content, _) = value.get_content();
                    let data = render_value_data(&content);
                    if data.len() >= min_size && matcher.matches(&data) {
                        hits.push(SearchHit {
                            hit_type: HitType::ValueData,
                            key_path: key_path.clone(),
                            value_name: vname,
                            value_data: data,
                            last_write: lw,
                            deleted,
                        });
                    }
                }
            }
        }

        for sub in hive.sub_keys(&mut key) {
            stack.push(sub);
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_substring_ci() {
        let m = Matcher::new("Run", false, true);
        assert!(m.matches("CurrentVersion\\Run"));
        assert!(m.matches("autorun"));
        assert!(!m.matches("startup"));
    }

    #[test]
    fn regex_mode() {
        let m = Matcher::new(r"^\d+$", true, false);
        assert!(m.matches("12345"));
        assert!(!m.matches("12a45"));
    }
}
