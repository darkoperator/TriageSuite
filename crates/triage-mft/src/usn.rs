use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::Result;

const MIN_RECORD_LENGTH: usize = 60;

const REASON_FLAGS: &[(u32, &str)] = &[
    (0x0000_0001, "DataOverwrite"),
    (0x0000_0002, "DataExtend"),
    (0x0000_0004, "DataTruncation"),
    (0x0000_0010, "NamedDataOverwrite"),
    (0x0000_0020, "NamedDataExtend"),
    (0x0000_0040, "NamedDataTruncation"),
    (0x0000_0100, "FileCreate"),
    (0x0000_0200, "FileDelete"),
    (0x0000_0400, "EaChange"),
    (0x0000_0800, "SecurityChange"),
    (0x0000_1000, "RenameOldName"),
    (0x0000_2000, "RenameNewName"),
    (0x0000_4000, "IndexableChange"),
    (0x0000_8000, "BasicInfoChange"),
    (0x0001_0000, "HardLinkChange"),
    (0x0002_0000, "CompressionChange"),
    (0x0004_0000, "EncryptionChange"),
    (0x0008_0000, "ObjectIdChange"),
    (0x0010_0000, "ReparsePointChange"),
    (0x0020_0000, "StreamChange"),
    (0x0040_0000, "TransactedChange"),
    (0x0080_0000, "IntegrityChange"),
    (0x8000_0000, "Close"),
];

const ATTRIBUTE_FLAGS: &[(u32, &str)] = &[
    (0x0000_0001, "ReadOnly"),
    (0x0000_0002, "Hidden"),
    (0x0000_0004, "System"),
    (0x0000_0010, "Directory"),
    (0x0000_0020, "Archive"),
    (0x0000_0040, "Device"),
    (0x0000_0080, "Normal"),
    (0x0000_0100, "Temporary"),
    (0x0000_0200, "SparseFile"),
    (0x0000_0400, "ReparsePoint"),
    (0x0000_0800, "Compressed"),
    (0x0000_1000, "Offline"),
    (0x0000_2000, "NotContentIndexed"),
    (0x0000_4000, "Encrypted"),
    (0x0000_8000, "IntegrityStream"),
    (0x0002_0000, "NoScrubData"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct JEntryRecord {
    pub name: String,
    pub extension: String,
    pub entry_number: u64,
    pub sequence_number: u16,
    pub parent_entry_number: u64,
    pub parent_sequence_number: u16,
    pub parent_path: String,
    pub update_sequence_number: u64,
    pub update_timestamp: String,
    pub update_reasons: String,
    pub file_attributes: String,
    pub offset_to_data: u64,
    pub source_file: PathBuf,
}

pub fn apply_parent_paths(records: &mut [JEntryRecord], index: &crate::mft::MftPathIndex) {
    for record in records {
        if let Some(path) =
            index.full_path(record.parent_entry_number, record.parent_sequence_number)
        {
            record.parent_path = path;
        }
    }
}

pub fn parse_usn_journal(path: impl AsRef<Path>) -> Result<Vec<JEntryRecord>> {
    let mut records = Vec::new();
    match visit_usn_journal(path, |record| {
        records.push(record);
        Ok::<(), std::convert::Infallible>(())
    }) {
        Ok(()) => Ok(records),
        Err(UsnVisitError::Parse(error)) => Err(error),
        Err(UsnVisitError::Visitor(never)) => match never {},
    }
}

#[derive(Debug)]
pub enum UsnVisitError<E> {
    Parse(crate::error::MftriageError),
    Visitor(E),
}

pub fn visit_usn_journal<E>(
    path: impl AsRef<Path>,
    mut visitor: impl FnMut(JEntryRecord) -> std::result::Result<(), E>,
) -> std::result::Result<(), UsnVisitError<E>> {
    use std::io::{Read, Seek};
    let path = path.as_ref();
    let mut reader = std::io::BufReader::new(
        std::fs::File::open(path).map_err(|e| UsnVisitError::Parse(e.into()))?,
    );
    let mut offset = 0usize;
    loop {
        let mut length_bytes = [0u8; 4];
        match reader.read_exact(&mut length_bytes) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(UsnVisitError::Parse(e.into())),
        }
        let record_length = u32::from_le_bytes(length_bytes) as usize;
        if record_length == 0 {
            reader
                .seek(std::io::SeekFrom::Current(4))
                .map_err(|e| UsnVisitError::Parse(e.into()))?;
            offset += 8;
            continue;
        }

        if record_length < MIN_RECORD_LENGTH {
            reader
                .seek(std::io::SeekFrom::Current(
                    record_length.saturating_sub(4).max(4) as i64,
                ))
                .map_err(|e| UsnVisitError::Parse(e.into()))?;
            offset += record_length.max(8);
            continue;
        }
        let mut bytes = vec![0u8; record_length];
        bytes[..4].copy_from_slice(&length_bytes);
        if let Err(e) = reader.read_exact(&mut bytes[4..]) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                break;
            }
            return Err(UsnVisitError::Parse(e.into()));
        }
        let major_version = read_u16(&bytes, 4);
        if major_version == 2 {
            if let Some(record) = parse_v2_record(&bytes, offset, path) {
                visitor(record).map_err(UsnVisitError::Visitor)?;
            }
        }
        offset += record_length;
    }
    Ok(())
}

