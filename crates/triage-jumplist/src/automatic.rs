//! Automatic Destinations parser.
//!
//! An `*.automaticDestinations-ms` file is an OLE compound file (read via the
//! `cfb` crate) holding a `DestList` stream plus one numbered stream per entry,
//! each containing a single embedded LNK. The numbered stream's name is the
//! entry number rendered as a lowercase hex string with no `0x` prefix
//! (entry 1 -> "1", entry 10 -> "a", entry 26 -> "1a"), matching C#
//! `EntryNumber.ToString("X")` compared case-insensitively (`AutomaticDestination.cs`).
//!
//! Each [`DestListEntry`] is paired with its parsed LNK to form an
//! [`AutoEntry`]. With `with_dir`, numbered streams not referenced by the
//! `DestList` are surfaced as orphan rows with a synthetic dest.

use std::io::{Cursor, Read};

use crate::destlist::{DestList, DestListEntry};
use crate::ParseError;

/// One automatic-destination row: a `DestList` entry plus its embedded LNK.
pub struct AutoEntry {
    pub dest: DestListEntry,
    /// `None` if the numbered stream is missing or the LNK fails to parse.
    pub lnk: Option<triage_lnk::LnkFile>,
}

/// A fully parsed Automatic Destinations file.
pub struct AutomaticDestination {
    pub dest_list_version: u32,
    pub last_used_entry_number: u32,
    pub entries: Vec<AutoEntry>,
    /// All root stream names present in the compound file (for `--withDir`).
    pub stream_names: Vec<String>,
}

/// Stream name for an entry number: lowercase hex, no prefix. Mirrors C#
/// `EntryNumber.ToString("X")` matched case-insensitively.
fn entry_stream_name(entry_number: u32) -> String {
    format!("{entry_number:x}")
}

/// Parse an `automaticDestinations-ms` file: open the compound file, parse the
/// `DestList`, and for each entry parse its numbered LNK stream. With
/// `with_dir`, also surfaces numbered streams not referenced by the `DestList`
/// as orphan [`AutoEntry`] rows (synthetic dest carrying the entry number).
pub fn parse(
    data: &[u8],
    codepage: u16,
    with_dir: bool,
) -> Result<AutomaticDestination, ParseError> {
    let mut comp = cfb::CompoundFile::open(Cursor::new(data))
        .map_err(|e| ParseError::Compound(e.to_string()))?;

    // Collect root stream names (strip the leading '/').
    let mut stream_names: Vec<String> = comp
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| e.name().to_string())
        .collect();
    stream_names.sort();
    stream_names.dedup();

    // Find the DestList stream case-insensitively. A missing DestList stream
    // OR a zero-byte one is a valid *empty* jump list (matches Zimmerman's
    // `AutomaticDestination.cs`, which only parses when DirectorySize > 0).
    // In that case there are simply no DestList-driven entries; with_dir can
    // still surface any numbered LNK streams.
    let dest_bytes = match stream_names
        .iter()
        .find(|n| n.eq_ignore_ascii_case("destlist"))
    {
        Some(name) => read_stream(&mut comp, &name.clone())?,
        None => Vec::new(),
    };

    let dest_list = if dest_bytes.is_empty() {
        None
    } else {
        Some(DestList::parse(&dest_bytes)?)
    };

    let (dest_list_version, last_used_entry_number, dest_entries) = match dest_list {
        Some(dl) => (dl.header.version, dl.header.last_entry_number, dl.entries),
        None => (0, 0, Vec::new()),
    };

    // Map stream names (lowercased) for case-insensitive numbered lookups.
    let lower_names: Vec<String> = stream_names.iter().map(|n| n.to_lowercase()).collect();

    let mut entries = Vec::with_capacity(dest_entries.len());
    let mut covered: Vec<String> = Vec::new();

    for dest in dest_entries {
        let want = entry_stream_name(dest.entry_number);
        let lnk = match lower_names.iter().position(|n| *n == want) {
            Some(idx) => {
                let actual = &stream_names[idx];
                covered.push(want.clone());
                read_stream(&mut comp, actual)
                    .ok()
                    .and_then(|b| triage_lnk::parse_with_codepage(&b, codepage).ok())
            }
            None => None,
        };
        entries.push(AutoEntry { dest, lnk });
    }

    if with_dir {
        // Surface numbered streams not already covered by a DestList entry.
        for name in &stream_names {
            if name.eq_ignore_ascii_case("destlist") {
                continue;
            }
            // Only consider names that are pure lowercase-hex entry numbers.
            let lower = name.to_lowercase();
            let Ok(entry_number) = u32::from_str_radix(&lower, 16) else {
                continue;
            };
            if covered.contains(&lower) {
                continue;
            }
            let lnk = read_stream(&mut comp, name)
                .ok()
                .and_then(|b| triage_lnk::parse_with_codepage(&b, codepage).ok());
            let dest = DestListEntry {
                entry_number,
                ..Default::default()
            };
            entries.push(AutoEntry { dest, lnk });
        }
    }

    Ok(AutomaticDestination {
        dest_list_version,
        last_used_entry_number,
        entries,
        stream_names,
    })
}

/// Read a root stream fully by name. `cfb` stream paths are `/Name`.
fn read_stream<F: Read + std::io::Seek>(
    comp: &mut cfb::CompoundFile<F>,
    name: &str,
) -> Result<Vec<u8>, ParseError> {
    let path = format!("/{name}");
    let mut stream = comp
        .open_stream(&path)
        .map_err(|e| ParseError::Compound(e.to_string()))?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .map_err(|e| ParseError::Compound(e.to_string()))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_stream_name_hex_no_prefix() {
        assert_eq!(entry_stream_name(1), "1");
        assert_eq!(entry_stream_name(10), "a");
        assert_eq!(entry_stream_name(26), "1a");
        assert_eq!(entry_stream_name(255), "ff");
    }
}
