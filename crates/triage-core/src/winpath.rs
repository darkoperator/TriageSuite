use percent_encoding::percent_decode_str;

/// Split a source path into normalized segments: both `/` and `\` are
/// separators, each segment is percent-decoded (e.g. `C%3A` -> `C:`),
/// and empty components are dropped. Original character case is preserved
/// so usernames keep their on-disk casing.
pub fn segments(path: &str) -> Vec<String> {
    path.split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .map(|s| percent_decode_str(s).decode_utf8_lossy().into_owned())
        .collect()
}

/// True for drive-root segment forms like `C` or `C:` (spec section 3.6).
pub fn is_drive_segment(seg: &str) -> bool {
    let s = seg.strip_suffix(':').unwrap_or(seg);
    s.len() == 1 && s.chars().all(|c| c.is_ascii_alphabetic())
}

/// Case-insensitive segment comparison (Windows artifact names).
pub fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_split_both_separators_and_percent_decode() {
        assert_eq!(
            segments(r"uploads\auto/C%3A/Users/charlie/file.lnk"),
            vec!["uploads", "auto", "C:", "Users", "charlie", "file.lnk"]
        );
    }

    #[test]
    fn segments_drop_empty_components() {
        assert_eq!(segments("C//Users///bob"), vec!["C", "Users", "bob"]);
    }

    #[test]
    fn drive_segments_recognized() {
        assert!(is_drive_segment("C"));
        assert!(is_drive_segment("c:"));
        assert!(is_drive_segment("D:"));
        assert!(!is_drive_segment("Users"));
        assert!(!is_drive_segment("CD"));
    }

    #[test]
    fn case_insensitive_compare() {
        assert!(eq_ci("UsErS", "users"));
        assert!(!eq_ci("user", "users"));
    }
}
