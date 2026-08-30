//! SCCA prefetch format parsing (versions 17/23/26/30/31).
//!
//! Layouts ported from Eric Zimmerman's C# Prefetch library
//! (Prefetch-master/Prefetch): `Other/Header.cs`, `Versions/Version17.cs`,
//! `Versions/Version23.cs`, `Versions/Version26.cs`,
//! `Versions/Version30or31.cs`, and `Other/VolumeInfo.cs` consumers.
//! Every untrusted offset/length is bounds-checked; no read can panic.

/// A parsed prefetch file (decompressed SCCA payload).
pub struct PrefetchFile {
    /// Format version: 17 | 23 | 26 | 30 | 31.
    pub version: u32,
    /// Executable name from the header (UTF-16, NUL-trimmed).
    pub executable_name: String,
    /// Header hash; matches the hex suffix of the .pf filename.
    pub hash: u32,
    /// Header field at 0x0C: the decompressed pf file's own size.
    pub file_size: u32,
    /// Total times the executable has run.
    pub run_count: u32,
    /// Last-run FILETIMEs, most recent first; zero entries excluded.
    pub run_times: Vec<u64>,
    /// Volumes referenced by this prefetch file.
    pub volumes: Vec<VolumeInfo>,
    /// Filename strings section: every file loaded at launch.
    pub files_loaded: Vec<String>,
}

/// One volume entry from the volumes-information section.
pub struct VolumeInfo {
    /// Device name, e.g. `\VOLUME{guid}`.
    pub device_name: String,
    /// Volume serial number.
    pub serial_number: u32,
    /// Volume creation FILETIME.
    pub creation_time: u64,
    /// This volume's directory strings.
    pub directories: Vec<String>,
}

/// Errors from prefetch parsing.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("not a prefetch file (bad signature)")]
    BadSignature,
    #[error("unsupported prefetch version {0}")]
    UnsupportedVersion(u32),
    #[error("truncated or corrupt structure: {0}")]
    Corrupt(&'static str),
    #[error(transparent)]
    Decompress(#[from] crate::decompress::DecompressError),
}

// ---------------------------------------------------------------------------
// Bounds-checked primitive reads (untrusted input; never panic, never wrap)
// ---------------------------------------------------------------------------

fn bytes_at<'a>(
    data: &'a [u8],
    off: usize,
    len: usize,
    what: &'static str,
) -> Result<&'a [u8], ParseError> {
    let end = off.checked_add(len).ok_or(ParseError::Corrupt(what))?;
    data.get(off..end).ok_or(ParseError::Corrupt(what))
}