fn parse_v2_record(
    record: &[u8],
    offset_to_data: usize,
    source_file: &Path,
) -> Option<JEntryRecord> {
    let file_name_length = read_u16(record, 56) as usize;
    let file_name_offset = read_u16(record, 58) as usize;
    let file_name_end = file_name_offset.checked_add(file_name_length)?;
    if file_name_offset < MIN_RECORD_LENGTH
        || file_name_end > record.len()
        || !file_name_length.is_multiple_of(2)
    {
        return None;
    }

    let name = decode_utf16le(&record[file_name_offset..file_name_end]);
    let (entry_number, sequence_number) = split_file_reference(read_u64(record, 8));
    let (parent_entry_number, parent_sequence_number) = split_file_reference(read_u64(record, 16));

    Some(JEntryRecord {
        extension: extension_for(&name),
        name,
        entry_number,
        sequence_number,
        parent_entry_number,
        parent_sequence_number,
        parent_path: String::new(),
        update_sequence_number: read_u64(record, 24),
        update_timestamp: format_filetime(read_i64(record, 32))?,
        update_reasons: format_flags(read_u32(record, 40), REASON_FLAGS),
        file_attributes: format_flags(read_u32(record, 52), ATTRIBUTE_FLAGS),
        offset_to_data: offset_to_data as u64,
        source_file: source_file.to_path_buf(),
    })
}

fn split_file_reference(reference: u64) -> (u64, u16) {
    (reference & 0x0000_FFFF_FFFF_FFFF, (reference >> 48) as u16)
}

fn format_filetime(filetime: i64) -> Option<String> {
    triage_core::timestamp::filetime_to_iso8601(filetime as i128)
}

fn format_flags(value: u32, flags: &[(u32, &str)]) -> String {
    flags
        .iter()
        .filter_map(|(flag, name)| (value & flag != 0).then_some(*name))
        .collect::<Vec<_>>()
        .join("|")
}

fn extension_for(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default()
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let code_units = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&code_units)
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(
        bytes[offset..offset + 2]
            .try_into()
            .expect("slice length checked"),
    )
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("slice length checked"),
    )
}

