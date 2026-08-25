//! LNK file header (fixed 0x4C bytes).

use crate::{read_u32, read_u64, ParseError};

/// Link target / content flags (`LinkFlags` in MS-SHLLINK). Values and
/// identifier spellings ported from LECmd's `Header.DataFlag` enum.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFlag {
    HasTargetIdList = 0x0000_0001,
    HasLinkInfo = 0x0000_0002,
    HasName = 0x0000_0004,
    HasRelativePath = 0x0000_0008,
    HasWorkingDir = 0x0000_0010,
    HasArguments = 0x0000_0020,
    HasIconLocation = 0x0000_0040,
    IsUnicode = 0x0000_0080,
    ForceNoLinkInfo = 0x0000_0100,
    HasExpString = 0x0000_0200,
    RunInSeparateProcess = 0x0000_0400,
    Reserved0 = 0x0000_0800,
    HasDarwinId = 0x0000_1000,
    RunAsUser = 0x0000_2000,
    HasExpIcon = 0x0000_4000,
    NoPidlAlias = 0x0000_8000,
    Reserved1 = 0x0001_0000,
    RunWithShimLayer = 0x0002_0000,
    ForceNoLinkTrack = 0x0004_0000,
    EnableTargetMetadata = 0x0008_0000,
    DisableLinkPathTracking = 0x0010_0000,
    DisableKnownFolderTracking = 0x0020_0000,
    DisableKnownFolderAlias = 0x0040_0000,
    AllowLinkToLink = 0x0080_0000,
    UnaliasOnSave = 0x0100_0000,
    PreferEnvironmentPath = 0x0200_0000,
    KeepLocalIdListForUncTarget = 0x0400_0000,
}

/// Every `DataFlag` in ascending bit order, paired with its LECmd identifier.
/// `Reserved0` / `Reserved1` are intentionally excluded so they are never
/// rendered by [`Header::header_flags_string`].
const DATA_FLAG_TABLE: &[(u32, &str)] = &[
    (DataFlag::HasTargetIdList as u32, "HasTargetIdList"),
    (DataFlag::HasLinkInfo as u32, "HasLinkInfo"),
    (DataFlag::HasName as u32, "HasName"),
    (DataFlag::HasRelativePath as u32, "HasRelativePath"),
    (DataFlag::HasWorkingDir as u32, "HasWorkingDir"),
    (DataFlag::HasArguments as u32, "HasArguments"),
    (DataFlag::HasIconLocation as u32, "HasIconLocation"),
    (DataFlag::IsUnicode as u32, "IsUnicode"),
    (DataFlag::ForceNoLinkInfo as u32, "ForceNoLinkInfo"),
    (DataFlag::HasExpString as u32, "HasExpString"),
    (
        DataFlag::RunInSeparateProcess as u32,
        "RunInSeparateProcess",
    ),
    (DataFlag::HasDarwinId as u32, "HasDarwinId"),
    (DataFlag::RunAsUser as u32, "RunAsUser"),
    (DataFlag::HasExpIcon as u32, "HasExpIcon"),
    (DataFlag::NoPidlAlias as u32, "NoPidlAlias"),
    (DataFlag::RunWithShimLayer as u32, "RunWithShimLayer"),
    (DataFlag::ForceNoLinkTrack as u32, "ForceNoLinkTrack"),
    (
        DataFlag::EnableTargetMetadata as u32,
        "EnableTargetMetadata",
    ),
    (
        DataFlag::DisableLinkPathTracking as u32,
        "DisableLinkPathTracking",
    ),
    (
        DataFlag::DisableKnownFolderTracking as u32,
        "DisableKnownFolderTracking",
    ),
    (
        DataFlag::DisableKnownFolderAlias as u32,
        "DisableKnownFolderAlias",
    ),
    (DataFlag::AllowLinkToLink as u32, "AllowLinkToLink"),
    (DataFlag::UnaliasOnSave as u32, "UnaliasOnSave"),
    (
        DataFlag::PreferEnvironmentPath as u32,
        "PreferEnvironmentPath",
    ),
    (
        DataFlag::KeepLocalIdListForUncTarget as u32,
        "KeepLocalIdListForUncTarget",
    ),
];

/// Windows file attributes. Values and identifier spellings ported from
/// LECmd's `Header.FileAttribute` enum.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAttribute {
    FileAttributeReadonly = 0x0000_0001,
    FileAttributeHidden = 0x0000_0002,
    FileAttributeSystem = 0x0000_0004,
    ResVolumeLabel = 0x0000_0008,
    FileAttributeDirectory = 0x0000_0010,
    FileAttributeArchive = 0x0000_0020,
    FileAttributeDevice = 0x0000_0040,
    FileAttributeNormal = 0x0000_0080,
    FileAttributeTemporary = 0x0000_0100,
    FileAttributeSparseFile = 0x0000_0200,
    FileAttributeReparsePoint = 0x0000_0400,
    FileAttributeCompressed = 0x0000_0800,
    FileAttributeOffline = 0x0000_1000,
    FileAttributeNotContentIndexed = 0x0000_2000,
    FileAttributeEncrypted = 0x0000_4000,
    UnkWin95Fat = 0x0000_8000,
    FileAttributeVirtual = 0x0001_0000,
}

