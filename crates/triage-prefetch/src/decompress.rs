//! MAM (LZXPRESS Huffman) decompression for Win10/11 prefetch files.

/// Errors from MAM decompression.
#[derive(Debug, thiserror::Error)]
pub enum DecompressError {
    #[error("not a MAM-compressed prefetch file")]
    NotMam,
    #[error("truncated or corrupt compressed data: {0}")]
    Corrupt(&'static str),
}

/// Decompress a MAM-wrapped (Win10/11) prefetch file to raw SCCA bytes.
/// Input: full file content starting with `MAM\x04`. Output is exactly the
/// declared decompressed size; streams that terminate early are zero-padded
/// (matching RtlDecompressBufferEx).
///
/// MAM container layout:
/// - bytes 0..4: signature `MAM\x04` (plain LZXPRESS-Huffman; the `MAM\x84`
///   CRC variant is rejected as [`DecompressError::NotMam`])
/// - bytes 4..8: little-endian u32 decompressed size; bit 31 is a flag in some
///   variants, so only the low 30 bits are taken as the size
/// - bytes 8.. : LZXPRESS-Huffman payload ([MS-XCA] section 2.2.4)
///
/// The payload itself is decoded by the `xpress-huffman` crate (pure Rust,
/// `forbid(unsafe_code)`, every untrusted read bounds-checked): per 64 KiB
/// chunk it parses the 256-byte table of 512 4-bit code lengths, assigns
/// canonical Huffman codes over 512 symbols (0-255 literals, 256+ packed
/// match length/distance), reads the bitstream in 16-bit little-endian
/// refills, and performs LZ77 match copies byte-by-byte so overlapping
/// sources replicate correctly.
pub fn decompress_mam(data: &[u8]) -> Result<Vec<u8>, DecompressError> {
    // Signature check: exactly `MAM\x04`. Anything else (including the
    // `MAM\x84` CRC variant, which carries an extra checksum field this
    // decoder does not handle) is not ours to decode.
    if data.len() < 8 || &data[..4] != b"MAM\x04" {
        return Err(DecompressError::NotMam);
    }

    // Declared decompressed size: LE u32 at offset 4, low 30 bits only
    // (bit 31 flags a CRC variant in some MAM containers; mask it off).
    let declared =
        (u32::from_le_bytes([data[4], data[5], data[6], data[7]]) & 0x3FFF_FFFF) as usize;

    /// Real prefetch files decompress to a few MB at most; this ceiling stops
    /// allocation-bomb inputs (a 16-byte file can otherwise declare ~1 GiB).
    const MAX_DECOMPRESSED: usize = 64 * 1024 * 1024;
    if declared > MAX_DECOMPRESSED {
        return Err(DecompressError::Corrupt(
            "declared size exceeds prefetch maximum",
        ));
    }

    // Decode the LZXPRESS-Huffman payload. The decoder stops at `declared`
    // bytes or stream end, whichever comes first; structural problems
    // (truncated chunk table, back-reference before output start) are errors.
    //
    // A degenerate Huffman code-length table (e.g. 256 bytes of 0xFF) causes
    // the `xpress-huffman` crate to panic with an index-out-of-bounds on its
    // fixed 1024-node arena. We catch that panic and map it to a Corrupt error.
    // Safety/unwind-safety note: the closure only reads the input slice and
    // builds a fresh Vec; no shared mutable state is touched, so
    // AssertUnwindSafe is sound here.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        xpress_huffman::decompress(&data[8..], declared)
    }));
    let mut out = match result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            return Err(match e {
                xpress_huffman::Error::TruncatedTable => {
                    DecompressError::Corrupt("truncated Huffman table")
                }
                xpress_huffman::Error::BadMatchOffset => {
                    DecompressError::Corrupt("match offset before output start")
                }
            })
        }
        Err(_) => {
            return Err(DecompressError::Corrupt(
                "decoder panic on malformed stream",
            ))
        }
    };

    // A final LZ77 match may not run past the declared size: a stream whose
    // last copy overshoots the output buffer is corrupt (RtlDecompressBufferEx
    // would fail with a too-small buffer).
    if out.len() > declared {
        return Err(DecompressError::Corrupt("output exceeds declared size"));
    }

    // Streams may legitimately end a few bytes short of the declared size;
    // Windows' RtlDecompressBufferEx leaves the tail zeroed, so zero-pad to
    // exactly the declared size.
    out.resize(declared, 0);
    Ok(out)
}