fn read_i64(bytes: &[u8], offset: usize) -> i64 {
    i64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("slice length checked"),
    )
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("slice length checked"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_FILETIME: i64 = 134_214_381_591_095_084;

    #[test]
    fn parses_first_sample_record() {
        if !std::path::Path::new("../../sample_files/$Extend/$UsnJrnl%3A$J").exists() {
            return; // sample evidence absent — skip (TriageSuite convention)
        }
        let records = parse_usn_journal("../../sample_files/$Extend/$UsnJrnl%3A$J").unwrap();
        let first = records.first().expect("expected at least one USN record");

        assert_eq!(first.name, "DLLHOST.EXE-7D5CE0CA.pf");
        assert_eq!(first.extension, ".pf");
        assert_eq!(first.entry_number, 2171);
        assert_eq!(first.sequence_number, 709);
        assert_eq!(first.parent_entry_number, 52874);
        assert_eq!(first.parent_sequence_number, 57);
        assert_eq!(first.parent_path, "");
        assert_eq!(first.update_sequence_number, 20040384512);
        assert_eq!(first.update_timestamp, "2026-04-23T17:15:59.1095084Z");
        assert_eq!(first.update_reasons, "DataExtend|DataTruncation");
        assert_eq!(first.file_attributes, "Archive|NotContentIndexed");
        assert_eq!(first.offset_to_data, 0);
    }

    #[test]
    fn skips_sparse_zero_padding_before_valid_v2_record() {
        let mut bytes = vec![0; 8];
        bytes.extend(valid_v2_record("after-padding.dat"));

        let records = parse_usn_bytes(bytes);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "after-padding.dat");
    }

    #[test]
    fn skips_malformed_short_record_before_valid_v2_record() {
        let mut bytes = vec![0; 32];
        write_u32(&mut bytes, 0, 32);
        bytes.extend(valid_v2_record("after-short.log"));

        let records = parse_usn_bytes(bytes);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "after-short.log");
    }

    #[test]
    fn skips_incomplete_trailing_record_without_panic() {
        let mut bytes = vec![0; 16];
        write_u32(&mut bytes, 0, 128);

        let records = parse_usn_bytes(bytes);

        assert!(records.is_empty());
    }

    #[test]
    fn skips_record_with_invalid_filename_bounds() {
        let mut bytes = valid_v2_record("bad-bounds.dat");
        write_u16(&mut bytes, 56, 128);

        let records = parse_usn_bytes(bytes);

        assert!(records.is_empty());
    }

    #[test]
    fn skips_unsupported_major_version() {
        let mut bytes = valid_v2_record("v3-record.dat");
        write_u16(&mut bytes, 4, 3);

        let records = parse_usn_bytes(bytes);

        assert!(records.is_empty());
    }

    #[test]
    fn preserves_filetime_100ns_precision() {
        let records = parse_usn_bytes(valid_v2_record("timestamp.dat"));

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].update_timestamp, "2026-04-23T17:15:59.1095084Z");
    }

    #[test]
    fn formats_all_v2_reason_flags() {
        assert_eq!(
            format_flags(0x00C0_0000, REASON_FLAGS),
            "TransactedChange|IntegrityChange"
        );
    }

    fn parse_usn_bytes(bytes: Vec<u8>) -> Vec<JEntryRecord> {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("$J");
        std::fs::write(&path, bytes).unwrap();
        parse_usn_journal(&path).unwrap()
    }

    fn valid_v2_record(name: &str) -> Vec<u8> {
        let name_bytes = name
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let record_length = MIN_RECORD_LENGTH + name_bytes.len();
        let mut record = vec![0; record_length];

        write_u32(&mut record, 0, record_length as u32);
        write_u16(&mut record, 4, 2);
        write_u64(&mut record, 8, (7_u64 << 48) | 42);
        write_u64(&mut record, 16, (8_u64 << 48) | 84);
        write_u64(&mut record, 24, 123_456);
        write_i64(&mut record, 32, SAMPLE_FILETIME);
        write_u32(&mut record, 40, 0x0000_0002);
        write_u32(&mut record, 52, 0x0000_0020);
        write_u16(&mut record, 56, name_bytes.len() as u16);
        write_u16(&mut record, 58, MIN_RECORD_LENGTH as u16);
        record[MIN_RECORD_LENGTH..].copy_from_slice(&name_bytes);

        record
    }

    fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_i64(bytes: &mut [u8], offset: usize, value: i64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
}
