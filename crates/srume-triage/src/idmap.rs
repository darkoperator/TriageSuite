//! Build the SruDbIdMapTable id->app / id->user maps. Each row maps a small
//! integer (`IdIndex`) to a blob whose meaning depends on `IdType`: app/service
//! entries are UTF-16LE strings; user entries are binary SIDs.

use std::collections::HashMap;

use triage_ese::{Database, EseValue};

/// App/service id -> decoded executable info.
#[derive(Debug, Clone, Default)]
pub struct AppInfo {
    pub exe_info: String,
    pub exe_info_description: String,
    pub exe_timestamp: Option<String>,
}

/// The two id maps for one SRUDB.
#[derive(Debug, Default)]
pub struct IdMaps {
    pub apps: HashMap<i64, AppInfo>,
    pub sids: HashMap<i64, String>,
}

/// Read SruDbIdMapTable. `IdType` distinguishes a SID entry (type 3) from app
/// entries (other types). (Per SrumECmd `Srum.cs` MapType.Sid.)
pub fn build_id_maps(db: &Database) -> Result<IdMaps, triage_ese::EseError> {
    let cols = db.columns("SruDbIdMapTable")?;
    let pos = |n: &str| cols.iter().position(|c| c.name == n);
    let (ti, ii, bi) = match (pos("IdType"), pos("IdIndex"), pos("IdBlob")) {
        (Some(t), Some(i), Some(b)) => (t, i, b),
        _ => return Ok(IdMaps::default()),
    };
    let mut maps = IdMaps::default();
    for row in db.rows("SruDbIdMapTable")? {
        let id_type = row[ti].as_i64().unwrap_or(-1);
        let idx = match row[ii].as_i64() {
            Some(v) => v,
            None => continue,
        };
        let blob = match &row[bi] {
            EseValue::Bytes(b) => b.as_slice(),
            _ => continue,
        };
        if id_type == 3 {
            if let Some(sid) = decode_sid(blob) {
                maps.sids.insert(idx, sid);
            }
        } else {
            let s = utf16le_trim_nul(blob);
            maps.apps.insert(idx, decode_app_info(&s));
        }
    }
    Ok(maps)
}

/// Decode a UTF-16LE blob, trimming trailing NULs.
fn utf16le_trim_nul(b: &[u8]) -> String {
    let u16s: Vec<u16> = b
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect();
    String::from_utf16_lossy(&u16s)
        .trim_end_matches('\0')
        .to_string()
}

/// Decode a binary SID (`S-1-...`). Returns None if malformed.
fn decode_sid(b: &[u8]) -> Option<String> {
    if b.len() < 8 {
        return None;
    }
    let revision = b[0];
    let sub_count = b[1] as usize;
    if b.len() < 8 + sub_count * 4 {
        return None;
    }
    let mut authority: u64 = 0;
    for &byte in &b[2..8] {
        authority = (authority << 8) | byte as u64;
    }
    let mut s = format!("S-{revision}-{authority}");
    for i in 0..sub_count {
        let off = 8 + i * 4;
        let sub = u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]);
        s.push_str(&format!("-{sub}"));
    }
    Some(s)
}

/// Decode the SrumECmd `!!exe!...` executable-info form (Srum.cs AppInfo).
/// A value not starting with `!!` is returned as-is in `exe_info`.
fn decode_app_info(raw: &str) -> AppInfo {
    if !raw.starts_with("!!") {
        return AppInfo {
            exe_info: raw.to_string(),
            ..Default::default()
        };
    }
    let segs: Vec<&str> = raw[2..].split('!').collect();
    if segs.len() < 4 {
        return AppInfo {
            exe_info: raw.to_string(),
            ..Default::default()
        };
    }
    let n = segs.len();
    let mut exe_info = if segs[0].is_empty() {
        "!".to_string()
    } else {
        segs[0].to_string()
    };
    for &seg in &segs[1..n - 3] {
        if seg.is_empty() {
            exe_info.push('!');
        } else {
            exe_info.push('!');
            exe_info.push_str(seg);
        }
    }
    let exe_timestamp = chrono::NaiveDateTime::parse_from_str(segs[n - 3], "%Y/%m/%d:%H:%M:%S")
        .ok()
        .map(|dt| {
            triage_core::timestamp::WinTimestamp::from_unix(dt.and_utc().timestamp()).to_string()
        });
    AppInfo {
        exe_info,
        exe_info_description: segs[n - 1].to_string(),
        exe_timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sid_decode() {
        // S-1-5-18 (Local System): rev 1, 1 subauth, authority 5, subauth 18.
        let b = [1u8, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];
        assert_eq!(decode_sid(&b).unwrap(), "S-1-5-18");
    }

    #[test]
    fn app_info_plain() {
        let a = decode_app_info("\\Device\\HarddiskVolume2\\Windows\\System32\\svchost.exe");
        assert_eq!(
            a.exe_info,
            "\\Device\\HarddiskVolume2\\Windows\\System32\\svchost.exe"
        );
        assert!(a.exe_info_description.is_empty());
    }

    #[test]
    fn app_info_bang_form() {
        let a = decode_app_info("!!svchost.exe!2024/06/29:00:05:00!something!netsvcs");
        assert_eq!(a.exe_info, "svchost.exe");
        assert_eq!(a.exe_info_description, "netsvcs");
        assert!(a.exe_timestamp.is_some());
    }
}
