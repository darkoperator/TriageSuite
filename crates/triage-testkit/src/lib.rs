//! Zimmerman-compatibility comparison harness (spec section 10.3).
//!
//! Compares TriageSuite CSV/NDJSON output against checked-in reference
//! output from the corresponding Zimmerman tool. The only divergences it
//! normalizes silently are the ones the suite specification declares
//! intentional: timestamp rendering (reference `2026-03-11 21:20:14.1234567`
//! vs suite ISO 8601 `2026-03-11T21:20:14.1234567Z`) and host path prefixes
//! (path-valued fields are compared by basename). Everything else must
//! either match exactly or be covered by a named [`AcceptedDelta`]; the
//! harness fails on any undocumented divergence.

use std::collections::BTreeMap;
use std::path::Path;

/// Row map type used by delta row guards: a header→value map for a single row.
pub type RowMap = std::collections::BTreeMap<String, String>;

/// A named, documented divergence from byte-identical compatibility.
pub struct AcceptedDelta {
    /// Column (CSV) / property (JSON) name this applies to.
    pub field: &'static str,
    /// Why the divergence is accepted.
    pub reason: &'static str,
    /// How to compare the two values; return true when acceptable.
    pub compare: fn(reference: &str, ours: &str) -> bool,
    /// Optional whole-row guard: when `Some`, the delta applies ONLY if this
    /// returns `true` for the current row (a header→value map). `None` means
    /// the delta applies to every row (backward-compatible default). Use this
    /// to structurally scope a delta to a specific context — e.g. only rows
    /// whose `TargetIDAbsolutePath` starts with `CLSID_SearchFolder`.
    ///
    /// The guard receives the **reference** row values (not ours), because
    /// guards classify row context and the reference is the authoritative
    /// source — our output may be empty for a column we haven't parsed yet.
    pub row_guard: Option<fn(&RowMap) -> bool>,
}

/// Normalize a PECmd-style timestamp ("2026-03-11 21:20:14.1234567",
/// UTC, no zone marker) to suite ISO 8601 ("2026-03-11T21:20:14.1234567Z").
/// Empty stays empty; non-timestamp strings pass through unchanged.
pub fn normalize_reference_timestamp(s: &str) -> String {
    let b = s.as_bytes();
    // Accept both the PECmd CSV form (date/time separated by a space) and the
    // ISO/`DateTimeOffset` form (separated by `T`), e.g. LECmd's JSON output.
    let looks_like_ts = b.len() >= 19
        && b[4] == b'-'
        && b[7] == b'-'
        && (b[10] == b' ' || b[10] == b'T')
        && b[13] == b':'
        && b[16] == b':';
    if !looks_like_ts {
        return s.to_string();
    }
    let mut out = s.replacen(' ', "T", 1);
    // Some reference renderers (LECmd's JSON via .NET DateTimeOffset) emit an
    // explicit UTC offset `+00:00` instead of the `Z` zone marker. Both denote
    // the same instant; normalize the offset form to `Z` so it compares equal
    // to the suite's ISO 8601 output.
    if let Some(stripped) = out.strip_suffix("+00:00") {
        out = stripped.to_string();
    }
    if !out.ends_with('Z') {
        out.push('Z');
    }
    out
}

/// Result of comparing one reference file against one of ours.
pub struct Diff {
    pub header_ok: bool,
    pub reference_rows: usize,
    pub our_rows: usize,
    pub mismatches: Vec<String>,
    /// Total count of "row missing from our output" entries where the
    /// occurrence index in the key is #2 or higher (i.e. duplicate
    /// occurrences that are legitimately absent from our deduplicated output).
    /// Counted over ALL mismatches — not capped. Only meaningful when
    /// `compare_csv_grouped` (occurrence_index=true) was used; 0 otherwise.
    pub missing_dup_occurrences: usize,
}

impl Diff {
    pub fn is_match(&self) -> bool {
        self.header_ok && self.reference_rows == self.our_rows && self.mismatches.is_empty()
    }
}

/// Cap on stored mismatch strings; all mismatches are still counted.
const MISMATCH_CAP: usize = 20;

/// Mismatch accumulator: stores at most [`MISMATCH_CAP`] entries but keeps
/// an exact total and a full count of duplicate-occurrence missing rows.
struct Mismatches {
    kept: Vec<String>,
    total: usize,
    /// Count of "row missing" entries for occurrence index #2+ (dup rows).
    /// Tracked over ALL mismatches, never capped.
    missing_dup_occurrences: usize,
}

impl Mismatches {
    fn new() -> Self {
        Self {
            kept: Vec::new(),
            total: 0,
            missing_dup_occurrences: 0,
        }
    }

    fn push(&mut self, m: String) {
        self.total += 1;
        // Track dup-occurrence missing rows regardless of display cap.
        // A dup occurrence is a "row missing from our output" entry whose
        // key ends in "#N]" with N >= 2 (i.e. "[...#2] row missing ...",
        // "[...#3] row missing ...", etc.).
        if m.contains("] row missing from our output") {
            // Extract the occurrence index from the key suffix "#N]".
            if let Some(hash_pos) = m.rfind('#') {
                let after_hash = &m[hash_pos + 1..];
                if let Some(bracket_pos) = after_hash.find(']') {
                    if let Ok(n) = after_hash[..bracket_pos].parse::<usize>() {
                        if n >= 2 {
                            self.missing_dup_occurrences += 1;
                        }
                    }
                }
            }
        }
        if self.kept.len() < MISMATCH_CAP {
            self.kept.push(m);
        }
    }

