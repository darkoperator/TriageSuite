use crate::{read_i32, read_i64, utf16_to_nul, DeletedEntry, ParseError};

/// Parse a `$I` record (RecycleBin/DollarI.cs). Format 1 = pre-Win10
/// (UTF-16 name from offset 24); Format 2 = Win10+ (i32 nameLen @24,
/// UTF-16 name @28). All reads bounds-checked; never panics.
pub fn parse(data: &[u8]) -> Result<DeletedEntry, ParseError> {
    let format = read_i64(data, 0)?;
    let file_size = read_i64(data, 8)?;
    let deleted_on = read_i64(data, 16)? as u64;

    let file_name = match format {
        1 => utf16_to_nul(data.get(24..).ok_or(ParseError::Corrupt("v1 name"))?),
        2 => {
            let name_len = read_i32(data, 24)? as usize;
            let byte_len = name_len
                .checked_mul(2)
                .ok_or(ParseError::Corrupt("name len overflow"))?;
            let start = 28usize;
            let end = start
                .checked_add(byte_len)
                .ok_or(ParseError::Corrupt("name range overflow"))?;
            let bytes = data
                .get(start..end)
                .ok_or(ParseError::Corrupt("v2 name truncated"))?;
            utf16_to_nul(bytes)
        }
        other => return Err(ParseError::UnsupportedVersion(other)),
    };

    Ok(DeletedEntry {
        file_name,
        file_size,
        deleted_on,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTURES_ROOT: &str = "../../test captures";

    /// Find `$I` files under any per-SID Recycle Bin directory in any local
    /// capture. Discovering these rather than hardcoding one collection keeps
    /// the test working across captures — and keeps host names and SIDs out of
    /// the source tree.
    fn any_dollar_i() -> Option<Vec<std::path::PathBuf>> {
        let root = std::path::Path::new(CAPTURES_ROOT);
        if !root.exists() {
            return None;
        }
        let mut v: Vec<std::path::PathBuf> = Vec::new();
        // <captures>/<collection>/uploads/auto/C%3A/$Recycle.Bin/<SID>/$I*
        for collection in std::fs::read_dir(root).ok()?.flatten() {
            let bin = collection.path().join("uploads/auto/C%3A/$Recycle.Bin");
            let Ok(sids) = std::fs::read_dir(&bin) else {
                continue;
            };
            for sid in sids.flatten() {
                let Ok(entries) = std::fs::read_dir(sid.path()) else {
                    continue;
                };
                v.extend(entries.flatten().map(|e| e.path()).filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("$I"))
                }));
            }
        }
        if v.is_empty() {
            return None;
        }
        v.sort();
        Some(v)
    }

    #[test]
    fn parses_real_dollar_i_files() {
        let Some(files) = any_dollar_i() else {
            eprintln!("captures absent; skipping");
            return;
        };
        assert!(!files.is_empty());
        for f in &files {
            let raw = std::fs::read(f).unwrap();
            let e = parse(&raw).unwrap();
            assert!(e.deleted_on > 0, "{}", f.display());
            assert!(e.file_size >= 0);
            assert!(!e.file_name.is_empty());
            assert!(
                e.file_name.contains(':')
                    || e.file_name.starts_with('\\')
                    || e.file_name.contains('\\'),
                "unexpected name {:?} in {}",
                e.file_name,
                f.display()
            );
        }
    }

    #[test]
    fn parses_synthetic_v1_and_v2() {
        let name: Vec<u16> = "C:\\x.txt\0".encode_utf16().collect();
        let mut v2 = Vec::new();
        v2.extend_from_slice(&2i64.to_le_bytes());
        v2.extend_from_slice(&0x1000i64.to_le_bytes());
        v2.extend_from_slice(&133_000_000_000_000_000u64.to_le_bytes());
        v2.extend_from_slice(&(name.len() as i32).to_le_bytes());
        for u in &name {
            v2.extend_from_slice(&u.to_le_bytes());
        }
        let e = parse(&v2).unwrap();
        assert_eq!(e.file_name, "C:\\x.txt");
        assert_eq!(e.file_size, 0x1000);
        assert_eq!(e.deleted_on, 133_000_000_000_000_000);

        let mut v1 = Vec::new();
        v1.extend_from_slice(&1i64.to_le_bytes());
        v1.extend_from_slice(&42i64.to_le_bytes());
        v1.extend_from_slice(&133_000_000_000_000_001u64.to_le_bytes());
        for u in &name {
            v1.extend_from_slice(&u.to_le_bytes());
        }
        let e = parse(&v1).unwrap();
        assert_eq!(e.file_name, "C:\\x.txt");
        assert_eq!(e.file_size, 42);
    }

    #[test]
    fn rejects_garbage_and_truncation_without_panic() {
        assert!(parse(b"").is_err());
        assert!(parse(b"short").is_err());
        let mut t = Vec::new();
        t.extend_from_slice(&2i64.to_le_bytes());
        t.extend_from_slice(&0i64.to_le_bytes());
        t.extend_from_slice(&0u64.to_le_bytes());
        t.extend_from_slice(&1000i32.to_le_bytes());
        let _ = parse(&t); // Err, never panic
    }
}
