use crate::{read_i32, read_i64, utf16_to_nul, DeletedEntry, ParseError};

/// Parse a legacy INFO2 file (RecycleBin/Info2.cs): 20-byte header then
/// fixed 800-byte records. One DeletedEntry per complete record; a trailing
/// partial record (< 800 bytes) is ignored.
pub fn parse(data: &[u8]) -> Result<Vec<DeletedEntry>, ParseError> {
    if data.len() < 20 {
        return Err(ParseError::Corrupt("INFO2 header"));
    }
    let mut entries = Vec::new();
    let mut idx = 20usize;
    while idx + 800 <= data.len() {
        let rec = &data[idx..idx + 800];
        let uni = utf16_to_nul(&rec[280..800]);
        // Prefer the Unicode name, fall back to ASCII when empty — matches
        // RBCmd Program.cs:448-451 exactly (compatibility, not a heuristic).
        let file_name = if uni.is_empty() {
            ascii_to_nul(&rec[0..260])
        } else {
            uni
        };
        // FileSize is an i32 on disk; widened to i64. Pre-Vista records >2 GiB
        // store 0xFFFFFFFF, which sign-extends to -1 — faithful to RBCmd.
        let file_size = read_i32(rec, 276)? as i64;
        let deleted_on = read_i64(rec, 268)? as u64;
        entries.push(DeletedEntry {
            file_name,
            file_size,
            deleted_on,
        });
        idx += 800;
    }
    Ok(entries)
}

fn ascii_to_nul(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_info2(records: &[(&str, i32, u64)]) -> Vec<u8> {
        let mut v = vec![0u8; 20];
        v[0..4].copy_from_slice(&5i32.to_le_bytes());
        v[12..16].copy_from_slice(&800i32.to_le_bytes());
        for (name, size, ft) in records {
            let mut rec = vec![0u8; 800];
            let asc = name.as_bytes();
            let n = asc.len().min(259);
            rec[..n].copy_from_slice(&asc[..n]);
            rec[260..264].copy_from_slice(&0i32.to_le_bytes());
            rec[264..268].copy_from_slice(&0i32.to_le_bytes());
            rec[268..276].copy_from_slice(&ft.to_le_bytes());
            rec[276..280].copy_from_slice(&size.to_le_bytes());
            let uni: Vec<u16> = name.encode_utf16().collect();
            for (i, u) in uni.iter().enumerate() {
                let off = 280 + i * 2;
                if off + 2 <= 800 {
                    rec[off..off + 2].copy_from_slice(&u.to_le_bytes());
                }
            }
            v.extend_from_slice(&rec);
        }
        v
    }

    #[test]
    fn parses_synthetic_info2() {
        let raw = build_info2(&[
            ("C:\\a.txt", 10, 133_000_000_000_000_000),
            ("C:\\b.txt", 20, 133_000_000_000_000_001),
        ]);
        let entries = parse(&raw).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].file_name, "C:\\a.txt");
        assert_eq!(entries[0].file_size, 10);
        assert_eq!(entries[0].deleted_on, 133_000_000_000_000_000);
        assert_eq!(entries[1].file_size, 20);
    }

    #[test]
    fn short_header_is_error_not_panic() {
        assert!(parse(b"tiny").is_err());
    }

    #[test]
    fn trailing_partial_record_is_ignored() {
        let mut raw = build_info2(&[("C:\\a.txt", 10, 133_000_000_000_000_000)]);
        raw.extend_from_slice(&[0u8; 100]);
        let entries = parse(&raw).unwrap();
        assert_eq!(entries.len(), 1);
    }
}