    fn finish(self) -> (Vec<String>, usize) {
        let mut v = self.kept;
        if self.total > MISMATCH_CAP {
            v.push(format!("...and {} more", self.total - MISMATCH_CAP));
        }
        (v, self.missing_dup_occurrences)
    }
}

/// Final path component, splitting on both `/` and `\` so host paths and
/// Windows artifact paths compare alike.
fn basename(s: &str) -> String {
    s.rsplit(['/', '\\']).next().unwrap_or(s).to_string()
}

/// True when a named accepted delta covers this field, passes the optional
/// row guard, and accepts the (reference, ours) value pair.
///
/// An [`AcceptedDelta`] with `field == ""` is a **whole-row wildcard**: it
/// applies to ANY field on rows where the guard returns `true`. This lets a
/// single delta document a parse-error row where EVERY column may diverge,
/// rather than listing one delta per affected column.
fn accepted_ok(
    accepted: &[AcceptedDelta],
    field: &str,
    reference: &str,
    ours: &str,
    row: &RowMap,
) -> bool {
    // Try ALL AcceptedDeltas for this field (or wildcard "") in order.  A
    // delta is tried only when its row_guard passes (or it has no guard); the
    // first delta whose compare() function returns true wins.  Using `.find()`
    // on the first guard match would stop too early when multiple deltas share
    // the same field but have different row guards (e.g. TypedURLs ValueData3
    // vs RecentDocs ValueData3 vs OpenSave ValueData3).
    accepted
        .iter()
        .filter(|d| d.field == field || d.field.is_empty())
        .any(|d| {
            if let Some(guard) = d.row_guard {
                if !guard(row) {
                    return false;
                }
            }
            (d.compare)(reference, ours)
        })
}

fn read_csv(path: &Path) -> (Vec<String>, Vec<Vec<String>>) {
    let mut rdr = csv::Reader::from_path(path)
        .unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()));
    let headers: Vec<String> = rdr
        .headers()
        .unwrap_or_else(|e| panic!("cannot read headers of {}: {e}", path.display()))
        .iter()
        .map(str::to_string)
        .collect();
    let rows: Vec<Vec<String>> = rdr
        .records()
        .map(|r| {
            r.unwrap_or_else(|e| panic!("bad CSV record in {}: {e}", path.display()))
                .iter()
                .map(str::to_string)
                .collect()
        })
        .collect();
    (headers, rows)
}

fn header_mismatch(reference: &[String], ours: &[String]) -> Diff {
    Diff {
        header_ok: false,
        reference_rows: 0,
        our_rows: 0,
        mismatches: vec![format!(
            "header mismatch:\n  reference: {reference:?}\n  ours:      {ours:?}"
        )],
        missing_dup_occurrences: 0,
    }
}

/// Key extractor: second-to-last path segment (often a SID dir) + basename
/// of the first listed key column. Stable across host path-prefix differences.
///
/// Invariant: the produced key must be unique per row within each file. If two
/// reference rows collapse to the same key, the comparison reports a confusing
/// "missing from our output" mismatch rather than a clean duplicate error — so
/// pick key columns whose (parent-segment, basename) pair is unique (e.g. `$I`
/// names under a SID directory are).
pub fn composite_sid_basename(
    row: &std::collections::BTreeMap<String, String>,
    key_columns: &[&str],
) -> String {
    let v = key_columns
        .first()
        .and_then(|c| row.get(*c))
        .cloned()
        .unwrap_or_default();
    let segs: Vec<&str> = v.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let base = segs.last().copied().unwrap_or("");
    let parent = if segs.len() >= 2 {
        segs[segs.len() - 2]
    } else {
        ""
    };
    format!("{parent}/{base}")
}

