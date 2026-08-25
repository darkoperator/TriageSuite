//! Windows Recycle Bin metadata parsers ($I records and legacy INFO2 files).

pub mod dollar_i;
pub mod info2;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("not a recognized recycle-bin record")]
    BadFormat,
    #[error("unsupported $I format version {0}")]
    UnsupportedVersion(i64),
    #[error("truncated or corrupt structure: {0}")]
    Corrupt(&'static str),
}

/// A single deleted-file record, shared by $I and INFO2 (RBCmd CsvOut shape).
pub struct DeletedEntry {
    /// Original full path/name of the deleted file (UTF-16 decoded).
    pub file_name: String,
    /// Size in bytes of the deleted file.
    pub file_size: i64,
    /// Deletion time as a Windows FILETIME (0 = unset).
    pub deleted_on: u64,
}

/// Read a little-endian i64 at `off`, bounds-checked.
pub(crate) fn read_i64(d: &[u8], off: usize) -> Result<i64, ParseError> {
    let end = off.checked_add(8).ok_or(ParseError::Corrupt("i64 off"))?;
    let b = d.get(off..end).ok_or(ParseError::Corrupt("i64 read"))?;
    Ok(i64::from_le_bytes(b.try_into().unwrap()))
}

/// Read a little-endian i32 at `off`, bounds-checked.
pub(crate) fn read_i32(d: &[u8], off: usize) -> Result<i32, ParseError> {
    let end = off.checked_add(4).ok_or(ParseError::Corrupt("i32 off"))?;
    let b = d.get(off..end).ok_or(ParseError::Corrupt("i32 read"))?;
    Ok(i32::from_le_bytes(b.try_into().unwrap()))
}

/// Decode UTF-16LE up to the first NUL (matching C# .Split('\0').First()).
pub(crate) fn utf16_to_nul(bytes: &[u8]) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let u = u16::from_le_bytes([pair[0], pair[1]]);
        if u == 0 {
            break;
        }
        units.push(u);
    }
    String::from_utf16_lossy(&units)
}
