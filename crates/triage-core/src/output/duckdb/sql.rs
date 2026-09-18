//! SQL text construction: two quoting rules and two naming rules.
//!
//! A string literal and an identifier are quoted by different characters,
//! and the two functions here are deliberately separate so a caller cannot
//! reach for the wrong one. Every path, every `types` key and every injected
//! literal value goes through `quote_literal`; every view and column name
//! goes through `quote_ident`.

use std::collections::BTreeSet;

/// `abc` -> `'abc'`, with embedded `'` doubled. Double quotes are data here
/// and pass through untouched.
pub fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// `abc` -> `"abc"`, with embedded `"` doubled. Single quotes are data here
/// and pass through untouched.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `<binary_name>_<dataset>`, lowercased, every character outside
/// `[a-z0-9_]` replaced with `_`, and prefixed with `_` if it would
/// otherwise start with a digit.
///
/// Collisions are not resolved here: the caller owns the set of names in
/// play and resolves them with [`NameAllocator`], so that a collision is
/// recorded in the inventory rather than silently absorbed.
pub fn view_name(binary_name: &str, dataset: &str) -> String {
    let raw = format!("{binary_name}_{dataset}").to_lowercase();
    let mut out: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// Hands out names that do not collide with anything already in play.
///
/// Comparison is case-insensitive because DuckDB resolves identifiers case
/// insensitively: a dataset with a `HOST` column and an injected `host` one
/// would be ambiguous, not distinct.
pub struct NameAllocator {
    taken: BTreeSet<String>,
}

impl NameAllocator {
    pub fn new(taken: impl IntoIterator<Item = String>) -> Self {
        NameAllocator {
            taken: taken.into_iter().map(|n| n.to_lowercase()).collect(),
        }
    }

    /// `desired` if free, else `desired_1`, `desired_2`, ... The returned
    /// name is itself marked taken, so repeated calls never collide.
    pub fn allocate(&mut self, desired: &str) -> String {
        let mut candidate = desired.to_string();
        let mut suffix = 1u32;
        while !self.taken.insert(candidate.to_lowercase()) {
            candidate = format!("{desired}_{suffix}");
            suffix = suffix.saturating_add(1);
        }
        candidate
    }
}