/// Every `FileAttribute` in ascending bit order, paired with its LECmd
/// identifier, in the order C# `[Flags] Enum.ToString()` would emit them.
const FILE_ATTRIBUTE_TABLE: &[(u32, &str)] = &[
    (
        FileAttribute::FileAttributeReadonly as u32,
        "FileAttributeReadonly",
    ),
    (
        FileAttribute::FileAttributeHidden as u32,
        "FileAttributeHidden",
    ),
    (
        FileAttribute::FileAttributeSystem as u32,
        "FileAttributeSystem",
    ),
    (FileAttribute::ResVolumeLabel as u32, "ResVolumeLabel"),
    (
        FileAttribute::FileAttributeDirectory as u32,
        "FileAttributeDirectory",
    ),
    (
        FileAttribute::FileAttributeArchive as u32,
        "FileAttributeArchive",
    ),
    (
        FileAttribute::FileAttributeDevice as u32,
        "FileAttributeDevice",
    ),
    (
        FileAttribute::FileAttributeNormal as u32,
        "FileAttributeNormal",
    ),
    (
        FileAttribute::FileAttributeTemporary as u32,
        "FileAttributeTemporary",
    ),
    (
        FileAttribute::FileAttributeSparseFile as u32,
        "FileAttributeSparseFile",
    ),
    (
        FileAttribute::FileAttributeReparsePoint as u32,
        "FileAttributeReparsePoint",
    ),
    (
        FileAttribute::FileAttributeCompressed as u32,
        "FileAttributeCompressed",
    ),
    (
        FileAttribute::FileAttributeOffline as u32,
        "FileAttributeOffline",
    ),
    (
        FileAttribute::FileAttributeNotContentIndexed as u32,
        "FileAttributeNotContentIndexed",
    ),
    (
        FileAttribute::FileAttributeEncrypted as u32,
        "FileAttributeEncrypted",
    ),
    (FileAttribute::UnkWin95Fat as u32, "UnkWin95Fat"),
    (
        FileAttribute::FileAttributeVirtual as u32,
        "FileAttributeVirtual",
    ),
];

