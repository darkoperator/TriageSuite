//! The search box: Timeline Explorer's search syntax.
//!
//! Tokens are separated by whitespace outside double quotes, and `"a
//! phrase"` is one term. A leading `+` puts a term in `all`, a leading `-` in
//! `none`, anything else in `any`. `Name:rest` scopes the term to the column
//! `Name` when the relation has one (compared without regard to case);
//! otherwise the whole token is plain text, so `http://x` is a term, not a
//! search of a column called `http`. A row matches when (`any` is empty or
//! one `any` term matches), every `all` term matches and no `none` term does.
//!
//! An unscoped term matches when one of the searchable columns contains it:
//! every catalogued column whose role is data or host. The run id, identity
//! and output-file metadata columns are left out, because their values (a
//! run stamp, a file path) would match a search by accident.
//!
//! The search text is untrusted input. Every term reaches the SQL as a bound
//! parameter; the only names written into the SQL are catalogued columns,
//! through `quote_ident`. Like a filter leaf, every predicate is wrapped in
//! `COALESCE(…, FALSE)`, so `-term` is the exact complement of `+term`.

use crate::catalog::{ColumnInfo, ColumnRole};
use crate::error::{EngineError, Result};
use crate::filter::{literal_sql, null_safe, regex_sql, LiteralMatch, SqlFragment};
use duckdb::types::Value as SqlValue;
use triage_core::output::duckdb::sql::quote_ident;

/// Characters in one search.
pub const MAX_SEARCH: usize = 2_000;
/// Terms in one search.
pub const MAX_TERMS: usize = 50;