/// Shared internal implementation for both `compare_csv` and
/// `compare_csv_composite`. `path_columns` names every column whose values
/// are paths and should be compared by basename. `key_fn` derives the lookup
/// key for a row given a header→value map and the path-column slice.
fn compare_csv_inner(
    reference_path: &Path,
    ours_path: &Path,
    path_columns: &[&str],
    key_fn: fn(&BTreeMap<String, String>, &[&str]) -> String,
    accepted: &[AcceptedDelta],
    occurrence_index: bool,
) -> Diff {
    let (ref_headers, ref_rows) = read_csv(reference_path);
    let (our_headers, our_rows) = read_csv(ours_path);
    if ref_headers != our_headers {
        return header_mismatch(&ref_headers, &our_headers);
    }

    /// Build a header→value map for a row (used only for key_fn).
    fn row_map(headers: &[String], row: &[String]) -> BTreeMap<String, String> {
        headers
            .iter()
            .zip(row.iter())
            .map(|(h, v)| (h.clone(), v.clone()))
            .collect()
    }

    // When `occurrence_index` is set, the base key is a GROUP key that may
    // legitimately repeat (e.g. several LNKs under one Custom category share the
    // same SourceFile+EntryName). We disambiguate by appending a per-group
    // running index in row order — honest as long as BOTH sides preserve the
    // same intra-file row order, which the aggregation guarantees. Without it,
    // the base key must already be unique per row.
    let mut occ: BTreeMap<String, usize> = BTreeMap::new();
    let next_key = |base: String, counts: &mut BTreeMap<String, usize>| -> String {
        if occurrence_index {
            let n = counts.entry(base.clone()).or_insert(0);
            *n += 1;
            format!("{base}#{n}")
        } else {
            base
        }
    };

    let mut mm = Mismatches::new();
    let our_count = our_rows.len();
    let mut ours_by_key: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in our_rows {
        let base = key_fn(&row_map(&our_headers, &row), path_columns);
        let key = next_key(base, &mut occ);
        if ours_by_key.insert(key.clone(), row).is_some() {
            mm.push(format!("[{key}] duplicate key in our output"));
        }
    }
    occ.clear();

    for ref_row in &ref_rows {
        let base = key_fn(&row_map(&ref_headers, ref_row), path_columns);
        let key = next_key(base, &mut occ);
        let Some(our_row) = ours_by_key.remove(&key) else {
            mm.push(format!("[{key}] row missing from our output"));
            continue;
        };
        // Build a reference row map for guard evaluation. The guard classifies
        // which CONTEXT the row belongs to (e.g. "is this a SearchFolder row?").
        // The reference is the authoritative source for that context: when our
        // parser hasn't fully decoded a shell item yet, our output may be empty
        // for the very column the guard inspects, so checking our_row would
        // prevent the guard from ever firing. The reference value is stable
        // and describes the row's identity, which is exactly what guards need.
        let ref_row_map = row_map(&ref_headers, ref_row);
        for (i, field) in ref_headers.iter().enumerate() {
            let rv = normalize_reference_timestamp(&ref_row[i]);
            let ov = &our_row[i];
            let ok = if path_columns.contains(&field.as_str()) {
                basename(&rv) == basename(ov)
            } else {
                rv == *ov || accepted_ok(accepted, field, &rv, ov, &ref_row_map)
            };
            if !ok {
                mm.push(format!("[{key}] {field}: reference={rv:?} ours={ov:?}"));
            }
        }
    }
    for key in ours_by_key.keys() {
        mm.push(format!("[{key}] extra row in our output"));
    }

    let (mismatches, missing_dup_occurrences) = mm.finish();
    Diff {
        header_ok: true,
        reference_rows: ref_rows.len(),
        our_rows: our_count,
        mismatches,
        missing_dup_occurrences,
    }
}

/// Compare two CSV files (headers exact; rows keyed by the basename of
/// `key_column`; reference timestamps normalized; accepted deltas applied).
pub fn compare_csv(
    reference_path: &Path,
    ours_path: &Path,
    key_column: &str,
    accepted: &[AcceptedDelta],
) -> Diff {
    fn default_key_fn(row: &BTreeMap<String, String>, path_columns: &[&str]) -> String {
        path_columns
            .first()
            .and_then(|c| row.get(*c))
            .map(|v| basename(v))
            .unwrap_or_default()
    }
    compare_csv_inner(
        reference_path,
        ours_path,
        &[key_column],
        default_key_fn,
        accepted,
        false,
    )
}

/// Compare two CSV files using a caller-supplied composite key extractor.
/// `path_columns` names the columns whose values are paths; EVERY column in
/// that slice is compared by basename. `key_fn` derives the lookup key for a
/// row from a header→value map and the path-column slice. Otherwise identical
/// to [`compare_csv`]: headers exact, reference timestamps normalized, accepted
/// deltas applied.
pub fn compare_csv_composite(
    reference_path: &Path,
    ours_path: &Path,
    path_columns: &[&str],
    key_fn: fn(&BTreeMap<String, String>, &[&str]) -> String,
    accepted: &[AcceptedDelta],
) -> Diff {
    compare_csv_inner(
        reference_path,
        ours_path,
        path_columns,
        key_fn,
        accepted,
        false,
    )
}

/// Like [`compare_csv_composite`] but `key_fn` returns a GROUP key that may
/// repeat across rows; the comparison disambiguates duplicates by appending a
/// per-group running occurrence index in row order. Use when no column tuple is
/// unique per row (e.g. JLECmd Custom Destinations: several LNKs under one
/// category share the same SourceFile + EntryName). Correctness relies on BOTH
/// the reference and our output preserving the same intra-file row order — the
/// aggregation walks files in a fixed order and keeps each file's rows in
/// emission order, so occurrence N on each side denotes the same LNK.
pub fn compare_csv_grouped(
    reference_path: &Path,
    ours_path: &Path,
    path_columns: &[&str],
    key_fn: fn(&BTreeMap<String, String>, &[&str]) -> String,
    accepted: &[AcceptedDelta],
) -> Diff {
    compare_csv_inner(
        reference_path,
        ours_path,
        path_columns,
        key_fn,
        accepted,
        true,
    )
}

