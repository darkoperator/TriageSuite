use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{MftriageError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BootRecord {
    pub entry_point: String,
    pub signature: String,
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub cluster_size: u32,
    pub reserved_sectors: u16,
    pub total_sectors: u64,
    pub mft_cluster_block_number: u64,
    pub mft_mirr_cluster_block_number: u64,
    pub mft_entry_size: i32,
    pub index_entry_size: i32,
    pub volume_serial_number_raw: String,
    pub volume_serial_number: String,
    pub volume_serial_number32: String,
    pub volume_serial_number32_reverse: String,
    pub sector_signature: String,
    pub source_file: PathBuf,
}

pub fn parse_boot(path: impl AsRef<Path>) -> Result<BootRecord> {
    let path = path.as_ref();
    let bytes = fs::read(path)?;
    if bytes.len() < 512 {
        return Err(MftriageError::MalformedArtifact {
            path: path.to_path_buf(),
            message: "boot sector is shorter than 512 bytes".to_string(),
        });
    }

    let entry_point = format_bytes(&bytes[0..3]);
    let signature = String::from_utf8_lossy(&bytes[3..11]).to_string();
    if signature != "NTFS    " {
        return Err(MftriageError::MalformedArtifact {
            path: path.to_path_buf(),
            message: format!("invalid NTFS boot signature: {signature:?}"),
        });
    }

    let sector_signature = u16::from_le_bytes([bytes[510], bytes[511]]);
    if sector_signature != 0xAA55 {
        return Err(MftriageError::MalformedArtifact {
            path: path.to_path_buf(),
            message: format!("invalid boot sector signature: 0x{sector_signature:04X}"),
        });
    }

    let bytes_per_sector = u16::from_le_bytes([bytes[11], bytes[12]]);
    let sectors_per_cluster = bytes[13];
    let cluster_size = bytes_per_sector as u32 * sectors_per_cluster as u32;
    let reserved_sectors = u16::from_le_bytes([bytes[14], bytes[15]]);
    let total_sectors = read_u64(&bytes, 40);
    let mft_cluster_block_number = read_u64(&bytes, 48);
    let mft_mirr_cluster_block_number = read_u64(&bytes, 56);
    let mft_entry_size = decode_record_size(bytes[64], cluster_size);
    let index_entry_size = decode_record_size(bytes[68], cluster_size);
    let serial_raw = read_u64(&bytes, 72);

    Ok(BootRecord {
        entry_point,
        signature,
        bytes_per_sector,
        sectors_per_cluster,
        cluster_size,
        reserved_sectors,
        total_sectors,
        mft_cluster_block_number,
        mft_mirr_cluster_block_number,
        mft_entry_size,
        index_entry_size,
        volume_serial_number_raw: format!("0x{serial_raw:X}"),
        volume_serial_number: format_volume_serial(serial_raw, false, false),
        volume_serial_number32: format_volume_serial(serial_raw, true, false),
        volume_serial_number32_reverse: format_volume_serial(serial_raw, true, true),
        sector_signature: format!("0x{sector_signature:04X}"),
        source_file: path.to_path_buf(),
    })
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("slice length checked"),
    )
}

fn decode_record_size(raw: u8, cluster_size: u32) -> i32 {
    let signed = raw as i8;
    if signed < 0 {
        1_i32 << (-signed as u32)
    } else {
        signed as i32 * cluster_size as i32
    }
}

fn format_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("0x{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_volume_serial(serial: u64, low_32: bool, reverse_low_32: bool) -> String {
    let value = if low_32 { serial & 0xFFFF_FFFF } else { serial };
    let value = if reverse_low_32 {
        let low = value as u32;
        low.swap_bytes() as u64
    } else {
        value
    };
    if low_32 {
        format!("{value:08X}")
    } else {
        format!("{value:016X}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sample_boot_sector() {
        if !std::path::Path::new("../../sample_files/$Boot").exists() {
            return; // sample evidence absent — skip (TriageSuite convention)
        }
        let record = parse_boot("../../sample_files/$Boot").unwrap();

        assert_eq!(record.entry_point, "0xEB 0x52 0x90");
        assert_eq!(record.signature, "NTFS    ");
        assert_eq!(record.bytes_per_sector, 512);
        assert_eq!(record.sectors_per_cluster, 8);
        assert_eq!(record.cluster_size, 4096);
        assert_eq!(record.mft_entry_size, 1024);
        assert_eq!(record.index_entry_size, 4096);
        assert_eq!(record.sector_signature, "0xAA55");
    }

    #[test]
    fn rejects_short_boot_sector() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("$Boot");
        fs::write(&path, [0_u8; 511]).unwrap();

        let err = parse_boot(&path).unwrap_err();

        match err {
            MftriageError::MalformedArtifact {
                path: err_path,
                message,
            } => {
                assert_eq!(err_path, path);
                assert_eq!(message, "boot sector is shorter than 512 bytes");
            }
            other => panic!("expected malformed artifact error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_non_ntfs_boot_sector() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("$Boot");
        let mut bytes = [0_u8; 512];
        bytes[510..512].copy_from_slice(&0xAA55_u16.to_le_bytes());
        fs::write(&path, bytes).unwrap();

        let err = parse_boot(&path).unwrap_err();

        match err {
            MftriageError::MalformedArtifact {
                path: err_path,
                message,
            } => {
                assert_eq!(err_path, path);
                assert!(message.contains("invalid NTFS boot signature"));
            }
            other => panic!("expected malformed artifact error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_sector_signature() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("$Boot");
        let mut bytes = [0_u8; 512];
        bytes[3..11].copy_from_slice(b"NTFS    ");
        fs::write(&path, bytes).unwrap();

        let err = parse_boot(&path).unwrap_err();

        match err {
            MftriageError::MalformedArtifact {
                path: err_path,
                message,
            } => {
                assert_eq!(err_path, path);
                assert!(message.contains("invalid boot sector signature"));
            }
            other => panic!("expected malformed artifact error, got {other:?}"),
        }
    }
}