/// One term: text to find, in one column or in every searchable column.
#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    pub text: String,
    /// The catalogued column's name, when the term was scoped to one.
    pub column: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchExpr {
    pub any: Vec<Term>,
    pub all: Vec<Term>,
    pub none: Vec<Term>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Search {
    Terms(SearchExpr),
    /// The whole input, tested against each searchable cell on its own, so
    /// `^` and `$` anchor to a cell.
    Regex(String),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SearchOptions {
    pub regex: bool,
    pub case_sensitive: bool,
}

/// Parse the search box's text against `columns`, the catalogued columns of
/// the relation it will run over. `Ok(None)` when there is nothing to search
/// for: blank input, or only empty phrases. `Invalid` over `MAX_SEARCH`
/// characters or `MAX_TERMS` terms, or for a regex that does not compile.
pub fn parse_search(
    input: &str,
    opts: SearchOptions,
    columns: &[ColumnInfo],
) -> Result<Option<Search>> {
    check_length(input)?;
    if input.trim().is_empty() {
        return Ok(None);
    }
    if opts.regex {
        check_regex(input, opts)?;
        return Ok(Some(Search::Regex(input.to_string())));
    }
    let mut expr = SearchExpr::default();
    for token in tokens(input) {
        let Some((bucket, term)) = term(&token, columns) else {
            continue;
        };
        match bucket {
            Bucket::Any => expr.any.push(term),
            Bucket::All => expr.all.push(term),
            Bucket::None => expr.none.push(term),
        }
    }
    check_terms(&expr)?;
    if expr == SearchExpr::default() {
        return Ok(None);
    }
    Ok(Some(Search::Terms(expr)))
}

/// Compile `search` to a boolean expression over `columns`. The caps and the
/// regex are checked again, so a `Search` built in code rather than parsed
/// cannot get past them; a scoped term naming a column `columns` lacks is
/// `Invalid`.
pub fn compile_search(
    search: &Search,
    opts: SearchOptions,
    columns: &[ColumnInfo],
) -> Result<SqlFragment> {
    let searchable: Vec<&ColumnInfo> = columns
        .iter()
        .filter(|c| matches!(c.role, ColumnRole::Data | ColumnRole::Host))
        .collect();
    let mut params = Vec::new();
    let sql = match search {
        Search::Regex(pattern) => {
            check_length(pattern)?;
            check_regex(pattern, opts)?;
            any_cell_regex_sql(pattern, &searchable, opts, &mut params)
        }
        Search::Terms(expr) => {
            check_terms(expr)?;
            let mut parts = Vec::new();
            if !expr.any.is_empty() {
                let any = expr
                    .any
                    .iter()
                    .map(|t| term_sql(t, columns, &searchable, opts, &mut params))
                    .collect::<Result<Vec<_>>>()?;
                parts.push(format!("({})", any.join(" OR ")));
            }
            for t in &expr.all {
                parts.push(term_sql(t, columns, &searchable, opts, &mut params)?);
            }
            for t in &expr.none {
                parts.push(format!(
                    "NOT {}",
                    term_sql(t, columns, &searchable, opts, &mut params)?
                ));
            }
            if parts.is_empty() {
                "TRUE".to_string()
            } else {
                format!("({})", parts.join(" AND "))
            }
        }
    };
    Ok(SqlFragment { sql, params })
}

fn check_length(text: &str) -> Result<()> {
    if text.chars().count() > MAX_SEARCH {
        return Err(EngineError::Invalid(format!(
            "a search is at most {MAX_SEARCH} characters"
        )));
    }
    Ok(())
}

fn check_terms(expr: &SearchExpr) -> Result<()> {
    let terms = || expr.any.iter().chain(&expr.all).chain(&expr.none);
    if terms().count() > MAX_TERMS {
        return Err(EngineError::Invalid(format!(
            "a search has at most {MAX_TERMS} terms"
        )));
    }
    if terms().map(|t| t.text.chars().count()).sum::<usize>() > MAX_SEARCH {
        return Err(EngineError::Invalid(format!(
            "a search is at most {MAX_SEARCH} characters"
        )));
    }
    Ok(())
}

/// Refuse a pattern the `regex` crate cannot compile. DuckDB runs the
/// pattern with RE2, whose dialect differs slightly, so this catches the
/// common mistakes rather than every one.
fn check_regex(pattern: &str, opts: SearchOptions) -> Result<()> {
    regex::RegexBuilder::new(pattern)
        .case_insensitive(!opts.case_sensitive)
        .build()
        .map(|_| ())
        .map_err(|e| EngineError::Invalid(format!("the search is not a valid regex: {e}")))
}

/// One character of a token, or a double quote (which is dropped from the
/// text but still says the token is a phrase).
#[derive(Clone, Copy, PartialEq)]
enum Piece {
    Char { c: char, quoted: bool },
    Quote,
}

/// Split `input` at whitespace outside double quotes. An unterminated quote
/// runs to the end of the input.
fn tokens(input: &str) -> Vec<Vec<Piece>> {
    let mut out = Vec::new();
    let mut current = Vec::new();
    let mut quoted = false;
    for c in input.chars() {
        if c == '"' {
            quoted = !quoted;
            current.push(Piece::Quote);
        } else if !quoted && c.is_whitespace() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else {
            current.push(Piece::Char { c, quoted });
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

enum Bucket {
    Any,
    All,
    None,
}

fn text_of(pieces: &[Piece]) -> String {
    pieces
        .iter()
        .filter_map(|p| match p {
            Piece::Char { c, .. } => Some(*c),
            Piece::Quote => None,
        })
        .collect()
}

/// The term one token stands for; `None` when it has no text (`""`, `+""`).
/// A token no scope applies to is plain text as a whole, so `EventId:` and
/// `EventId:""` (a scope with nothing after the colon) are the unscoped
/// term `EventId:`.
fn term(token: &[Piece], columns: &[ColumnInfo]) -> Option<(Bucket, Term)> {
    // A `+` or `-` with nothing after it is the character itself.
    let (bucket, rest) = match token {
        [Piece::Char {
            c: '+',
            quoted: false,
        }, rest @ ..]
            if !rest.is_empty() =>
        {
            (Bucket::All, rest)
        }
        [Piece::Char {
            c: '-',
            quoted: false,
        }, rest @ ..]
            if !rest.is_empty() =>
        {
            (Bucket::None, rest)
        }
        _ => (Bucket::Any, token),
    };
    let term = scoped(rest, columns).unwrap_or_else(|| Term {
        text: text_of(rest),
        column: None,
    });
    (!term.text.is_empty()).then_some((bucket, term))
}

/// `Name:rest` as a term of column `Name`, when the name before the first
/// unquoted colon is plain text naming a catalogued column and `rest` has
/// text; otherwise `None`, and the caller reads the whole token as text.
fn scoped(token: &[Piece], columns: &[ColumnInfo]) -> Option<Term> {
    let colon = token.iter().position(|p| {
        *p == Piece::Char {
            c: ':',
            quoted: false,
        }
    })?;
    let (name, rest) = (&token[..colon], &token[colon + 1..]);
    if name.is_empty()
        || name
            .iter()
            .any(|p| !matches!(p, Piece::Char { quoted: false, .. }))
    {
        return None;
    }
    let name = text_of(name).to_lowercase();
    let column = columns.iter().find(|c| c.name.to_lowercase() == name)?;
    let text = text_of(rest);
    (!text.is_empty()).then(|| Term {
        text,
        column: Some(column.name.clone()),
    })
}

/// `term` as a substring test, with its text pushed as a parameter.
fn term_sql(
    term: &Term,
    columns: &[ColumnInfo],
    searchable: &[&ColumnInfo],
    opts: SearchOptions,
    params: &mut Vec<SqlValue>,
) -> Result<String> {
    let mut test = |haystack: String| {
        null_safe(&literal_sql(
            &haystack,
            &term.text,
            LiteralMatch::Contains,
            opts.case_sensitive,
            params,
        ))
    };
    let texts = || searchable.iter().map(|c| quote_ident(&c.text_column));
    Ok(match &term.column {
        Some(name) => {
            let column = columns.iter().find(|c| c.name == *name).ok_or_else(|| {
                EngineError::Invalid(format!("search names an unknown column \"{name}\""))
            })?;
            test(quote_ident(&column.text_column))
        }
        None if searchable.is_empty() => "FALSE".into(),
        // A term holding the separator could match across two joined
        // cells, so it is tested against each cell on its own.
        None if term.text.contains(SEPARATOR) => {
            format!("({})", texts().map(test).collect::<Vec<_>>().join(" OR "))
        }
        // One test over the cells joined by the unit separator, instead of
        // one test per column.
        None => test(format!(
            "concat_ws(chr({}), {})",
            u32::from(SEPARATOR),
            texts().collect::<Vec<_>>().join(", ")
        )),
    })
}

/// What an unscoped term's cells are joined with: the ASCII unit separator,
/// which evidence text almost never holds.
const SEPARATOR: char = '\u{1f}';

/// `pattern` tested against each searchable cell on its own.
fn any_cell_regex_sql(
    pattern: &str,
    searchable: &[&ColumnInfo],
    opts: SearchOptions,
    params: &mut Vec<SqlValue>,
) -> String {
    if searchable.is_empty() {
        return "FALSE".into();
    }
    let tests: Vec<String> = searchable
        .iter()
        .map(|c| {
            null_safe(&regex_sql(
                &quote_ident(&c.text_column),
                pattern,
                opts.case_sensitive,
                params,
            ))
        })
        .collect();
    format!("({})", tests.join(" OR "))
}