/// The shell link class id, little-endian byte order as stored on disk:
/// `00021401-0000-0000-C000-000000000046`.
const CLASS_ID: [u8; 16] = [
    0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

/// The fixed 0x4C-byte LNK header (MS-SHLLINK ShellLinkHeader).
#[derive(Debug, Clone)]
pub struct Header {
    pub data_flags: u32,
    pub file_attributes: u32,
    pub target_creation: u64,
    pub target_access: u64,
    pub target_write: u64,
    pub file_size: u32,
}

impl Header {
    /// Size of the fixed LNK header in bytes.
    pub const SIZE: usize = 0x4C;

    /// Parse the fixed LNK header. Validates the `0x4C` signature and the
    /// 16-byte shell link class id before reading fields.
    pub fn parse(d: &[u8]) -> Result<Header, ParseError> {
        if d.len() < Self::SIZE {
            return Err(ParseError::BadSignature);
        }
        if read_u32(d, 0)? != 0x0000_004C {
            return Err(ParseError::BadSignature);
        }
        if d[4..20] != CLASS_ID {
            return Err(ParseError::BadSignature);
        }
        Ok(Header {
            data_flags: read_u32(d, 0x14)?,
            file_attributes: read_u32(d, 0x18)?,
            target_creation: read_u64(d, 0x1C)?,
            target_access: read_u64(d, 0x24)?,
            target_write: read_u64(d, 0x2C)?,
            file_size: read_u32(d, 0x34)?,
        })
    }

    /// True if `flag` is set in the link's data flags.
    pub fn has(&self, flag: DataFlag) -> bool {
        let bit = flag as u32;
        self.data_flags & bit == bit
    }

    /// Render the set data flags the way C# `[Flags] Enum.ToString()` does:
    /// identifier names in ascending bit order, joined by `", "`. The
    /// `Reserved0` / `Reserved1` bits are never emitted.
    pub fn header_flags_string(&self) -> String {
        Self::join_set_flags(self.data_flags, DATA_FLAG_TABLE)
    }

    /// Render the set file attributes the way C# `[Flags] Enum.ToString()`
    /// does: identifier names in ascending bit order, joined by `", "`.
    /// When no attributes are set (value == 0), C# emits the literal `"0"`
    /// (because no zero-named member exists in the enum), so we do the same.
    pub fn file_attributes_string(&self) -> String {
        if self.file_attributes == 0 {
            return "0".to_string();
        }
        Self::join_set_flags(self.file_attributes, FILE_ATTRIBUTE_TABLE)
    }

    fn join_set_flags(value: u32, table: &[(u32, &str)]) -> String {
        table
            .iter()
            .filter(|(bit, _)| value & bit == *bit)
            .map(|(_, name)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file is a true shell link only if it actually begins with the LNK
    /// signature (`0x4C`). Some `.lnk`-extension files in the captures are
    /// Recycle Bin `$I` metadata records for deleted `.lnk` targets — they
    /// carry the extension but are not shell links and must be excluded.
    fn is_shell_link_bytes(b: &[u8]) -> bool {
        b.len() >= 4 && b[0..4] == [0x4C, 0x00, 0x00, 0x00]
    }

    /// Return a representative real LNK that carries a target IDList or link
    /// info (the common case). A handful of real LNKs in the captures target
    /// an environment-variable path (HasExpString) and set neither bit, so we
    /// skip those here to keep the smoke test about a typical link.
    fn a_real_lnk() -> Option<Vec<u8>> {
        let root = std::path::Path::new("../../test captures");
        if !root.exists() {
            return None;
        }
        for p in tests_support::all_lnks(root) {
            if let Ok(b) = std::fs::read(&p) {
                if !is_shell_link_bytes(&b) {
                    continue;
                }
                if let Ok(h) = Header::parse(&b) {
                    if h.has(DataFlag::HasTargetIdList) || h.has(DataFlag::HasLinkInfo) {
                        return Some(b);
                    }
                }
            }
        }
        None
    }

    #[test]
    fn parses_real_lnk_header() {
        let Some(raw) = a_real_lnk() else {
            eprintln!("captures absent; skipping");
            return;
        };
        let h = Header::parse(&raw).unwrap();
        assert!(h.has(DataFlag::HasTargetIdList) || h.has(DataFlag::HasLinkInfo));
        assert!(raw.len() > 0x4C);
    }

    #[test]
    fn rejects_non_lnk() {
        let big = vec![0x00u8; 128];
        assert!(matches!(Header::parse(&big), Err(ParseError::BadSignature)));
        assert!(Header::parse(b"short").is_err());
    }

    #[test]
    fn parses_every_lnk_header_in_captures() {
        let root = std::path::Path::new("../../test captures");
        if !root.exists() {
            return;
        }
        let mut ok = 0u32;
        let mut fail = vec![];
        let mut decoys = 0u32;
        for p in tests_support::all_lnks(root) {
            let raw = std::fs::read(&p).unwrap();
            if !is_shell_link_bytes(&raw) {
                // Not a shell link (e.g. Recycle Bin $I metadata with a .lnk
                // extension). Must be rejected, not parsed.
                assert!(
                    matches!(Header::parse(&raw), Err(ParseError::BadSignature)),
                    "non-LNK should be rejected: {}",
                    p.display()
                );
                decoys += 1;
                continue;
            }
            match Header::parse(&raw) {
                Ok(_) => ok += 1,
                Err(e) => fail.push(format!("{}: {e}", p.display())),
            }
        }
        assert!(
            fail.is_empty(),
            "{ok} ok; {decoys} decoys; failures: {fail:#?}"
        );
        assert!(ok > 300, "expected 300+ lnk headers, got {ok}");
    }

    #[test]
    fn renders_header_flags_and_attributes() {
        let Some(raw) = a_real_lnk() else {
            return;
        };
        let h = Header::parse(&raw).unwrap();
        // a real LNK with target ids should render that flag name in the string
        let flags = h.header_flags_string();
        assert!(!flags.is_empty());
        // attributes string is comma-joined identifiers or "0" when zero
        let _ = h.file_attributes_string();
    }

    /// LECmd: `FileAttributes = lnk.Header.FileAttributes.ToString()`.
    /// C# `[Flags] Enum.ToString()` on a zero value (with no zero-named member)
    /// returns the literal `"0"`. Verify that our implementation matches.
    #[test]
    fn file_attributes_zero_returns_literal_zero() {
        let mut raw = vec![0u8; Header::SIZE];
        // Write the LNK signature (header size 0x4C) at offset 0.
        raw[0] = 0x4C;
        raw[1] = 0x00;
        raw[2] = 0x00;
        raw[3] = 0x00;
        // Write the shell link class id at offset 4.
        let class_id: [u8; 16] = [
            0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x46,
        ];
        raw[4..20].copy_from_slice(&class_id);
        // Leave file_attributes at zero (offset 0x18).
        let h = Header::parse(&raw).unwrap();
        assert_eq!(h.file_attributes, 0);
        assert_eq!(
            h.file_attributes_string(),
            "0",
            "zero FileAttributes must render as \"0\" for LECmd parity"
        );
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use std::path::{Path, PathBuf};
    pub fn all_lnks(root: &Path) -> Vec<PathBuf> {
        let mut out = vec![];
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
                        out.push(p);
                    }
                }
            }
        }
        out
    }
}