/// Test-support walker shared with format.rs tests.
#[cfg(test)]
pub(crate) fn walk_pf(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pf")) {
                    out.push(p);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "../../test captures/Collection-DESKTOP-OA8SHHC-2026-03-12T22_54_56Z/uploads/auto/C%3A/Windows/Prefetch/MSEDGE.EXE-37D25F9E.pf";

    fn sample() -> Option<Vec<u8>> {
        std::fs::read(SAMPLE).ok()
    }

    #[test]
    fn decompresses_real_mam_prefetch() {
        let Some(raw) = sample() else {
            eprintln!("test captures not present; skipping");
            return;
        };
        assert_eq!(&raw[..4], b"MAM\x04");
        let decompressed = decompress_mam(&raw).unwrap();
        let declared = u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize;
        assert_eq!(decompressed.len(), declared);
        assert_eq!(&decompressed[4..8], b"SCCA");
        let version = u32::from_le_bytes(decompressed[0..4].try_into().unwrap());
        assert!(
            matches!(version, 17 | 23 | 26 | 30 | 31),
            "version {version}"
        );
    }

    #[test]
    fn matches_independent_python_decompression() {
        // Cross-validation: target/pf-decompressed was produced by dissect.util
        // (pure Python, independent implementation) during fixture generation.
        let Some(raw) = sample() else { return };
        let Ok(expected) =
            std::fs::read("../../target/pf-decompressed/DESKTOP/MSEDGE.EXE-37D25F9E.pf")
        else {
            eprintln!("staged python decompression not present; skipping");
            return;
        };
        let decompressed = decompress_mam(&raw).unwrap();
        assert_eq!(decompressed.len(), expected.len(), "length mismatch");
        assert_eq!(
            decompressed, expected,
            "byte-for-byte mismatch vs dissect.util"
        );
    }

    #[test]
    fn truncated_input_is_an_error_not_a_panic() {
        let Some(raw) = sample() else { return };
        for cut in [0usize, 3, 7, 8, 64, raw.len() / 2] {
            let _ = decompress_mam(&raw[..cut]); // must return Err, never panic
        }
    }

    #[test]
    fn non_mam_input_is_an_error() {
        assert!(decompress_mam(b"SCCA whatever").is_err());
    }

    #[test]
    fn hostile_huffman_table_is_corrupt_not_panic() {
        // 256 bytes of 0xFF = over-subscribed code-length table that
        // overflows the decoder's node arena if not caught.
        let mut evil = Vec::new();
        evil.extend_from_slice(b"MAM\x04");
        evil.extend_from_slice(&1024u32.to_le_bytes());
        evil.extend_from_slice(&[0xFF; 256]);
        evil.extend_from_slice(&[0x00; 64]);
        match decompress_mam(&evil) {
            Err(DecompressError::Corrupt(_)) => {}
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }

    #[test]
    fn oversized_declared_size_is_rejected_before_allocation() {
        let mut evil = Vec::new();
        evil.extend_from_slice(b"MAM\x04");
        evil.extend_from_slice(&0x3FFF_FFFFu32.to_le_bytes());
        evil.extend_from_slice(&[0x00; 8]);
        match decompress_mam(&evil) {
            Err(DecompressError::Corrupt(_)) => {}
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }

    #[test]
    fn decompresses_every_pf_in_captures() {
        let root = std::path::Path::new("../../test captures");
        if !root.exists() {
            return;
        }
        let mut ok = 0u32;
        let mut failed = vec![];
        for entry in walk_pf(root) {
            let raw = std::fs::read(&entry).unwrap();
            if raw.len() >= 4 && &raw[..4] == b"MAM\x04" {
                match decompress_mam(&raw) {
                    Ok(_) => ok += 1,
                    Err(e) => failed.push(format!("{}: {e}", entry.display())),
                }
            }
        }
        assert!(failed.is_empty(), "{ok} ok, failures: {failed:#?}");
        assert!(ok > 500, "expected 500+ MAM files, got {ok}");
    }
}
