//! Filename decoding is total: every input produces a value or an explicit
//! absence, never a panic. Filenames reach this from a directory listing, so
//! they are attacker-influenced in exactly the way a capture is.

use triage_orchestrator::validate::validate_capture;
use triage_orchestrator::velo::merge::user_from_per_user_filename;

#[test]
fn user_decoding_is_total_over_hostile_filenames() {
    let stem = "2026-03-13T192553Z_LETriage_results";
    let cases = [
        String::new(),
        ".".into(),
        "..".into(),
        ".csv".into(),
        stem.to_string(),
        format!("{stem}_"),
        format!("{stem}_.csv"),
        format!("{stem}_..csv"),
        format!("{stem}_{}.csv", "a".repeat(4096)),
        format!("{stem}_\u{0}.csv"),
        format!("{stem}_\u{FFFD}.csv"),
        format!("{stem}_../../etc/passwd.csv"),
        "\u{202E}gnp.csv".into(),
    ];
    // The stem is swept with the same hostility as the filename: it is built
    // from a tool's own `DatasetSpec`, but the decode must be total over it
    // regardless, and an empty or oversized stem is exactly where a prefix
    // strip would be tempted to index rather than slice.
    let hostile_stems = [
        String::new(),
        stem.to_string(),
        format!("{stem}_"),
        format!("{stem}_Timeline"),
        "a".repeat(4096),
        "\u{0}".into(),
        "\u{202E}".into(),
        "../../etc".into(),
    ];
    for case in &cases {
        // Totality only: must not panic, whatever it decides to return.
        let _ = user_from_per_user_filename(case, stem);
        for hostile in &hostile_stems {
            let _ = user_from_per_user_filename(case, hostile);
        }
    }

    // Pin the one case that looks like a path-traversal payload.
    // `user_from_per_user_filename` only ever slices the input between a
    // literal prefix/underscore and a literal dot; it never inspects path
    // separators, so a label containing `/` or `\` comes back verbatim
    // instead of `None` or a sanitized string. That is safe today only
    // because the label lands in a CSV cell (the `TriageUser` column) and is
    // never used to build a filesystem path. This assertion pins that exact
    // verbatim behavior: if someone later changes the function to sanitize
    // separators, or reuses it to construct a path, this is what breaks and
    // forces them to re-examine whether the "it's just a CSV cell" reasoning
    // still holds.
    assert_eq!(
        user_from_per_user_filename(&format!("{stem}_../../etc/passwd.csv"), stem).as_deref(),
        Some("../../etc/passwd")
    );
}

/// `validate_capture` reads an attacker-influenced path and, for a `.zip`,
/// attacker-influenced archive entry names. It must never panic, whatever it
/// decides to report -- an absent, unreadable or malformed input is a
/// finding, not a propagated error.
#[test]
fn validation_is_total_over_hostile_paths_and_entries() {
    let hostile_paths = [
        String::new(),
        "/nonexistent".into(),
        "/dev/null".into(),
        "\u{0}".into(),
        "\u{0}.zip".into(),
        "..".into(),
        "../../../../etc/passwd".into(),
        "a".repeat(4096),
        format!("{}.zip", "a".repeat(4096)),
    ];
    for path in &hostile_paths {
        // Totality only: must not panic, whatever it decides to report.
        let _ = validate_capture(std::path::Path::new(path));
    }

    // A malformed .zip -- present, readable, but not a valid archive -- must
    // report a finding rather than panicking on a corrupt central directory.
    let tmp = tempfile::tempdir().unwrap();
    let bogus = tmp.path().join("bogus.zip");
    std::fs::write(&bogus, b"not actually a zip file").unwrap();
    let report = validate_capture(&bogus);
    assert!(!report.valid, "a corrupt zip must be reported invalid");

    // An empty (zero-entry) directory: no panic, reported invalid (missing
    // every required artifact).
    let empty = tmp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let report = validate_capture(&empty);
    assert!(!report.valid);
}