/// Compare two CSVs that have NO unique key column (e.g. PECmd timeline):
/// sort both row-sets after normalization and compare as ordered multisets.
/// `path_columns` names columns whose values are paths: compared by basename.
pub fn compare_csv_unkeyed(
    reference_path: &Path,
    ours_path: &Path,
    path_columns: &[&str],
    accepted: &[AcceptedDelta],
) -> Diff {
    let (ref_headers, ref_rows) = read_csv(reference_path);
    let (our_headers, our_rows) = read_csv(ours_path);
    if ref_headers != our_headers {
        return header_mismatch(&ref_headers, &our_headers);
    }
    let path_idx: Vec<usize> = ref_headers
        .iter()
        .enumerate()
        .filter(|(_, h)| path_columns.contains(&h.as_str()))
        .map(|(i, _)| i)
        .collect();

    let mut ref_norm: Vec<Vec<String>> = ref_rows
        .iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(i, v)| {
                    let v = normalize_reference_timestamp(v);
                    if path_idx.contains(&i) {
                        basename(&v)
                    } else {
                        v
                    }
                })
                .collect()
        })
        .collect();
    let mut our_norm: Vec<Vec<String>> = our_rows
        .iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(i, v)| {
                    if path_idx.contains(&i) {
                        basename(v)
                    } else {
                        v.clone()
                    }
                })
                .collect()
        })
        .collect();
    ref_norm.sort();
    our_norm.sort();

    let mut mm = Mismatches::new();
    if ref_norm.len() != our_norm.len() {
        mm.push(format!(
            "row count: reference {} vs ours {}",
            ref_norm.len(),
            our_norm.len()
        ));
    }
    // Unkeyed comparison: rows are sorted so per-row identity is lost; row
    // guards cannot fire meaningfully here. Pass an empty map — any delta with
    // a row_guard will simply not apply (guard returns false), which is safe
    // because no row-guarded delta should ever appear in an unkeyed comparer.
    let empty_row: BTreeMap<String, String> = BTreeMap::new();
    for (n, (ref_row, our_row)) in ref_norm.iter().zip(our_norm.iter()).enumerate() {
        for (i, field) in ref_headers.iter().enumerate() {
            let (rv, ov) = (&ref_row[i], &our_row[i]);
            if rv != ov && !accepted_ok(accepted, field, rv, ov, &empty_row) {
                mm.push(format!(
                    "[sorted row {n}] {field}: reference={rv:?} ours={ov:?}"
                ));
            }
        }
    }

    let (mismatches, missing_dup_occurrences) = mm.finish();
    Diff {
        header_ok: true,
        reference_rows: ref_norm.len(),
        our_rows: our_norm.len(),
        mismatches,
        missing_dup_occurrences,
    }
}

/// JSON value as the string the comparison operates on: strings raw,
/// everything else (bool/number/null) via `to_string`.
fn stringify(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn read_ndjson(path: &Path) -> Vec<serde_json::Map<String, serde_json::Value>> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            match serde_json::from_str::<serde_json::Value>(l)
                .unwrap_or_else(|e| panic!("bad JSON line in {}: {e}", path.display()))
            {
                serde_json::Value::Object(o) => o,
                other => panic!("non-object NDJSON line in {}: {other}", path.display()),
            }
        })
        .collect()
}

/// Compare NDJSON files keyed by basename of `key_prop`. Property sets must
/// match per record (a missing or extra property is a mismatch); values are
/// compared on their stringified forms with reference timestamps normalized
/// and accepted deltas applied.
pub fn compare_ndjson(
    reference_path: &Path,
    ours_path: &Path,
    key_prop: &str,
    accepted: &[AcceptedDelta],
) -> Diff {
    let ref_recs = read_ndjson(reference_path);
    let our_recs = read_ndjson(ours_path);
    let our_count = our_recs.len();

    let mut mm = Mismatches::new();
    let mut ours_by_key: BTreeMap<String, serde_json::Map<String, serde_json::Value>> =
        BTreeMap::new();
    for rec in our_recs {
        let Some(k) = rec.get(key_prop).map(stringify) else {
            mm.push(format!("our record lacks key property {key_prop:?}"));
            continue;
        };
        let key = basename(&k);
        if ours_by_key.insert(key.clone(), rec).is_some() {
            mm.push(format!("[{key}] duplicate key in our output"));
        }
    }

    for ref_rec in &ref_recs {
        let Some(k) = ref_rec.get(key_prop).map(stringify) else {
            mm.push(format!("reference record lacks key property {key_prop:?}"));
            continue;
        };
        let key = basename(&k);
        let Some(our_rec) = ours_by_key.remove(&key) else {
            mm.push(format!("[{key}] record missing from our output"));
            continue;
        };
        // Build a string row map from the REFERENCE record for guard evaluation.
        // Guards classify row context (e.g. "is this a SearchFolder record?").
        // The reference is the authoritative source: our output may be empty for
        // the very property the guard inspects when we haven't fully parsed a
        // shell-item subtype yet.
        let ref_row_map: BTreeMap<String, String> = ref_rec
            .iter()
            .map(|(k, v)| (k.clone(), stringify(v)))
            .collect();
        for (prop, rv) in ref_rec {
            let Some(ov) = our_rec.get(prop) else {
                mm.push(format!(
                    "[{key}] property {prop} missing from ours (reference: {})",
                    stringify(rv)
                ));
                continue;
            };
            let rs = normalize_reference_timestamp(&stringify(rv));
            let os = stringify(ov);
            let ok = if prop == key_prop {
                basename(&rs) == basename(&os)
            } else {
                rs == os || accepted_ok(accepted, prop, &rs, &os, &ref_row_map)
            };
            if !ok {
                mm.push(format!("[{key}] {prop}: reference={rs:?} ours={os:?}"));
            }
        }
        for prop in our_rec.keys() {
            if !ref_rec.contains_key(prop) {
                mm.push(format!(
                    "[{key}] extra property {prop} in ours (value: {})",
                    stringify(&our_rec[prop])
                ));
            }
        }
    }
    for key in ours_by_key.keys() {
        mm.push(format!("[{key}] extra record in our output"));
    }

    let (mismatches, missing_dup_occurrences) = mm.finish();
    Diff {
        header_ok: true,
        reference_rows: ref_recs.len(),
        our_rows: our_count,
        mismatches,
        missing_dup_occurrences,
    }
}