fn u16_at(data: &[u8], off: usize, what: &'static str) -> Result<u16, ParseError> {
    let b = bytes_at(data, off, 2, what)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], off: usize, what: &'static str) -> Result<u32, ParseError> {
    let b = bytes_at(data, off, 4, what)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(data: &[u8], off: usize, what: &'static str) -> Result<u64, ParseError> {
    let b = bytes_at(data, off, 8, what)?;
    Ok(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

/// Decode UTF-16LE bytes (lossy, like C# Encoding.Unicode.GetString).
fn utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

// ---------------------------------------------------------------------------
// Per-version file-information block layout
// (offsets are relative to the file-info block, which starts at byte 84 —
//  every VersionNN.cs does `Buffer.BlockCopy(rawBytes, 84, fileInfoBytes, ...)`)
// ---------------------------------------------------------------------------

struct VersionLayout {
    /// Total file-info block size (bytes copied from offset 84 in the C#).
    file_info_size: usize,
    /// Offset of the run-time FILETIME array within the file-info block.
    run_times_offset: usize,
    /// Number of run-time slots (1 for v17/23, 8 for v26/30/31).
    run_time_slots: usize,
    /// Offset of the run-count u32 within the file-info block.
    /// For v30/31 this is the *default*; see `run_count_v30_shift`.
    run_count_offset: usize,
    /// Version30or31.cs run-count variant branch: newer Win10/11 builds shift
    /// the counter back 8 bytes (sentinel u32 at run_count_offset - 4).
    run_count_v30_shift: bool,
    /// Size of one volume entry in the volumes-information section.
    volume_entry_size: usize,
}

fn layout_for(version: u32) -> Result<VersionLayout, ParseError> {
    Ok(match version {
        // Version17.cs: fileInfoBytes = 68 bytes @84; rawTime i64 @36 (one
        // slot); RunCount @60; volBytes = 40.
        17 => VersionLayout {
            file_info_size: 68,
            run_times_offset: 36,
            run_time_slots: 1,
            run_count_offset: 60,
            run_count_v30_shift: false,
            volume_entry_size: 40,
        },
        // Version23.cs: fileInfoBytes = 156 bytes @84; TotalDirectoryCount
        // @36; rawTime i64 @44 (one slot); RunCount @68; volBytes = 104.
        23 => VersionLayout {
            file_info_size: 156,
            run_times_offset: 44,
            run_time_slots: 1,
            run_count_offset: 68,
            run_count_v30_shift: false,
            volume_entry_size: 104,
        },
        // Version26.cs: fileInfoBytes = 224 bytes @84; runtimeBytes =
        // Skip(44).Take(64) → 8 slots @44; RunCount @124; volBytes = 104.
        26 => VersionLayout {
            file_info_size: 224,
            run_times_offset: 44,
            run_time_slots: 8,
            run_count_offset: 124,
            run_count_v30_shift: false,
            volume_entry_size: 104,
        },
        // Version30or31.cs: fileInfoBytes = 224 bytes @84; runtimeBytes =
        // 64 bytes @44 → 8 slots; RunCount @124 with the variant branch
        // (runcountPre @120; if nonzero, RunCount @116); volBytes = 96.
        30 | 31 => VersionLayout {
            file_info_size: 224,
            run_times_offset: 44,
            run_time_slots: 8,
            run_count_offset: 124,
            run_count_v30_shift: true,
            volume_entry_size: 96,
        },
        other => return Err(ParseError::UnsupportedVersion(other)),
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse raw (already-decompressed) SCCA bytes.
pub fn parse(data: &[u8]) -> Result<PrefetchFile, ParseError> {
    // --- Header (Other/Header.cs; 84 bytes, copied in every VersionNN.cs) ---
    if data.len() < 8 {
        return Err(ParseError::BadSignature);
    }
    // Header.cs: signature ASCII "SCCA" at offset 4
    // (PrefetchFile.cs: Signature = 0x41434353).
    if &data[4..8] != b"SCCA" {
        return Err(ParseError::BadSignature);
    }
    // Header.cs: version i32 at offset 0; PrefetchFile.cs dispatches on
    // IPrefetch.cs enum: 17, 23, 26, 30, 31.
    let version = u32_at(data, 0, "header version")?;
    let layout = layout_for(version)?;

    // Header.cs: index 8 = 4 unknown bytes; FileSize i32 at offset 12.
    let file_size = u32_at(data, 12, "header file size")?;

    // Header.cs: executable name = 60 bytes of UTF-16 at offset 16,
    // truncated at the first NUL, then trimmed.
    let name_bytes = bytes_at(data, 16, 60, "header executable name")?;
    let name_full = utf16le(name_bytes);
    let executable_name = name_full
        .split('\0')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    // Header.cs: Hash i32 at offset 76 (16 + 60).
    let hash = u32_at(data, 76, "header hash")?;

    // --- File-information block at offset 84 (VersionNN.cs) ---
    let fi = 84usize;
    // Common section table, identical in all four version classes:
    // fileInfoBytes offsets 16/20 = filename strings, 24/28/32 = volumes.
    // (FileMetrics @0/4 and TraceChains @8/12 are not needed here.)
    let what = "file information block";
    bytes_at(data, fi, layout.file_info_size, what)?;
    let filename_strings_offset = u32_at(data, fi + 16, what)? as usize;
    let filename_strings_size = u32_at(data, fi + 20, what)? as usize;
    let volumes_info_offset = u32_at(data, fi + 24, what)? as usize;
    let volume_count = u32_at(data, fi + 28, what)? as usize;
    let volumes_info_size = u32_at(data, fi + 32, what)? as usize;

    // Run times: v17 has one i64 @36, v23 one @44, v26/30/31 read 8 slots
    // @44 and keep only rawTime > 0 (zeros excluded per VersionNN.cs).
    let mut run_times = Vec::new();
    for i in 0..layout.run_time_slots {
        let off = fi + layout.run_times_offset + i * 8;
        let t = u64_at(data, off, "run time array")?;
        if t > 0 {
            run_times.push(t);
        }
    }

    // Run count. Version30or31.cs variant branch: runcountPre is the u32 at
    // (124 - 4); when it is nonzero the build is a newer Win10/11 layout and
    // the real counter sits at (124 - 8).
    let mut run_count = u32_at(data, fi + layout.run_count_offset, "run count")?;
    if layout.run_count_v30_shift {
        let pre = u32_at(data, fi + layout.run_count_offset - 4, "run count")?;
        if pre != 0 {
            run_count = u32_at(data, fi + layout.run_count_offset - 8, "run count")?;
        }
    }

    // --- Filename strings section (VersionNN.cs: FilenameStringsOffset /
    // FilenameStringsSize; UTF-16, split on NUL, empties removed) ---
    let fns = bytes_at(
        data,
        filename_strings_offset,
        filename_strings_size,
        "filename strings section",
    )?;
    let files_loaded: Vec<String> = utf16le(fns)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    // --- Volumes-information section (VersionNN.cs volume loop) ---
    // The C# copies VolumesInfoSize bytes and slices entries out of it, so
    // all entries must fit inside the declared section.
    let vol_section = bytes_at(
        data,
        volumes_info_offset,
        volumes_info_size,
        "volumes information section",
    )?;
    let mut volumes = Vec::new();
    // explicit cap: iteration count is implied by the section size; make it auditable
    let volume_count = volume_count.min(volumes_info_size / layout.volume_entry_size.max(1));
    for j in 0..volume_count {
        let base = j
            .checked_mul(layout.volume_entry_size)
            .ok_or(ParseError::Corrupt("volume entry offset overflow"))?;
        // Volume entry layout (identical fields in all versions; only the
        // entry stride differs): volDevOffset i32 @0, volDevNumChar i32 @4,
        // creation i64 @8, serial i32 @16, fileRef @20/24 (skipped),
        // dirStringsOffset i32 @28, numDirectoryStrings i32 @32.
        let what = "volume entry";
        let dev_name_offset = u32_at(vol_section, base, what)? as usize;
        let dev_name_chars = u32_at(vol_section, base + 4, what)? as usize;
        let creation_time = u64_at(vol_section, base + 8, what)?;
        let serial_number = u32_at(vol_section, base + 16, what)?;
        let dir_strings_offset = u32_at(vol_section, base + 28, what)? as usize;
        let num_directory_strings = u32_at(vol_section, base + 32, what)? as usize;

        // Device name: UTF-16 at VolumesInfoOffset + volDevOffset,
        // volDevNumChar * 2 bytes (VersionNN.cs devNameBytes).
        let dev_name_len = dev_name_chars
            .checked_mul(2)
            .ok_or(ParseError::Corrupt("volume device name length overflow"))?;
        let abs_dev_off = volumes_info_offset
            .checked_add(dev_name_offset)
            .ok_or(ParseError::Corrupt("volume device name offset overflow"))?;
        let dev_bytes = bytes_at(data, abs_dev_off, dev_name_len, "volume device name")?;
        let device_name = utf16le(dev_bytes).trim_end_matches('\0').to_string();

        // Directory strings: at VolumesInfoOffset + dirStringsOffset; each is
        // an i16 char count followed by (count*2 + 2) bytes of UTF-16
        // (the count is doubled for Unicode plus 2 for the NUL terminator),
        // NULs trimmed (VersionNN.cs directory-strings loop).
        let mut idx = volumes_info_offset
            .checked_add(dir_strings_offset)
            .ok_or(ParseError::Corrupt("directory strings offset overflow"))?;
        // explicit cap: each directory string costs ≥4 bytes (2-byte length prefix +
        // at minimum a 2-byte NUL terminator when char_count == 0); cap by remaining buffer.
        let num_directory_strings = num_directory_strings.min((data.len().saturating_sub(idx)) / 4);
        let mut directories = Vec::with_capacity(num_directory_strings.min(1024));
        for _ in 0..num_directory_strings {
            let char_count = u16_at(data, idx, "directory string length")? as usize;
            let byte_len = char_count
                .checked_mul(2)
                .and_then(|n| n.checked_add(2))
                .ok_or(ParseError::Corrupt("directory string length overflow"))?; // chars are UTF-16 + NUL pair
            let str_off = idx
                .checked_add(2)
                .ok_or(ParseError::Corrupt("directory string offset overflow"))?;
            let s = bytes_at(data, str_off, byte_len, "directory string")?;
            directories.push(utf16le(s).trim_end_matches('\0').to_string());
            idx = str_off
                .checked_add(byte_len)
                .ok_or(ParseError::Corrupt("directory string offset overflow"))?;
        }

        volumes.push(VolumeInfo {
            device_name,
            serial_number,
            creation_time,
            directories,
        });
    }

    Ok(PrefetchFile {
        version,
        executable_name,
        hash,
        file_size,
        run_count,
        run_times,
        volumes,
        files_loaded,
    })
}

/// Convenience: sniff MAM vs SCCA, decompress if needed, then parse.
pub fn parse_file_bytes(data: &[u8]) -> Result<PrefetchFile, ParseError> {
    if data.len() >= 8 && &data[..4] == b"MAM\x04" {
        let scca = crate::decompress::decompress_mam(data)?;
        return parse(&scca);
    }
    if data.len() >= 8 && &data[4..8] == b"SCCA" {
        return parse(data);
    }
    Err(ParseError::BadSignature)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locate a prefetch sample under any local capture. Discovering it by
    /// name keeps the test independent of which captures are present, and
    /// keeps capture host names out of the source tree.
    fn find_prefetch(name_prefix: &str) -> Option<std::path::PathBuf> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
        let mut stack = vec![root];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(name_prefix) && n.ends_with(".pf"))
                {
                    return Some(p);
                }
            }
        }
        None
    }

    #[test]
    fn parses_real_win11_prefetch() {
        let Some(path) = find_prefetch("MSEDGE.EXE-") else {
            eprintln!("captures absent; skipping");
            return;
        };
        let raw = std::fs::read(path).unwrap();
        let pf = parse_file_bytes(&raw).unwrap();
        assert_eq!(pf.executable_name, "MSEDGE.EXE");
        assert_eq!(pf.hash, 0x37D25F9E);
        assert!(matches!(pf.version, 30 | 31));
        assert!(pf.run_count >= 1);
        assert!(!pf.run_times.is_empty() && pf.run_times.len() <= 8);
        assert!(!pf.volumes.is_empty());
        assert!(!pf.volumes[0].directories.is_empty());
        assert!(pf.volumes[0].device_name.starts_with("\\VOLUME{"));
        assert!(!pf.files_loaded.is_empty());
        assert!(pf.files_loaded.iter().all(|f| f.starts_with('\\')));
        // file_size header field equals the decompressed byte count
        let declared = u32::from_le_bytes(raw[4..8].try_into().unwrap());
        assert_eq!(pf.file_size, declared);
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        assert!(parse(b"garbage").is_err());
        let Some(path) = find_prefetch("MSEDGE.EXE-") else {
            return;
        };
        let Ok(raw) = std::fs::read(path) else {
            return;
        };
        let scca = crate::decompress::decompress_mam(&raw).unwrap();
        for cut in [8usize, 84, 200, scca.len() / 2] {
            let _ = parse(&scca[..cut]); // Err, never panic
        }
    }

    #[test]
    fn parses_every_pf_in_captures() {
        let root = std::path::Path::new("../../test captures");
        if !root.exists() {
            return;
        }
        let mut ok = 0u32;
        let mut failures = vec![];
        for p in crate::decompress::walk_pf(root) {
            let raw = std::fs::read(&p).unwrap();
            match parse_file_bytes(&raw) {
                Ok(pf) => {
                    // filename embeds the hash: NAME-XXXXXXXX.pf
                    let stem = p.file_stem().unwrap().to_string_lossy().to_string();
                    let suffix = stem.rsplit('-').next().unwrap().to_string();
                    assert_eq!(
                        format!("{:08X}", pf.hash),
                        suffix.to_uppercase(),
                        "hash mismatch for {}",
                        p.display()
                    );
                    ok += 1;
                }
                Err(e) => failures.push(format!("{}: {e}", p.display())),
            }
        }
        assert!(failures.is_empty(), "{ok} ok; failures: {failures:#?}");
        assert!(ok > 500);
    }
}
