//! Windows Shell Link (.lnk) format parser.

pub mod extradata;
pub mod header;
pub mod linkinfo;
pub mod stringdata;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("not a shell link (bad signature or class id)")]
    BadSignature,
    #[error("truncated or corrupt structure: {0}")]
    Corrupt(&'static str),
}

/// A parsed Shell Link file: the fixed header, the LinkTargetIDList (rendered
/// via the shell-item engine), the LinkInfo (volume/network paths), and the
/// StringData blocks. Extra data blocks are filled in by a later task.
pub struct LnkFile {
    pub header: header::Header,
    pub target_ids: Vec<triage_shellitems::ShellItem>,
    pub link_info: Option<linkinfo::LinkInfo>,
    pub string_data: stringdata::StringData,
    /// Byte offset at which ExtraData begins (i.e. immediately after StringData).
    /// `extra_data` is parsed from `raw[extra_data_offset..]`.
    pub extra_data_offset: usize,
    /// ExtraData blocks: the ordered list of block types present plus the
    /// decoded TrackerDataBlock (machine id, MAC, creation time).
    pub extra_data: extradata::ExtraData,
}

/// Parse a Shell Link from its raw bytes using the default codepage (1252).
pub fn parse(data: &[u8]) -> Result<LnkFile, ParseError> {
    parse_with_codepage(data, 1252)
}

/// Parse a Shell Link from its raw bytes: the fixed header, the
/// LinkTargetIDList (when `HasTargetIdList`), the LinkInfo (when `HasLinkInfo`
/// and not `ForceNoLinkInfo`), and the StringData blocks. Bounds-checked
/// throughout. Only header-level failures error; a malformed LinkInfo or
/// StringData section degrades to a partial/empty section so an otherwise-good
/// link still parses, matching LECmd's leniency.
pub fn parse_with_codepage(data: &[u8], codepage: u16) -> Result<LnkFile, ParseError> {
    let header = header::Header::parse(data)?;
    let mut off = header::Header::SIZE;
    let mut target_ids = Vec::new();
    if header.has(header::DataFlag::HasTargetIdList) {
        let idlist_size = read_u16(data, off)? as usize;
        off += 2;
        let end = off
            .checked_add(idlist_size)
            .ok_or(ParseError::Corrupt("idlist range"))?;
        let idlist = data
            .get(off..end)
            .ok_or(ParseError::Corrupt("idlist truncated"))?;
        target_ids = triage_shellitems::parse_id_list_with_codepage(idlist, codepage);
        off = end;
    }

    let mut link_info = None;
    if header.has(header::DataFlag::HasLinkInfo) && !header.has(header::DataFlag::ForceNoLinkInfo) {
        // Pre-read the LinkInfo total size so we can step past a partially
        // corrupt block and still reach StringData (LECmd leniency).
        let total_size = read_u32(data, off).ok().map(|s| s as usize);
        match linkinfo::parse(data, off, codepage) {
            Ok((li, len)) => {
                off += len;
                link_info = Some(li);
            }
            Err(_) => {
                link_info = Some(linkinfo::LinkInfo::default());
                match total_size {
                    Some(sz) if sz >= 4 && off.checked_add(sz).is_some_and(|e| e <= data.len()) => {
                        off += sz; // step past corrupt block, continue to StringData
                    }
                    _ => {
                        // outer size unreadable/insane — cannot locate StringData
                        return Ok(LnkFile {
                            header,
                            target_ids,
                            link_info,
                            string_data: stringdata::StringData::default(),
                            extra_data: extradata::parse(data, off),
                            extra_data_offset: off,
                        });
                    }
                }
            }
        }
    }

    let is_unicode = header.has(header::DataFlag::IsUnicode);
    let (string_data, consumed) =
        stringdata::parse(data, off, header.data_flags, is_unicode, codepage)
            .unwrap_or((stringdata::StringData::default(), 0));
    off += consumed;

    let extra_data = extradata::parse(data, off);

    Ok(LnkFile {
        header,
        target_ids,
        link_info,
        string_data,
        extra_data,
        extra_data_offset: off,
    })
}

/// Read a little-endian u32 at `off`, bounds-checked.
pub(crate) fn read_u32(d: &[u8], off: usize) -> Result<u32, ParseError> {
    let end = off.checked_add(4).ok_or(ParseError::Corrupt("u32 off"))?;
    let b = d.get(off..end).ok_or(ParseError::Corrupt("u32 read"))?;
    Ok(u32::from_le_bytes(b.try_into().unwrap()))
}

/// Read a little-endian u64 at `off`, bounds-checked.
pub(crate) fn read_u64(d: &[u8], off: usize) -> Result<u64, ParseError> {
    let end = off.checked_add(8).ok_or(ParseError::Corrupt("u64 off"))?;
    let b = d.get(off..end).ok_or(ParseError::Corrupt("u64 read"))?;
    Ok(u64::from_le_bytes(b.try_into().unwrap()))
}

/// Read a little-endian u16 at `off`, bounds-checked.
pub(crate) fn read_u16(d: &[u8], off: usize) -> Result<u16, ParseError> {
    let end = off.checked_add(2).ok_or(ParseError::Corrupt("u16 off"))?;
    let b = d.get(off..end).ok_or(ParseError::Corrupt("u16 read"))?;
    Ok(u16::from_le_bytes(b.try_into().unwrap()))
}
