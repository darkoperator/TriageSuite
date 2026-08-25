use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::{MftriageError, Result};
use serde::Serialize;

pub const MFT_RECORD_SIZE: usize = 1024;
const NTFS_SECTOR_SIZE: usize = 512;

const ATTRIBUTE_FLAGS: &[(u32, &str)] = &[
    (0x0000_0001, "ReadOnly"),
    (0x0000_0002, "Hidden"),
    (0x0000_0004, "System"),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameType {
    Posix,
    Windows,
    Dos,
    DosWindows,
    Unknown(u8),
}

impl std::fmt::Display for NameType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameType::Posix => f.write_str("Posix"),
            NameType::Windows => f.write_str("Windows"),
            NameType::Dos => f.write_str("Dos"),
            NameType::DosWindows => f.write_str("DosWindows"),
            NameType::Unknown(value) => write!(f, "Unknown({value})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecordHeader {
    pub entry_number: u64,
    pub sequence_number: u16,
    pub reference_count: u16,
    pub logfile_sequence_number: u64,
    pub first_attribute_offset: usize,
    pub flags: u16,
    pub used_size: usize,
    pub allocated_size: usize,
    pub base_record_reference: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeHeader {
    pub record_offset: usize,
    pub attribute_type: u32,
    pub record_length: usize,
    pub non_resident: bool,
    pub name_length: u8,
    pub name_offset: usize,
    pub attribute_id: u16,
    pub content_offset: usize,
    pub content_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MftRecord {
    pub entry_number: u64,
    pub sequence_number: u16,
    pub in_use: bool,
    pub parent_entry_number: u64,
    pub parent_sequence_number: Option<u16>,
    pub parent_path: String,
    pub file_name: String,
    pub extension: String,
    pub file_size: u64,
    pub reference_count: u16,
    pub reparse_target: String,
    pub is_directory: bool,
    pub has_ads: bool,
    pub is_ads: bool,
    #[serde(rename = "SI<FN")]
    pub timestomped: bool,
    #[serde(rename = "uSecZeros")]
    pub usec_zeros: bool,
    pub copied: bool,
    pub si_flags: String,
    pub name_type: String,
    pub created0x10: String,
    pub created0x30: String,
    pub last_modified0x10: String,
    pub last_modified0x30: String,
    pub last_record_change0x10: String,
    pub last_record_change0x30: String,
    pub last_access0x10: String,
    pub last_access0x30: String,
    pub update_sequence_number: i64,
    pub logfile_sequence_number: i64,
    pub security_id: u32,
    pub object_id_file_droid: String,
    pub logged_util_stream: String,
    pub zone_id_contents: String,
    pub source_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct FileListingRecord {
    pub full_path: String,
    pub extension: String,
    pub is_directory: bool,
    pub file_size: u64,
    pub created0x10: String,
    pub last_modified0x10: String,
    pub source_file: PathBuf,
}

impl From<&MftRecord> for FileListingRecord {
    fn from(record: &MftRecord) -> Self {
        Self {
            full_path: join_parent_path(&record.parent_path, &record.file_name),
            extension: record.extension.clone(),
            is_directory: record.is_directory,
            file_size: record.file_size,
            created0x10: record.created0x10.clone(),
            last_modified0x10: record.last_modified0x10.clone(),
            source_file: record.source_file.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MftParseOptions {
    pub include_dos_names: bool,
    pub include_all_file_name_timestamps: bool,
}

pub struct ParsedMft {
    pub records: Vec<MftRecord>,
    pub path_index: MftPathIndex,
}

impl ParsedMft {
    pub fn file_listing_records(&self) -> Vec<FileListingRecord> {
        self.records
            .iter()
            .filter(|record| !record.is_ads)
            .map(FileListingRecord::from)
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct MftPathIndex {
    entries: HashMap<u64, PathIndexEntry>,
}

#[derive(Debug, Clone)]
struct PathIndexEntry {
    sequence_number: u16,
    parent_entry_number: u64,
    parent_sequence_number: Option<u16>,
    file_name: String,
}

enum PathResolution {
    Resolved(String),
    MissingParent,
    Invalid,
}

#[derive(Debug, Clone, Copy, Default)]
struct StandardInformation {
    created: u64,
    modified: u64,
    record_changed: u64,
    accessed: u64,
    flags: u32,
    update_sequence_number: u64,
    security_id: u32,
}

#[derive(Debug, Clone)]
struct FileNameInformation {
    parent_entry_number: u64,
    parent_sequence_number: Option<u16>,
    created: u64,
    modified: u64,
    record_changed: u64,
    accessed: u64,
    allocated_size: u64,
    real_size: u64,
    flags: u32,
    name_type: NameType,
    file_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DataAttribute {
    name: Option<String>,
    size: u64,
    non_resident: bool,
}

pub fn parse_file_record_header(record: &[u8], entry_number: u64) -> Option<FileRecordHeader> {
    if record.len() < MFT_RECORD_SIZE || record.get(0..4)? != b"FILE" {
        return None;
    }

    let fixup_offset = read_u16(record, 0x04)? as usize;
    let fixup_count = read_u16(record, 0x06)? as usize;
    let logfile_sequence_number = read_u64(record, 0x08)?;
    let sequence_number = read_u16(record, 0x10)?;
    let reference_count = read_u16(record, 0x12)?;
    let first_attribute_offset = read_u16(record, 0x14)? as usize;
    let flags = read_u16(record, 0x16)?;
    let used_size = read_u32(record, 0x18)? as usize;
    let allocated_size = read_u32(record, 0x1c)? as usize;
    let base_record_reference = read_u64(record, 0x20)?;

    let fixup_bytes = fixup_count.checked_mul(2)?;
    let fixup_end = fixup_offset.checked_add(fixup_bytes)?;
    if fixup_count == 0 || fixup_end > record.len() {
        return None;
    }

    if allocated_size == 0 || allocated_size > MFT_RECORD_SIZE || used_size > allocated_size {
        return None;
    }

    if first_attribute_offset < 0x28 || first_attribute_offset > used_size {
        return None;
    }

    if first_attribute_offset > MFT_RECORD_SIZE {
        return None;
    }

    Some(FileRecordHeader {
        entry_number,
        sequence_number,
        reference_count,
        logfile_sequence_number,
        first_attribute_offset,
        flags,
        used_size,
        allocated_size,
        base_record_reference,
    })
}

pub fn apply_update_sequence_fixup(record: &mut [u8]) -> Result<()> {
    if record.len() < MFT_RECORD_SIZE {
        return Err(malformed_mft("FILE record is shorter than 1024 bytes"));
    }

    let record = &mut record[..MFT_RECORD_SIZE];

    if record.get(0..4) != Some(b"FILE") {
        return Err(malformed_mft("FILE record signature is missing"));
    }

    let fixup_offset = read_u16(record, 0x04)
        .ok_or_else(|| malformed_mft("FILE record fixup offset is missing"))?
        as usize;
    let fixup_count = read_u16(record, 0x06)
        .ok_or_else(|| malformed_mft("FILE record fixup count is missing"))?
        as usize;
    let fixup_bytes = fixup_count
        .checked_mul(2)
        .ok_or_else(|| malformed_mft("FILE record fixup array is too large"))?;
    let fixup_end = fixup_offset
        .checked_add(fixup_bytes)
        .ok_or_else(|| malformed_mft("FILE record fixup array overflows"))?;

    if fixup_count == 0 || fixup_end > MFT_RECORD_SIZE {
        return Err(malformed_mft("FILE record fixup array is out of bounds"));
    }

    let expected_fixup_count = MFT_RECORD_SIZE / NTFS_SECTOR_SIZE + 1;
    if fixup_count != expected_fixup_count {
        return Err(malformed_mft(
            "FILE record fixup count does not match record size",
        ));
    }

    let update_sequence_number: [u8; 2] = record
        .get(fixup_offset..fixup_offset + 2)
        .ok_or_else(|| malformed_mft("FILE record update sequence number is out of bounds"))?
        .try_into()
        .map_err(|_| malformed_mft("FILE record update sequence number is invalid"))?;

    for sector_index in 0..fixup_count - 1 {
        let sector_tail = (sector_index + 1)
            .checked_mul(NTFS_SECTOR_SIZE)
            .and_then(|offset| offset.checked_sub(2))
            .ok_or_else(|| malformed_mft("FILE record sector tail offset overflows"))?;
        let sector_tail_end = sector_tail
            .checked_add(2)
            .ok_or_else(|| malformed_mft("FILE record sector tail offset overflows"))?;
        if sector_tail_end > record.len() {
            return Err(malformed_mft("FILE record sector tail is out of bounds"));
        }

        if record[sector_tail..sector_tail_end] != update_sequence_number {
            return Err(malformed_mft(
                "FILE record sector tail does not match update sequence number",
            ));
        }

        let fixup_value_offset = fixup_offset + 2 + sector_index * 2;
        let fixup_value: [u8; 2] = record
            .get(fixup_value_offset..fixup_value_offset + 2)
            .ok_or_else(|| malformed_mft("FILE record fixup value is out of bounds"))?
            .try_into()
            .map_err(|_| malformed_mft("FILE record fixup value is invalid"))?;
        record[sector_tail..sector_tail_end].copy_from_slice(&fixup_value);
    }

    Ok(())
}

pub fn walk_attributes(record: &[u8], first_attribute_offset: usize) -> Vec<AttributeHeader> {
    let mut attributes = Vec::new();
    let mut offset = first_attribute_offset;

    while offset < record.len() {
        let Some(attribute_type) = read_u32(record, offset) else {
            break;
        };
        if attribute_type == 0xffff_ffff {
            break;
        }

        let Some(record_length) = read_u32(record, offset + 0x04).map(|value| value as usize)
        else {
            break;
        };
        if record_length == 0 {
            break;
        }

        let Some(attribute_end) = offset.checked_add(record_length) else {
            break;
        };
        if attribute_end > record.len() {
            break;
        }

        let attribute = &record[offset..attribute_end];
        if attribute.len() < 0x10 {
            break;
        }

        let non_resident_flag = attribute[0x08];
        let name_length = attribute[0x09];
        let Some(name_offset) = read_u16(attribute, 0x0a).map(|value| value as usize) else {
            offset = attribute_end;
            continue;
        };
        let Some(attribute_id) = read_u16(attribute, 0x0e) else {
            offset = attribute_end;
            continue;
        };
        let Some(name_end) = (name_length as usize)
            .checked_mul(2)
            .and_then(|name_bytes| name_offset.checked_add(name_bytes))
        else {
            offset = attribute_end;
            continue;
        };
        if name_end > record_length {
            offset = attribute_end;
            continue;
        }

        let non_resident = non_resident_flag != 0;
        let mut content_offset = 0;
        let mut content_size = 0;
        let mut include_attribute = true;

        if non_resident {
            if record_length < 0x38 {
                include_attribute = false;
            } else if let Some(real_size) = read_u64(attribute, 0x30) {
                content_size = real_size as usize;
            }
        } else {
            if record_length < 0x18 {
                offset = attribute_end;
                continue;
            }

            let Some(resident_content_size) = read_u32(attribute, 0x10).map(|value| value as usize)
            else {
                offset = attribute_end;
                continue;
            };
            let Some(resident_content_offset) =
                read_u16(attribute, 0x14).map(|value| value as usize)
            else {
                offset = attribute_end;
                continue;
            };
            let Some(content_end) = resident_content_offset.checked_add(resident_content_size)
            else {
                offset = attribute_end;
                continue;
            };

            let content_starts_before_named_header =
                name_length > 0 && resident_content_offset < name_end;
            if resident_content_offset < 0x18
                || content_starts_before_named_header
                || content_end > record_length
            {
                include_attribute = false;
            } else {
                content_offset = resident_content_offset;
                content_size = resident_content_size;
            }
        }

        if include_attribute {
            attributes.push(AttributeHeader {
                record_offset: offset,
                attribute_type,
                record_length,
                non_resident,
                name_length,
                name_offset,
                attribute_id,
                content_offset,
                content_size,
            });
        }

        offset = attribute_end;
    }

    attributes
}

pub fn parse_mft(path: impl AsRef<Path>, options: MftParseOptions) -> Result<ParsedMft> {
    parse_mft_with_progress(path, options, |_, _| {})
}

#[derive(Debug)]
pub enum MftVisitError<E> {
    Parse(MftriageError),
    Visitor(E),
}

/// Two-pass bounded-memory MFT visitor: the first pass builds only the path
/// index; the second parses and emits each logical row immediately.
pub fn visit_mft<E>(
    path: impl AsRef<Path>,
    options: MftParseOptions,
    mut visitor: impl FnMut(MftRecord) -> std::result::Result<(), E>,
) -> std::result::Result<MftPathIndex, MftVisitError<E>> {
    let path = path.as_ref();
    let mut index = MftPathIndex::default();
    let mut reader = std::io::BufReader::new(
        std::fs::File::open(path).map_err(|e| MftVisitError::Parse(e.into()))?,
    );
    let mut record = vec![0u8; MFT_RECORD_SIZE];
    let mut entry_number = 0u64;
    loop {
        match reader.read_exact(&mut record) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(MftVisitError::Parse(e.into())),
        }
        let mut fixed = record.clone();
        if apply_update_sequence_fixup(&mut fixed).is_ok() {
            if let Some(header) = parse_file_record_header(&fixed, entry_number) {
                index.insert_record(&header, &fixed);
            }
        }
        entry_number += 1;
    }

    let mut reader = std::io::BufReader::new(
        std::fs::File::open(path).map_err(|e| MftVisitError::Parse(e.into()))?,
    );
    entry_number = 0;
    loop {
        match reader.read_exact(&mut record) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(MftVisitError::Parse(e.into())),
        }
        let mut fixed = record.clone();
        if apply_update_sequence_fixup(&mut fixed).is_ok() {
            if let Some(header) = parse_file_record_header(&fixed, entry_number) {
                for row in parse_record_rows(&fixed, header.entry_number, path, options, &index) {
                    visitor(row).map_err(MftVisitError::Visitor)?;
                }
            }
        }
        entry_number += 1;
    }
    Ok(index)
}

pub fn parse_mft_with_progress(
    path: impl AsRef<Path>,
    options: MftParseOptions,
    on_progress: impl Fn(u64, u64),
) -> Result<ParsedMft> {
    let path = path.as_ref();
    let bytes = fs::read(path)?;
    let total = bytes.len() as u64;
    let mut fixed_records = Vec::new();

    for (entry_number, chunk) in bytes.chunks_exact(MFT_RECORD_SIZE).enumerate() {
        let mut record = chunk.to_vec();
        if apply_update_sequence_fixup(&mut record).is_err() {
            continue;
        }

        let Some(header) = parse_file_record_header(&record, entry_number as u64) else {
            continue;
        };

        fixed_records.push((header, record));
        on_progress((entry_number + 1) as u64 * MFT_RECORD_SIZE as u64, total);
    }

    let path_index = MftPathIndex::from_records(&fixed_records);
    let mut records = Vec::new();
    for (header, record) in &fixed_records {
        records.extend(parse_record_rows(
            record,
            header.entry_number,
            path,
            options,
            &path_index,
        ));
    }

    Ok(ParsedMft {
        records,
        path_index,
    })
}

impl MftPathIndex {
    fn insert_record(&mut self, header: &FileRecordHeader, record: &[u8]) {
        let record = &record[..header.used_size.min(record.len())];
        let file_names = parse_file_names(record, header.first_attribute_offset);
        let Some(file_name) = preferred_index_name(&file_names) else {
            return;
        };
        self.entries.insert(
            header.entry_number,
            PathIndexEntry {
                sequence_number: header.sequence_number,
                parent_entry_number: file_name.parent_entry_number,
                parent_sequence_number: file_name.parent_sequence_number,
                file_name: file_name.file_name.clone(),
            },
        );
    }

    fn from_records(records: &[(FileRecordHeader, Vec<u8>)]) -> Self {
        let mut index = Self::default();
        for (header, record) in records {
            let record = &record[..header.used_size];
            let file_names = parse_file_names(record, header.first_attribute_offset);
            let Some(file_name) = preferred_index_name(&file_names) else {
                continue;
            };

            index.entries.insert(
                header.entry_number,
                PathIndexEntry {
                    sequence_number: header.sequence_number,
                    parent_entry_number: file_name.parent_entry_number,
                    parent_sequence_number: file_name.parent_sequence_number,
                    file_name: file_name.file_name.clone(),
                },
            );
        }

        index
    }

    pub fn full_path(&self, entry_number: u64, sequence_number: u16) -> Option<String> {
        self.full_path_optional_sequence(entry_number, Some(sequence_number))
    }

    pub fn full_path_optional_sequence(
        &self,
        entry_number: u64,
        sequence_number: Option<u16>,
    ) -> Option<String> {
        if entry_number == 5 {
            return Some(".".to_string());
        }

        let mut seen = HashSet::new();
        match self.full_path_inner(entry_number, sequence_number, &mut seen) {
            PathResolution::Resolved(path) => Some(path),
            PathResolution::MissingParent | PathResolution::Invalid => None,
        }
    }

    fn full_path_inner(
        &self,
        entry_number: u64,
        sequence_number: Option<u16>,
        seen: &mut HashSet<u64>,
    ) -> PathResolution {
        if entry_number == 5 {
            return PathResolution::Resolved(".".to_string());
        }
        if !seen.insert(entry_number) {
            return PathResolution::Invalid;
        }

        let Some(entry) = self.entries.get(&entry_number) else {
            return PathResolution::MissingParent;
        };
        if sequence_number.is_some_and(|sequence| sequence != entry.sequence_number) {
            return PathResolution::Invalid;
        }

        let parent = match self.full_path_inner(
            entry.parent_entry_number,
            entry.parent_sequence_number,
            seen,
        ) {
            PathResolution::Resolved(path) => path,
            PathResolution::MissingParent => ".".to_string(),
            PathResolution::Invalid => return PathResolution::Invalid,
        };

        PathResolution::Resolved(join_parent_path(&parent, &entry.file_name))
    }
}

fn parse_record_rows(
    record: &[u8],
    entry_number: u64,
    source_file: &Path,
    options: MftParseOptions,
    path_index: &MftPathIndex,
) -> Vec<MftRecord> {
    let Some(header) = parse_file_record_header(record, entry_number) else {
        return Vec::new();
    };
    if header.base_record_reference != 0 {
        return Vec::new();
    }
    let record = &record[..header.used_size];

    let Some(standard_information) =
        parse_standard_information(record, header.first_attribute_offset)
    else {
        return Vec::new();
    };

    let mut file_names = parse_file_names(record, header.first_attribute_offset);
    file_names.sort_by(|left, right| {
        name_type_preference(left.name_type)
            .cmp(&name_type_preference(right.name_type))
            .then_with(|| right.real_size.cmp(&left.real_size))
    });
    let data_attributes = parse_data_attributes(record, header.first_attribute_offset);
    let unnamed_data = data_attributes
        .iter()
        .filter(|attribute| attribute.name.is_none())
        .max_by_key(|attribute| attribute.size);
    let named_data = data_attributes
        .iter()
        .filter(|attribute| attribute.name.is_some())
        .collect::<Vec<_>>();
    let has_ads = !named_data.is_empty();

    let mut rows = Vec::new();
    for file_name in file_names
        .into_iter()
        .filter(|file_name| options.include_dos_names || file_name.name_type != NameType::Dos)
    {
        let mut base_record = build_mft_record(
            &header,
            &standard_information,
            &file_name,
            source_file,
            options,
            path_index,
            unnamed_data,
        );
        base_record.has_ads = has_ads;
        rows.push(base_record.clone());

        for data_attribute in &named_data {
            let mut ads_record = base_record.clone();
            let Some(stream_name) = &data_attribute.name else {
                continue;
            };
            ads_record.file_name = format!("{}:{stream_name}", base_record.file_name);
            ads_record.extension = extension_for(&ads_record.file_name);
            ads_record.file_size = data_attribute.size;
            ads_record.has_ads = false;
            ads_record.is_ads = true;
            rows.push(ads_record);
        }
    }

    rows
}

fn build_mft_record(
    header: &FileRecordHeader,
    standard_information: &StandardInformation,
    file_name: &FileNameInformation,
    source_file: &Path,
    options: MftParseOptions,
    path_index: &MftPathIndex,
    unnamed_data: Option<&DataAttribute>,
) -> MftRecord {
    let created0x10 = format_filetime_or_empty(standard_information.created);
    let last_modified0x10 = format_filetime_or_empty(standard_information.modified);
    let last_record_change0x10 = format_filetime_or_empty(standard_information.record_changed);
    let last_access0x10 = format_filetime_or_empty(standard_information.accessed);

    let copied = standard_information.created == file_name.created
        && standard_information.modified == file_name.modified
        && standard_information.record_changed == file_name.record_changed
        && standard_information.accessed == file_name.accessed;
    let parent_path = path_index
        .full_path_optional_sequence(
            file_name.parent_entry_number,
            file_name.parent_sequence_number,
        )
        .unwrap_or_else(|| ".".to_string());

    MftRecord {
        entry_number: header.entry_number,
        sequence_number: header.sequence_number,
        in_use: header.flags & 0x0001 != 0,
        parent_entry_number: file_name.parent_entry_number,
        parent_sequence_number: file_name.parent_sequence_number,
        parent_path,
        file_name: file_name.file_name.clone(),
        extension: extension_for(&file_name.file_name),
        file_size: file_size_for(file_name, unnamed_data),
        reference_count: header.reference_count,
        reparse_target: String::new(),
        is_directory: header.flags & 0x0002 != 0 || file_name.flags & 0x0000_0010 != 0,
        has_ads: false,
        is_ads: false,
        timestomped: file_name.created > standard_information.created,
        usec_zeros: filetime_remainder_is_zero(standard_information.created)
            && filetime_remainder_is_zero(standard_information.modified),
        copied,
        si_flags: format_flags(standard_information.flags),
        name_type: file_name.name_type.to_string(),
        created0x10,
        created0x30: format_0x30_timestamp(
            file_name.created,
            standard_information.created,
            options.include_all_file_name_timestamps,
        ),
        last_modified0x10,
        last_modified0x30: format_0x30_timestamp(
            file_name.modified,
            standard_information.modified,
            options.include_all_file_name_timestamps,
        ),
        last_record_change0x10,
        last_record_change0x30: format_0x30_timestamp(
            file_name.record_changed,
            standard_information.record_changed,
            options.include_all_file_name_timestamps,
        ),
        last_access0x10,
        last_access0x30: format_0x30_timestamp(
            file_name.accessed,
            standard_information.accessed,
            options.include_all_file_name_timestamps,
        ),
        update_sequence_number: standard_information.update_sequence_number as i64,
        logfile_sequence_number: header.logfile_sequence_number as i64,
        security_id: standard_information.security_id,
        object_id_file_droid: String::new(),
        logged_util_stream: String::new(),
        zone_id_contents: String::new(),
        source_file: source_file.to_path_buf(),
    }
}

fn parse_standard_information(
    record: &[u8],
    first_attribute_offset: usize,
) -> Option<StandardInformation> {
    walk_attributes(record, first_attribute_offset)
        .into_iter()
        .filter(|attribute| attribute.attribute_type == 0x10 && !attribute.non_resident)
        .find_map(|attribute| {
            let content = attribute_content(record, attribute)?;
            Some(StandardInformation {
                created: read_u64(content, 0x00)?,
                modified: read_u64(content, 0x08)?,
                record_changed: read_u64(content, 0x10)?,
                accessed: read_u64(content, 0x18)?,
                flags: read_u32(content, 0x20)?,
                update_sequence_number: read_u64(content, 0x28).unwrap_or_default(),
                security_id: read_u32(content, 0x30).unwrap_or_default(),
            })
        })
}

fn parse_file_names(record: &[u8], first_attribute_offset: usize) -> Vec<FileNameInformation> {
    walk_attributes(record, first_attribute_offset)
        .into_iter()
        .filter(|attribute| attribute.attribute_type == 0x30 && !attribute.non_resident)
        .filter_map(|attribute| parse_file_name(attribute_content(record, attribute)?))
        .collect()
}

fn parse_data_attributes(record: &[u8], first_attribute_offset: usize) -> Vec<DataAttribute> {
    walk_attributes(record, first_attribute_offset)
        .into_iter()
        .filter(|attribute| attribute.attribute_type == 0x80)
        .filter_map(|attribute| {
            let name = attribute_name(record, attribute)?;
            let name = (!name.is_empty()).then_some(name);
            Some(DataAttribute {
                name,
                size: attribute.content_size as u64,
                non_resident: attribute.non_resident,
            })
        })
        .collect()
}

fn parse_file_name(content: &[u8]) -> Option<FileNameInformation> {
    let (parent_entry_number, parent_sequence_number) =
        split_file_reference(read_u64(content, 0x00)?);
    let name_length = *content.get(0x40)? as usize;
    let namespace = *content.get(0x41)?;
    let name_bytes_len = name_length.checked_mul(2)?;
    let name_end = 0x42_usize.checked_add(name_bytes_len)?;
    if name_end > content.len() {
        return None;
    }

    Some(FileNameInformation {
        parent_entry_number,
        parent_sequence_number: Some(parent_sequence_number),
        created: read_u64(content, 0x08)?,
        modified: read_u64(content, 0x10)?,
        record_changed: read_u64(content, 0x18)?,
        accessed: read_u64(content, 0x20)?,
        allocated_size: read_u64(content, 0x28)?,
        real_size: read_u64(content, 0x30)?,
        flags: read_u32(content, 0x38)?,
        name_type: name_type_from_namespace(namespace),
        file_name: decode_utf16le(&content[0x42..name_end]),
    })
}

fn file_size_for(file_name: &FileNameInformation, unnamed_data: Option<&DataAttribute>) -> u64 {
    if let Some(data_attribute) = unnamed_data {
        if data_attribute.non_resident {
            return data_attribute.size;
        }
        return file_name.real_size.max(data_attribute.size);
    }

    if file_name.real_size != 0 {
        file_name.real_size
    } else {
        file_name.allocated_size
    }
}

fn attribute_content(record: &[u8], attribute: AttributeHeader) -> Option<&[u8]> {
    let content_start = attribute
        .record_offset
        .checked_add(attribute.content_offset)?;
    let content_end = content_start.checked_add(attribute.content_size)?;
    record.get(content_start..content_end)
}

fn attribute_name(record: &[u8], attribute: AttributeHeader) -> Option<String> {
    let name_start = attribute.record_offset.checked_add(attribute.name_offset)?;
    let name_bytes = (attribute.name_length as usize).checked_mul(2)?;
    let name_end = name_start.checked_add(name_bytes)?;
    let attribute_end = attribute
        .record_offset
        .checked_add(attribute.record_length)?;
    if name_end > attribute_end {
        return None;
    }

    record.get(name_start..name_end).map(decode_utf16le)
}

fn preferred_index_name(file_names: &[FileNameInformation]) -> Option<&FileNameInformation> {
    file_names
        .iter()
        .find(|file_name| file_name.name_type == NameType::DosWindows)
        .or_else(|| {
            file_names
                .iter()
                .find(|file_name| file_name.name_type == NameType::Windows)
        })
        .or_else(|| {
            file_names
                .iter()
                .find(|file_name| file_name.name_type == NameType::Posix)
        })
}

fn split_file_reference(reference: u64) -> (u64, u16) {
    (reference & 0x0000_FFFF_FFFF_FFFF, (reference >> 48) as u16)
}

fn name_type_from_namespace(namespace: u8) -> NameType {
    match namespace {
        0 => NameType::Posix,
        1 => NameType::Windows,
        2 => NameType::Dos,
        3 => NameType::DosWindows,
        value => NameType::Unknown(value),
    }
}

fn name_type_preference(name_type: NameType) -> u8 {
    match name_type {
        NameType::DosWindows => 0,
        NameType::Windows => 1,
        NameType::Posix => 2,
        NameType::Dos => 3,
        NameType::Unknown(_) => 4,
    }
}

fn format_0x30_timestamp(filetime: u64, si_filetime: u64, include_all: bool) -> String {
    if include_all || filetime != si_filetime {
        format_filetime_or_empty(filetime)
    } else {
        String::new()
    }
}

fn format_filetime_or_empty(filetime: u64) -> String {
    format_filetime(filetime).unwrap_or_default()
}

fn format_filetime(filetime: u64) -> Option<String> {
    triage_core::timestamp::filetime_to_iso8601(filetime as i128)
}

fn filetime_remainder_is_zero(filetime: u64) -> bool {
    filetime.is_multiple_of(10_000_000)
}

fn format_flags(value: u32) -> String {
    ATTRIBUTE_FLAGS
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

fn join_parent_path(parent: &str, file_name: &str) -> String {
    if parent == "." {
        format!(".\\{file_name}")
    } else {
        format!("{parent}\\{file_name}")
    }
}

fn malformed_mft(message: impl Into<String>) -> MftriageError {
    MftriageError::MalformedArtifact {
        path: PathBuf::from("$MFT"),
        message: message.into(),
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let value = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes(value.try_into().ok()?))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let value = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes(value.try_into().ok()?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let value = bytes.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_le_bytes(value.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FILETIME: u64 = 132_325_955_060_000_000;

    #[test]
    fn parses_first_file_record_header() {
        if !std::path::Path::new("../../sample_files/$MFT").exists() {
            return; // sample evidence absent — skip (TriageSuite convention)
        }
        let bytes = std::fs::read("../../sample_files/$MFT").unwrap();
        let header = parse_file_record_header(&bytes[..MFT_RECORD_SIZE], 0).unwrap();

        assert_eq!(header.entry_number, 0);
        assert_eq!(header.sequence_number, 1);
        assert_eq!(header.first_attribute_offset, 56);
        assert_eq!(header.flags, 0x0001);
        assert_eq!(header.used_size, 600);
        assert_eq!(header.allocated_size, 1024);
        assert_eq!(header.base_record_reference, 0);
    }

    #[test]
    fn rejects_invalid_file_record_signature() {
        let mut record = vec![0; MFT_RECORD_SIZE];
        record[0..4].copy_from_slice(b"BAAD");

        assert_eq!(parse_file_record_header(&record, 0), None);
    }

    #[test]
    fn rejects_short_file_record() {
        if !std::path::Path::new("../../sample_files/$MFT").exists() {
            return; // sample evidence absent — skip (TriageSuite convention)
        }
        let bytes = std::fs::read("../../sample_files/$MFT").unwrap();

        assert_eq!(
            parse_file_record_header(&bytes[..MFT_RECORD_SIZE - 1], 0),
            None
        );
    }

    #[test]
    fn applies_fixup_for_first_file_record() {
        if !std::path::Path::new("../../sample_files/$MFT").exists() {
            return; // sample evidence absent — skip (TriageSuite convention)
        }
        let bytes = std::fs::read("../../sample_files/$MFT").unwrap();
        let mut record = bytes[..MFT_RECORD_SIZE].to_vec();
        apply_update_sequence_fixup(&mut record).unwrap();

        assert_eq!(&record[510..512], &[0xe1, 0xeb]);
        assert_eq!(&record[1022..1024], &[0x00, 0x00]);
    }

    #[test]
    fn walks_first_record_attributes() {
        if !std::path::Path::new("../../sample_files/$MFT").exists() {
            return; // sample evidence absent — skip (TriageSuite convention)
        }
        let bytes = std::fs::read("../../sample_files/$MFT").unwrap();
        let mut record = bytes[..MFT_RECORD_SIZE].to_vec();
        apply_update_sequence_fixup(&mut record).unwrap();
        let header = parse_file_record_header(&record, 0).unwrap();
        let attributes = walk_attributes(&record, header.first_attribute_offset);

        assert!(attributes
            .iter()
            .any(|attribute| attribute.attribute_type == 0x10));
        assert!(attributes
            .iter()
            .any(|attribute| attribute.attribute_type == 0x30));
        assert!(attributes
            .iter()
            .any(|attribute| attribute.attribute_type == 0x80));
    }

    #[test]
    fn rejects_fixup_when_sector_tail_does_not_match_update_sequence_number() {
        if !std::path::Path::new("../../sample_files/$MFT").exists() {
            return; // sample evidence absent — skip (TriageSuite convention)
        }
        let bytes = std::fs::read("../../sample_files/$MFT").unwrap();
        let mut record = bytes[..MFT_RECORD_SIZE].to_vec();
        record[510] ^= 0xff;

        assert!(apply_update_sequence_fixup(&mut record).is_err());
    }

    #[test]
    fn rejects_fixup_array_outside_first_record_when_buffer_is_larger() {
        if !std::path::Path::new("../../sample_files/$MFT").exists() {
            return; // sample evidence absent — skip (TriageSuite convention)
        }
        let bytes = std::fs::read("../../sample_files/$MFT").unwrap();
        let mut record = bytes[..MFT_RECORD_SIZE].to_vec();
        record.resize(MFT_RECORD_SIZE * 2, 0);
        record[0x04..0x06].copy_from_slice(&(MFT_RECORD_SIZE as u16 + 8).to_le_bytes());

        assert!(apply_update_sequence_fixup(&mut record).is_err());
    }

    #[test]
    fn skips_short_resident_attribute_without_reading_next_attribute_bytes() {
        let mut record = vec![0; MFT_RECORD_SIZE];
        record[0..4].copy_from_slice(b"FILE");
        record[0x38..0x3c].copy_from_slice(&0x10_u32.to_le_bytes());
        record[0x3c..0x40].copy_from_slice(&0x10_u32.to_le_bytes());
        record[0x40] = 0;
        record[0x48..0x4c].copy_from_slice(&0x30_u32.to_le_bytes());

        let attributes = walk_attributes(&record, 0x38);

        assert!(attributes.is_empty());
    }

    #[test]
    fn skips_short_non_resident_attribute_without_reading_next_attribute_bytes() {
        let mut record = vec![0; MFT_RECORD_SIZE];
        record[0..4].copy_from_slice(b"FILE");
        record[0x38..0x3c].copy_from_slice(&0x80_u32.to_le_bytes());
        record[0x3c..0x40].copy_from_slice(&0x30_u32.to_le_bytes());
        record[0x40] = 1;
        record[0x68..0x70].copy_from_slice(&123_u64.to_le_bytes());

        let attributes = walk_attributes(&record, 0x38);

        assert!(attributes.is_empty());
    }

    #[test]
    fn parses_first_mft_rows() {
        if !std::path::Path::new("../../sample_files/$MFT").exists() {
            return; // sample evidence absent — skip (TriageSuite convention)
        }
        let parsed = parse_mft("../../sample_files/$MFT", MftParseOptions::default()).unwrap();
        let first = &parsed.records[0];

        assert_eq!(first.entry_number, 0);
        assert_eq!(first.sequence_number, 1);
        assert!(first.in_use);
        assert_eq!(first.parent_entry_number, 5);
        assert_eq!(first.parent_sequence_number, Some(5));
        assert_eq!(first.parent_path, ".");
        assert_eq!(first.file_name, "$MFT");
        assert_eq!(first.extension, "");
        assert_eq!(first.file_size, 1_084_227_584);
        assert_eq!(first.si_flags, "Hidden|System");
        assert_eq!(first.name_type, "DosWindows");
        assert_eq!(first.last_modified0x10, "2024-01-31T10:34:13.3529500Z");
        assert_eq!(first.last_modified0x30, "");
        assert_eq!(first.logfile_sequence_number, 26_812_764_244);
    }

    #[test]
    fn file_listing_uses_parent_paths() {
        let record = synthetic_mft_record();
        let listing = FileListingRecord::from(&record);

        assert_eq!(listing.full_path, ".\\header.txt");
        assert_eq!(listing.source_file, record.source_file);
    }

    #[test]
    fn sn_option_includes_dos_names() {
        let record = build_record_with_file_names(&[(NameType::Dos, "DOSNAME.TXT")], false);
        let without = parse_record_rows(
            &record,
            49,
            PathBuf::from("synthetic_mft").as_path(),
            MftParseOptions::default(),
            &MftPathIndex::default(),
        );
        let with = parse_record_rows(
            &record,
            49,
            PathBuf::from("synthetic_mft").as_path(),
            MftParseOptions {
                include_dos_names: true,
                include_all_file_name_timestamps: false,
            },
            &MftPathIndex::default(),
        );

        assert_eq!(without.len(), 0);
        assert_eq!(with.len(), 1);
    }

    #[test]
    fn default_omits_dos_only_names_but_option_includes_them() {
        let record = build_record_with_file_names(&[(NameType::Dos, "DOSNAME.TXT")], false);
        let without = parse_record_rows(
            &record,
            42,
            PathBuf::from("synthetic_mft").as_path(),
            MftParseOptions::default(),
            &MftPathIndex::default(),
        );
        let with = parse_record_rows(
            &record,
            42,
            PathBuf::from("synthetic_mft").as_path(),
            MftParseOptions {
                include_dos_names: true,
                include_all_file_name_timestamps: false,
            },
            &MftPathIndex::default(),
        );

        assert!(without.is_empty());
        assert_eq!(with.len(), 1);
        assert_eq!(with[0].file_name, "DOSNAME.TXT");
        assert_eq!(with[0].name_type, "Dos");
    }

    #[test]
    fn resident_unnamed_data_size_overrides_smaller_file_name_size() {
        let mut record = build_record_with_file_names(&[(NameType::Windows, "data.bin")], true);
        set_first_file_name_real_size(&mut record, 16);
        append_resident_attribute(&mut record, 0x80, None, &[0xab; 64]);

        let rows = parse_record_rows(
            &record,
            46,
            PathBuf::from("synthetic_mft").as_path(),
            MftParseOptions::default(),
            &MftPathIndex::default(),
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file_size, 64);
        assert!(!rows[0].has_ads);
        assert!(!rows[0].is_ads);
    }

    #[test]
    fn named_data_creates_ads_row_and_marks_base_record() {
        let mut record = build_record_with_file_names(&[(NameType::Windows, "base.txt")], true);
        append_resident_attribute(&mut record, 0x80, Some("Zone.Identifier"), b"ZoneId=3");

        let rows = parse_record_rows(
            &record,
            47,
            PathBuf::from("synthetic_mft").as_path(),
            MftParseOptions::default(),
            &MftPathIndex::default(),
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].file_name, "base.txt");
        assert!(rows[0].has_ads);
        assert!(!rows[0].is_ads);
        assert_eq!(rows[1].file_name, "base.txt:Zone.Identifier");
        assert_eq!(rows[1].file_size, 8);
        assert!(!rows[1].has_ads);
        assert!(rows[1].is_ads);
        assert_eq!(rows[1].parent_path, rows[0].parent_path);
        assert_eq!(rows[1].created0x10, rows[0].created0x10);
        assert_eq!(rows[1].last_modified0x10, rows[0].last_modified0x10);
    }

    #[test]
    fn include_all_file_name_timestamps_controls_equal_0x30_fields() {
        let record = build_record_with_file_names(&[(NameType::Windows, "same.txt")], true);
        let default_rows = parse_record_rows(
            &record,
            43,
            PathBuf::from("synthetic_mft").as_path(),
            MftParseOptions::default(),
            &MftPathIndex::default(),
        );
        let all_rows = parse_record_rows(
            &record,
            43,
            PathBuf::from("synthetic_mft").as_path(),
            MftParseOptions {
                include_dos_names: false,
                include_all_file_name_timestamps: true,
            },
            &MftPathIndex::default(),
        );

        assert_eq!(default_rows[0].created0x30, "");
        assert_eq!(default_rows[0].last_modified0x30, "");
        assert_eq!(all_rows[0].created0x30, "2020-04-29T00:58:26.0000000Z");
        assert_eq!(
            all_rows[0].last_modified0x30,
            "2020-04-29T00:58:26.0000000Z"
        );
    }

    #[test]
    fn path_index_rejects_cycles_instead_of_masking_as_root_relative() {
        let mut index = MftPathIndex::default();
        index.entries.insert(
            10,
            PathIndexEntry {
                sequence_number: 1,
                parent_entry_number: 11,
                parent_sequence_number: Some(1),
                file_name: "a".to_string(),
            },
        );
        index.entries.insert(
            11,
            PathIndexEntry {
                sequence_number: 1,
                parent_entry_number: 10,
                parent_sequence_number: Some(1),
                file_name: "b".to_string(),
            },
        );

        assert_eq!(index.full_path(10, 1), None);
    }

    #[test]
    fn parse_record_rows_ignores_attributes_after_used_size() {
        let mut record = build_record_with_file_names(&[], true);
        let original_used_size = read_u32(&record, 0x18).unwrap() as usize;
        let mut slack_offset = original_used_size;
        let fn_content = build_file_name_content(NameType::Windows, "slack.txt", true);
        write_resident_attribute(&mut record, &mut slack_offset, 0x30, &fn_content);

        let rows = parse_record_rows(
            &record,
            44,
            PathBuf::from("synthetic_mft").as_path(),
            MftParseOptions::default(),
            &MftPathIndex::default(),
        );

        assert!(rows.is_empty());
    }

    #[test]
    fn walk_attributes_rejects_resident_content_inside_attribute_header() {
        let mut record = vec![0; MFT_RECORD_SIZE];
        record[0..4].copy_from_slice(b"FILE");
        record[0x38..0x3c].copy_from_slice(&0x10_u32.to_le_bytes());
        record[0x3c..0x40].copy_from_slice(&0x30_u32.to_le_bytes());
        record[0x40] = 0;
        record[0x48..0x4c].copy_from_slice(&8_u32.to_le_bytes());
        record[0x4c..0x4e].copy_from_slice(&0x10_u16.to_le_bytes());

        let attributes = walk_attributes(&record, 0x38);

        assert!(attributes.is_empty());
    }

    #[test]
    fn walk_attributes_rejects_named_resident_content_inside_name_area() {
        let mut record = vec![0; MFT_RECORD_SIZE];
        record[0..4].copy_from_slice(b"FILE");
        record[0x38..0x3c].copy_from_slice(&0x10_u32.to_le_bytes());
        record[0x3c..0x40].copy_from_slice(&0x20_u32.to_le_bytes());
        record[0x40] = 0;
        record[0x41] = 2;
        record[0x42..0x44].copy_from_slice(&0x18_u16.to_le_bytes());
        record[0x48..0x4c].copy_from_slice(&4_u32.to_le_bytes());
        record[0x4c..0x4e].copy_from_slice(&0x1a_u16.to_le_bytes());

        let attributes = walk_attributes(&record, 0x38);

        assert!(attributes.is_empty());
    }

    #[test]
    fn mft_records_preserve_48_bit_parent_entry_numbers() {
        let mut record = build_record_with_file_names(&[(NameType::Windows, "high.txt")], true);
        let header = parse_file_record_header(&record, 45).unwrap();
        let fn_attribute =
            walk_attributes(&record[..header.used_size], header.first_attribute_offset)
                .into_iter()
                .find(|attribute| attribute.attribute_type == 0x30)
                .unwrap();
        let content_start = fn_attribute.record_offset + fn_attribute.content_offset;
        let high_parent = u32::MAX as u64 + 17;
        let high_parent_reference = (5_u64 << 48) | high_parent;
        record[content_start..content_start + 8]
            .copy_from_slice(&high_parent_reference.to_le_bytes());

        let rows = parse_record_rows(
            &record,
            45,
            PathBuf::from("synthetic_mft").as_path(),
            MftParseOptions::default(),
            &MftPathIndex::default(),
        );

        assert_eq!(rows[0].parent_entry_number, high_parent);
    }

    #[test]
    fn mft_records_preserve_entry_numbers_above_u32() {
        let record = build_record_with_file_names(&[(NameType::Windows, "entry.txt")], true);
        let high_entry = u32::MAX as u64 + 33;

        let rows = parse_record_rows(
            &record,
            high_entry,
            PathBuf::from("synthetic_mft").as_path(),
            MftParseOptions::default(),
            &MftPathIndex::default(),
        );

        assert_eq!(rows[0].entry_number, high_entry);
    }

    fn build_record_with_file_names(names: &[(NameType, &str)], equal_timestamps: bool) -> Vec<u8> {
        let mut record = vec![0; MFT_RECORD_SIZE];
        record[0..4].copy_from_slice(b"FILE");
        record[0x04..0x06].copy_from_slice(&0x30_u16.to_le_bytes());
        record[0x06..0x08].copy_from_slice(&3_u16.to_le_bytes());
        record[0x08..0x10].copy_from_slice(&123_u64.to_le_bytes());
        record[0x10..0x12].copy_from_slice(&1_u16.to_le_bytes());
        record[0x12..0x14].copy_from_slice(&1_u16.to_le_bytes());
        record[0x14..0x16].copy_from_slice(&0x38_u16.to_le_bytes());
        record[0x16..0x18].copy_from_slice(&1_u16.to_le_bytes());
        record[0x1c..0x20].copy_from_slice(&(MFT_RECORD_SIZE as u32).to_le_bytes());

        let mut offset = 0x38;
        let si_content = build_standard_information_content();
        write_resident_attribute(&mut record, &mut offset, 0x10, &si_content);
        for (name_type, name) in names {
            let fn_content = build_file_name_content(*name_type, name, equal_timestamps);
            write_resident_attribute(&mut record, &mut offset, 0x30, &fn_content);
        }
        record[offset..offset + 4].copy_from_slice(&0xffff_ffff_u32.to_le_bytes());
        record[0x18..0x1c].copy_from_slice(&(offset as u32 + 4).to_le_bytes());

        record
    }

    fn build_standard_information_content() -> Vec<u8> {
        let mut content = vec![0; 0x38];
        for offset in [0x00, 0x08, 0x10, 0x18] {
            content[offset..offset + 8].copy_from_slice(&TEST_FILETIME.to_le_bytes());
        }
        content[0x20..0x24].copy_from_slice(&0x0000_0020_u32.to_le_bytes());
        content[0x28..0x30].copy_from_slice(&99_u64.to_le_bytes());
        content[0x30..0x34].copy_from_slice(&7_u32.to_le_bytes());
        content
    }

    fn build_file_name_content(name_type: NameType, name: &str, equal_timestamps: bool) -> Vec<u8> {
        let name_utf16 = name.encode_utf16().collect::<Vec<_>>();
        let mut content = vec![0; 0x42 + name_utf16.len() * 2];
        let parent_reference = (5_u64 << 48) | 5;
        content[0x00..0x08].copy_from_slice(&parent_reference.to_le_bytes());
        let fn_filetime = if equal_timestamps {
            TEST_FILETIME
        } else {
            TEST_FILETIME + 10_000_000
        };
        for offset in [0x08, 0x10, 0x18, 0x20] {
            content[offset..offset + 8].copy_from_slice(&fn_filetime.to_le_bytes());
        }
        content[0x28..0x30].copy_from_slice(&4096_u64.to_le_bytes());
        content[0x30..0x38].copy_from_slice(&1234_u64.to_le_bytes());
        content[0x38..0x3c].copy_from_slice(&0x0000_0020_u32.to_le_bytes());
        content[0x40] = name_utf16.len() as u8;
        content[0x41] = namespace_for(name_type);
        for (index, code_unit) in name_utf16.into_iter().enumerate() {
            let offset = 0x42 + index * 2;
            content[offset..offset + 2].copy_from_slice(&code_unit.to_le_bytes());
        }
        content
    }

    fn namespace_for(name_type: NameType) -> u8 {
        match name_type {
            NameType::Posix => 0,
            NameType::Windows => 1,
            NameType::Dos => 2,
            NameType::DosWindows => 3,
            NameType::Unknown(value) => value,
        }
    }

    fn write_resident_attribute(
        record: &mut [u8],
        offset: &mut usize,
        attribute_type: u32,
        content: &[u8],
    ) {
        let record_length = 0x18 + content.len();
        record[*offset..*offset + 4].copy_from_slice(&attribute_type.to_le_bytes());
        record[*offset + 4..*offset + 8].copy_from_slice(&(record_length as u32).to_le_bytes());
        record[*offset + 8] = 0;
        record[*offset + 0x10..*offset + 0x14]
            .copy_from_slice(&(content.len() as u32).to_le_bytes());
        record[*offset + 0x14..*offset + 0x16].copy_from_slice(&0x18_u16.to_le_bytes());
        record[*offset + 0x18..*offset + 0x18 + content.len()].copy_from_slice(content);
        *offset += record_length;
    }

    fn append_resident_attribute(
        record: &mut [u8],
        attribute_type: u32,
        name: Option<&str>,
        content: &[u8],
    ) {
        let mut offset = read_u32(record, 0x18).unwrap() as usize - 4;
        write_resident_attribute_with_name(record, &mut offset, attribute_type, name, content);
        record[offset..offset + 4].copy_from_slice(&0xffff_ffff_u32.to_le_bytes());
        record[0x18..0x1c].copy_from_slice(&(offset as u32 + 4).to_le_bytes());
    }

    fn set_first_file_name_real_size(record: &mut [u8], real_size: u64) {
        let header = parse_file_record_header(record, 0).unwrap();
        let attribute = walk_attributes(&record[..header.used_size], header.first_attribute_offset)
            .into_iter()
            .find(|attribute| attribute.attribute_type == 0x30)
            .unwrap();
        let real_size_offset = attribute.record_offset + attribute.content_offset + 0x30;
        record[real_size_offset..real_size_offset + 8].copy_from_slice(&real_size.to_le_bytes());
    }

    fn write_resident_attribute_with_name(
        record: &mut [u8],
        offset: &mut usize,
        attribute_type: u32,
        name: Option<&str>,
        content: &[u8],
    ) {
        let name_utf16 = name.unwrap_or_default().encode_utf16().collect::<Vec<_>>();
        let name_bytes_len = name_utf16.len() * 2;
        let content_offset = 0x18 + name_bytes_len;
        let record_length = content_offset + content.len();
        record[*offset..*offset + 4].copy_from_slice(&attribute_type.to_le_bytes());
        record[*offset + 4..*offset + 8].copy_from_slice(&(record_length as u32).to_le_bytes());
        record[*offset + 8] = 0;
        record[*offset + 9] = name_utf16.len() as u8;
        record[*offset + 0x0a..*offset + 0x0c].copy_from_slice(&0x18_u16.to_le_bytes());
        record[*offset + 0x10..*offset + 0x14]
            .copy_from_slice(&(content.len() as u32).to_le_bytes());
        record[*offset + 0x14..*offset + 0x16]
            .copy_from_slice(&(content_offset as u16).to_le_bytes());
        for (index, code_unit) in name_utf16.into_iter().enumerate() {
            let name_offset = *offset + 0x18 + index * 2;
            record[name_offset..name_offset + 2].copy_from_slice(&code_unit.to_le_bytes());
        }
        record[*offset + content_offset..*offset + content_offset + content.len()]
            .copy_from_slice(content);
        *offset += record_length;
    }

    fn synthetic_mft_record() -> MftRecord {
        let record = build_record_with_file_names(&[(NameType::Windows, "header.txt")], true);
        parse_record_rows(
            &record,
            48,
            PathBuf::from("synthetic_mft").as_path(),
            MftParseOptions::default(),
            &MftPathIndex::default(),
        )
        .into_iter()
        .next()
        .unwrap()
    }
}