/// Like [`compare_ndjson`] but derives each record's lookup key from a
/// caller-supplied `key_fn` over a property→value map (mirrors
/// [`compare_csv_composite`]). Needed when the key property's basename is not
/// unique per record (e.g. LECmd writes many same-named LNKs under different
/// directories into one file). `path_props` names properties compared by
/// basename; the key property itself is always compared by basename.
pub fn compare_ndjson_composite(
    reference_path: &Path,
    ours_path: &Path,
    key_prop: &str,
    path_props: &[&str],
    key_fn: fn(&BTreeMap<String, String>, &[&str]) -> String,
    accepted: &[AcceptedDelta],
) -> Diff {
    compare_ndjson_inner(
        reference_path,
        ours_path,
        key_prop,
        path_props,
        key_fn,
        accepted,
        false,
    )
}

/// Like [`compare_ndjson_composite`] but `key_fn` returns a GROUP key that may
/// repeat; duplicates are disambiguated by a per-group running occurrence index
/// in record order (the NDJSON analogue of [`compare_csv_grouped`]).
pub fn compare_ndjson_grouped(
    reference_path: &Path,
    ours_path: &Path,
    key_prop: &str,
    path_props: &[&str],
    key_fn: fn(&BTreeMap<String, String>, &[&str]) -> String,
    accepted: &[AcceptedDelta],
) -> Diff {
    compare_ndjson_inner(
        reference_path,
        ours_path,
        key_prop,
        path_props,
        key_fn,
        accepted,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn compare_ndjson_inner(
    reference_path: &Path,
    ours_path: &Path,
    key_prop: &str,
    path_props: &[&str],
    key_fn: fn(&BTreeMap<String, String>, &[&str]) -> String,
    accepted: &[AcceptedDelta],
    occurrence_index: bool,
) -> Diff {
    fn key_map(rec: &serde_json::Map<String, serde_json::Value>) -> BTreeMap<String, String> {
        rec.iter().map(|(k, v)| (k.clone(), stringify(v))).collect()
    }

    let ref_recs = read_ndjson(reference_path);
    let our_recs = read_ndjson(ours_path);
    let our_count = our_recs.len();

    let mut occ: BTreeMap<String, usize> = BTreeMap::new();
    let next_key = |base: String, counts: &mut BTreeMap<String, usize>| -> String {
        if occurrence_index {
            let n = counts.entry(base.clone()).or_insert(0);
            *n += 1;
            format!("{base}#{n}")
        } else {
            base
        }
    };

    let mut mm = Mismatches::new();
    let mut ours_by_key: BTreeMap<String, serde_json::Map<String, serde_json::Value>> =
        BTreeMap::new();
    for rec in our_recs {
        let base = key_fn(&key_map(&rec), path_props);
        let key = next_key(base, &mut occ);
        if ours_by_key.insert(key.clone(), rec).is_some() {
            mm.push(format!("[{key}] duplicate key in our output"));
        }
    }
    occ.clear();

    for ref_rec in &ref_recs {
        let base = key_fn(&key_map(ref_rec), path_props);
        let key = next_key(base, &mut occ);
        let Some(our_rec) = ours_by_key.remove(&key) else {
            mm.push(format!("[{key}] record missing from our output"));
            continue;
        };
        // Build a string row map from the REFERENCE record for guard evaluation.
        // Guards classify row context (e.g. "is this a SearchFolder record?").
        // The reference is the authoritative source: our output may be empty for
        // the very property the guard inspects when we haven't fully parsed a
        // shell-item subtype yet.
        let ref_row_map = key_map(ref_rec);
        for (prop, rv) in ref_rec {
            let rs = normalize_reference_timestamp(&stringify(rv));
            let Some(ov) = our_rec.get(prop) else {
                // Our null-nuking JSON sink omits empty/None properties, so a
                // property present (and empty) in the CSV is simply absent here.
                // Treat absence as the empty string for accepted-delta purposes,
                // mirroring the CSV path's empty-cell handling.
                if accepted_ok(accepted, prop, &rs, "", &ref_row_map) {
                    continue;
                }
                mm.push(format!(
                    "[{key}] property {prop} missing from ours (reference: {})",
                    stringify(rv)
                ));
                continue;
            };
            let os = stringify(ov);
            let ok = if prop == key_prop || path_props.contains(&prop.as_str()) {
                basename(&rs) == basename(&os)
            } else {
                rs == os || accepted_ok(accepted, prop, &rs, &os, &ref_row_map)
            };
            if !ok {
                mm.push(format!("[{key}] {prop}: reference={rs:?} ours={os:?}"));
            }
        }
        for prop in our_rec.keys() {
            if !ref_rec.contains_key(prop) {
                mm.push(format!(
                    "[{key}] extra property {prop} in ours (value: {})",
                    stringify(&our_rec[prop])
                ));
            }
        }
    }
    for key in ours_by_key.keys() {
        mm.push(format!("[{key}] extra record in our output"));
    }

    let (mismatches, missing_dup_occurrences) = mm.finish();
    Diff {
        header_ok: true,
        reference_rows: ref_recs.len(),
        our_rows: our_count,
        mismatches,
        missing_dup_occurrences,
    }
}

/// Guard for capture/fixture-gated tests. Returns `true` when the required
/// path is missing AND skipping is explicitly allowed (env
/// `TRIAGE_ALLOW_COMPAT_SKIP=1`); the caller should then `return`. If the
/// path is missing and skipping is NOT allowed, panics — so a green test run
/// always means the gated assertions actually executed (M1 carry-over).
/// Returns `false` when the path exists (caller proceeds normally).
pub fn skip_if_missing(required: &std::path::Path, what: &str) -> bool {
    if required.exists() {
        return false;
    }
    if std::env::var("TRIAGE_ALLOW_COMPAT_SKIP").as_deref() == Ok("1") {
        eprintln!(
            "SKIP ({what}): {} absent, TRIAGE_ALLOW_COMPAT_SKIP=1",
            required.display()
        );
        return true;
    }
    panic!(
        "{what} not found at {} — capture-gated test cannot run. \
         Set TRIAGE_ALLOW_COMPAT_SKIP=1 to allow skipping on machines \
         without evidence.",
        required.display()
    );
}

#[cfg(test)]
mod skip_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn skip_if_missing_all_branches() {
        // Branch 1: existing path → no skip.
        let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(!skip_if_missing(&here, "self"));

        // Branch 2: missing path + TRIAGE_ALLOW_COMPAT_SKIP=1 → skip (true).
        std::env::set_var("TRIAGE_ALLOW_COMPAT_SKIP", "1");
        let r = skip_if_missing(Path::new("/nonexistent/xyz"), "thing");
        std::env::remove_var("TRIAGE_ALLOW_COMPAT_SKIP");
        assert!(r);

        // Branch 3: missing path without allow env → panic.
        let result = std::panic::catch_unwind(|| {
            skip_if_missing(Path::new("/nonexistent/xyz"), "thing");
        });
        assert!(result.is_err());
        let msg = result.unwrap_err();
        let msg_str = msg
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| msg.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(
            msg_str.contains("capture-gated test cannot run"),
            "unexpected panic: {msg_str}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn timestamp_normalization() {
        assert_eq!(
            normalize_reference_timestamp("2024-06-29 13:52:19.3446519"),
            "2024-06-29T13:52:19.3446519Z"
        );
        assert_eq!(normalize_reference_timestamp(""), "");
        assert_eq!(
            normalize_reference_timestamp("MPCMDRUN.EXE"),
            "MPCMDRUN.EXE"
        );
        // already-ISO values pass through with at most the Z appended
        assert_eq!(
            normalize_reference_timestamp("2024-06-29T13:52:19.3446519Z"),
            "2024-06-29T13:52:19.3446519Z"
        );
    }

    #[test]
    fn csv_passing_pair_normalizes_timestamps_and_key_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let r = write(
            tmp.path(),
            "ref.csv",
            "SourceFilename,LastRun,Exe\n/staging/copy/A.pf,2024-06-29 13:52:19.3446519,X.EXE\n",
        );
        let o = write(
            tmp.path(),
            "ours.csv",
            "SourceFilename,LastRun,Exe\n/capture/orig/A.pf,2024-06-29T13:52:19.3446519Z,X.EXE\n",
        );
        let d = compare_csv(&r, &o, "SourceFilename", &[]);
        assert!(d.is_match(), "{:?}", d.mismatches);
        assert_eq!((d.reference_rows, d.our_rows), (1, 1));
    }

    #[test]
    fn csv_value_mismatch_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let r = write(tmp.path(), "ref.csv", "SourceFilename,Exe\nA.pf,X.EXE\n");
        let o = write(tmp.path(), "ours.csv", "SourceFilename,Exe\nA.pf,Y.EXE\n");
        let d = compare_csv(&r, &o, "SourceFilename", &[]);
        assert!(!d.is_match());
        assert_eq!(d.mismatches.len(), 1);
        assert!(d.mismatches[0].contains("Exe"), "{:?}", d.mismatches);
    }

    #[test]
    fn csv_header_mismatch_fails_early() {
        let tmp = tempfile::tempdir().unwrap();
        let r = write(tmp.path(), "ref.csv", "SourceFilename,Exe\nA.pf,X.EXE\n");
        let o = write(
            tmp.path(),
            "ours.csv",
            "SourceFilename,Executable\nA.pf,X.EXE\n",
        );
        let d = compare_csv(&r, &o, "SourceFilename", &[]);
        assert!(!d.header_ok);
        assert!(!d.is_match());
        assert!(d.mismatches[0].contains("header mismatch"));
    }

    #[test]
    fn csv_accepted_delta_passes_and_unaccepted_fields_still_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let r = write(tmp.path(), "ref.csv", "SourceFilename,Flag\nA.pf,False\n");
        let o = write(tmp.path(), "ours.csv", "SourceFilename,Flag\nA.pf,false\n");
        let delta = AcceptedDelta {
            field: "Flag",
            reason: "case-only difference in bool rendering",
            compare: |r, o| r.eq_ignore_ascii_case(o),
            row_guard: None,
        };
        assert!(compare_csv(&r, &o, "SourceFilename", &[delta]).is_match());
        // without the delta the same pair fails
        assert!(!compare_csv(&r, &o, "SourceFilename", &[]).is_match());
    }

    #[test]
    fn csv_missing_and_extra_rows_are_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let r = write(tmp.path(), "ref.csv", "SourceFilename\nA.pf\nB.pf\n");
        let o = write(tmp.path(), "ours.csv", "SourceFilename\nA.pf\nC.pf\n");
        let d = compare_csv(&r, &o, "SourceFilename", &[]);
        assert!(!d.is_match());
        assert!(d
            .mismatches
            .iter()
            .any(|m| m.contains("B.pf") && m.contains("missing")));
        assert!(d
            .mismatches
            .iter()
            .any(|m| m.contains("C.pf") && m.contains("extra")));
    }

    #[test]
    fn unkeyed_csv_matches_regardless_of_row_order() {
        let tmp = tempfile::tempdir().unwrap();
        let r = write(
            tmp.path(),
            "ref.csv",
            "RunTime,ExecutableName\n2024-06-29 13:52:19.3446519,\\VOL\\X.EXE\n2024-06-28 01:00:00.0000000,\\VOL\\Y.EXE\n",
        );
        let o = write(
            tmp.path(),
            "ours.csv",
            "RunTime,ExecutableName\n2024-06-28T01:00:00.0000000Z,\\VOL\\Y.EXE\n2024-06-29T13:52:19.3446519Z,\\VOL\\X.EXE\n",
        );
        let d = compare_csv_unkeyed(&r, &o, &[], &[]);
        assert!(d.is_match(), "{:?}", d.mismatches);
        // a changed volume path is NOT excused when ExecutableName is not a path column
        let bad = write(
            tmp.path(),
            "bad.csv",
            "RunTime,ExecutableName\n2024-06-28T01:00:00.0000000Z,\\OTHER\\Y.EXE\n2024-06-29T13:52:19.3446519Z,\\VOL\\X.EXE\n",
        );
        assert!(!compare_csv_unkeyed(&r, &bad, &[], &[]).is_match());
        // ...but IS excused when it is declared as one
        assert!(compare_csv_unkeyed(&r, &bad, &["ExecutableName"], &[]).is_match());
    }

    #[test]
    fn unkeyed_csv_reports_row_count_difference() {
        let tmp = tempfile::tempdir().unwrap();
        let r = write(tmp.path(), "ref.csv", "A\n1\n2\n");
        let o = write(tmp.path(), "ours.csv", "A\n1\n");
        let d = compare_csv_unkeyed(&r, &o, &[], &[]);
        assert!(!d.is_match());
        assert_eq!((d.reference_rows, d.our_rows), (2, 1));
        assert!(d.mismatches[0].contains("row count"));
    }

    #[test]
    fn ndjson_pass_with_bool_delta_and_property_set_enforcement() {
        let tmp = tempfile::tempdir().unwrap();
        let r = write(
            tmp.path(),
            "ref.json",
            "{\"SourceFilename\":\"/staging/A.pf\",\"LastRun\":\"2024-06-29 13:52:19.3446519\",\"ParsingError\":false}\n",
        );
        let o = write(
            tmp.path(),
            "ours.json",
            "{\"SourceFilename\":\"/capture/A.pf\",\"LastRun\":\"2024-06-29T13:52:19.3446519Z\",\"ParsingError\":\"False\"}\n",
        );
        let delta = AcceptedDelta {
            field: "ParsingError",
            reason: "bool vs string form",
            compare: |r, o| r.eq_ignore_ascii_case(o),
            row_guard: None,
        };
        let d = compare_ndjson(&r, &o, "SourceFilename", std::slice::from_ref(&delta));
        assert!(d.is_match(), "{:?}", d.mismatches);

        // an extra property in ours is a mismatch even when its value is empty
        let extra = write(
            tmp.path(),
            "extra.json",
            "{\"SourceFilename\":\"/capture/A.pf\",\"LastRun\":\"2024-06-29T13:52:19.3446519Z\",\"ParsingError\":\"False\",\"Note\":\"\"}\n",
        );
        let d = compare_ndjson(&r, &extra, "SourceFilename", &[delta]);
        assert!(!d.is_match());
        assert!(d
            .mismatches
            .iter()
            .any(|m| m.contains("extra property Note")));
    }

    #[test]
    fn ndjson_missing_property_and_missing_record_are_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let r = write(
            tmp.path(),
            "ref.json",
            "{\"SourceFilename\":\"A.pf\",\"Hash\":\"1\"}\n{\"SourceFilename\":\"B.pf\",\"Hash\":\"2\"}\n",
        );
        let o = write(tmp.path(), "ours.json", "{\"SourceFilename\":\"A.pf\"}\n");
        let d = compare_ndjson(&r, &o, "SourceFilename", &[]);
        assert!(!d.is_match());
        assert!(d
            .mismatches
            .iter()
            .any(|m| m.contains("property Hash missing")));
        assert!(d
            .mismatches
            .iter()
            .any(|m| m.contains("[B.pf] record missing")));
    }

    #[test]
    fn composite_key_distinguishes_same_basename_under_different_parents() {
        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("ref.csv");
        let ours = dir.path().join("ours.csv");
        std::fs::write(
            &reference,
            "SourceName,FileName,FileSize\n\
             /a/S-1/$I1,doc.txt,10\n\
             /a/S-2/$I1,doc.txt,20\n",
        )
        .unwrap();
        std::fs::write(
            &ours,
            "SourceName,FileName,FileSize\n\
             /cap/S-1/$I1,doc.txt,10\n\
             /cap/S-2/$I1,doc.txt,20\n",
        )
        .unwrap();
        let d = compare_csv_composite(
            &reference,
            &ours,
            &["SourceName"],
            composite_sid_basename,
            &[],
        );
        assert!(d.is_match(), "{:#?}", d.mismatches);
    }

    /// Grouped comparison: a key that repeats (same category, multiple LNKs)
    /// is disambiguated by row order, so positional rows compare against each
    /// other and a value mismatch on the Nth occurrence is still caught.
    #[test]
    fn grouped_key_pairs_repeated_keys_by_row_order() {
        fn cat_key(row: &BTreeMap<String, String>, _p: &[&str]) -> String {
            format!(
                "{}|{}",
                basename(row.get("SourceFile").map(String::as_str).unwrap_or("")),
                row.get("EntryName").cloned().unwrap_or_default()
            )
        }
        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("ref.csv");
        let ours = dir.path().join("ours.csv");
        // Two LNKs share (file, EntryName) = (jl, Cat); a third differs.
        std::fs::write(
            &reference,
            "SourceFile,EntryName,Arg\n\
             /a/jl,Cat,one\n/a/jl,Cat,two\n/a/jl,Other,three\n",
        )
        .unwrap();
        std::fs::write(
            &ours,
            "SourceFile,EntryName,Arg\n\
             /cap/jl,Cat,one\n/cap/jl,Cat,two\n/cap/jl,Other,three\n",
        )
        .unwrap();
        let d = compare_csv_grouped(&reference, &ours, &["SourceFile"], cat_key, &[]);
        assert!(d.is_match(), "{:#?}", d.mismatches);
        assert_eq!((d.reference_rows, d.our_rows), (3, 3));

        // A mismatch on the SECOND occurrence is still reported (no silent drop).
        std::fs::write(
            &ours,
            "SourceFile,EntryName,Arg\n\
             /cap/jl,Cat,one\n/cap/jl,Cat,CHANGED\n/cap/jl,Other,three\n",
        )
        .unwrap();
        let d = compare_csv_grouped(&reference, &ours, &["SourceFile"], cat_key, &[]);
        assert!(!d.is_match());
        assert!(
            d.mismatches
                .iter()
                .any(|m| m.contains("Arg") && m.contains("#2")),
            "{:#?}",
            d.mismatches
        );
    }

    #[test]
    fn composite_key_still_catches_value_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("ref.csv");
        let ours = dir.path().join("ours.csv");
        std::fs::write(
            &reference,
            "SourceName,FileName,FileSize\n/a/S-1/$I1,doc.txt,10\n",
        )
        .unwrap();
        std::fs::write(
            &ours,
            "SourceName,FileName,FileSize\n/cap/S-1/$I1,doc.txt,99\n",
        )
        .unwrap();
        let d = compare_csv_composite(
            &reference,
            &ours,
            &["SourceName"],
            composite_sid_basename,
            &[],
        );
        assert!(!d.is_match());
        assert!(
            d.mismatches.iter().any(|m| m.contains("FileSize")),
            "{:#?}",
            d.mismatches
        );
    }

    #[test]
    fn mismatches_are_capped_but_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ref_csv = String::from("SourceFilename,V\n");
        let mut our_csv = String::from("SourceFilename,V\n");
        for i in 0..30 {
            ref_csv.push_str(&format!("f{i}.pf,a\n"));
            our_csv.push_str(&format!("f{i}.pf,b\n"));
        }
        let r = write(tmp.path(), "ref.csv", &ref_csv);
        let o = write(tmp.path(), "ours.csv", &our_csv);
        let d = compare_csv(&r, &o, "SourceFilename", &[]);
        assert!(!d.is_match());
        assert_eq!(d.mismatches.len(), 21);
        assert_eq!(d.mismatches[20], "...and 10 more");
    }

    /// A delta with a `row_guard` that only matches rows where the reference
    /// column "Kind" == "special":
    /// - a mismatch on a "special" row is accepted
    /// - the SAME field mismatch on a "normal" row is NOT accepted (regression guard has teeth)
    #[test]
    fn row_guard_scopes_delta_to_matching_rows_only() {
        let tmp = tempfile::tempdir().unwrap();
        // Two rows: one "special", one "normal". Both have a Value mismatch.
        let r = write(
            tmp.path(),
            "ref.csv",
            "Key,Kind,Value\nk1,special,OLD\nk2,normal,OLD\n",
        );
        let o = write(
            tmp.path(),
            "ours.csv",
            "Key,Kind,Value\nk1,special,NEW\nk2,normal,NEW\n",
        );
        let delta = AcceptedDelta {
            field: "Value",
            reason: "only accepted for special-kind rows",
            compare: |_r, _o| true,
            row_guard: Some(|row| row.get("Kind").map(|v| v == "special").unwrap_or(false)),
        };
        let d = compare_csv(&r, &o, "Key", &[delta]);
        // The "special" mismatch is accepted; the "normal" one is not.
        assert!(
            !d.is_match(),
            "expected the normal-row mismatch to fail the gate"
        );
        assert_eq!(
            d.mismatches.len(),
            1,
            "expected exactly 1 mismatch (the normal row)"
        );
        assert!(
            d.mismatches[0].contains("k2") && d.mismatches[0].contains("Value"),
            "unexpected mismatch: {:?}",
            d.mismatches
        );
    }
}

pub mod synthetic;
