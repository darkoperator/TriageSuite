//! ESE database header sniffing — no `ese_parser_lib` needed. Reads the file
//! magic and the on-disk format-revision field so callers can detect the
//! revision and report revisions above the supported ceiling without
//! attempting a load that would fail cryptically.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// ESE file signature `0x89ABCDEF`, stored little-endian at offset 4.
const ESE_MAGIC: u32 = 0x89AB_CDEF;
/// Offset of the `u32` LE format-revision field (110 / 230 / 300).
const REVISION_OFFSET: u64 = 0xE8;

/// True if the file begins with the ESE magic (u32 LE `0x89ABCDEF` at offset 4).
pub fn is_ese(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 8];
    if f.read_exact(&mut buf).is_err() {
        return false;
    }
    u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) == ESE_MAGIC
}

/// Read the ESE format revision (110, 230, 300, ...) from the header. Returns
/// an error if the file is too short or not an ESE database.
pub fn format_revision(path: &Path) -> std::io::Result<u32> {
    let mut f = std::fs::File::open(path)?;
    let mut magic = [0u8; 8];
    f.read_exact(&mut magic)?;
    if u32::from_le_bytes([magic[4], magic[5], magic[6], magic[7]]) != ESE_MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not an ESE database (bad magic)",
        ));
    }
    f.seek(SeekFrom::Start(REVISION_OFFSET))?;
    let mut rev = [0u8; 4];
    f.read_exact(&mut rev)?;
    Ok(u32::from_le_bytes(rev))
}

/// The highest ESE format revision the engine reads. Revision 300 (Win11 24H2 /
/// Server 2025) is supported via the forked ese_parser_lib tag-count mask
/// (see the rev-300 spike memo). Revisions above this are detected and reported.
pub const MAX_SUPPORTED_REVISION: u32 = 300;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn captures_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
    }

    /// Walk the captures for an SRUDB.dat whose path contains `collection`.
    fn find_srudb(collection: &str) -> Option<PathBuf> {
        let mut stack = vec![captures_root()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.file_name().and_then(|n| n.to_str()) == Some("SRUDB.dat")
                    && p.to_string_lossy().contains(collection)
                {
                    return Some(p);
                }
            }
        }
        None
    }

    #[test]
    fn revision_matches_known_srudbs() {
        // Gated on local evidence (gitignored); skip if absent.
        if !captures_root().exists() {
            return;
        }
        for (coll, want) in [("Collection-STCL1", 110u32), ("Collection-DESKTOP", 230)] {
            if let Some(p) = find_srudb(coll) {
                assert!(is_ese(&p), "{coll} SRUDB should be ESE");
                assert_eq!(format_revision(&p).unwrap(), want, "{coll} revision");
            }
        }
        if let Some(p) = find_srudb("Collection-STDC1") {
            assert_eq!(
                format_revision(&p).unwrap(),
                300,
                "STDC1 server SRUDB is rev 300"
            );
        }
    }
}
