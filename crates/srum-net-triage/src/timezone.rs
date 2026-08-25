//! Optional `--tz` auto-detection from a SYSTEM hive's
//! `ControlSet00N\Control\TimeZoneInformation` key. Opt-in via
//! `--system-hive`; an explicit `--tz` always overrides this.
//!
//! This reads a single static offset snapshot (`ActiveTimeBias` if present,
//! else `Bias` + `StandardBias`) — not a full per-timestamp DST calculation
//! against `StandardStart`/`DaylightStart`. A capture spanning a DST
//! transition will have part of its data off by the DST delta (normally 60
//! minutes). See docs/tools/SrumNetTriage.md's Known limitations.

use std::path::Path;

use triage_registry::value::raw_bytes;
use triage_registry::Hive;

use crate::aggregate::TzOffset;

fn read_i32(key: &notatin::cell_key_node::CellKeyNode, name: &str) -> Option<i32> {
    let value = key.get_value(name)?;
    let raw = raw_bytes(&value);
    (raw.len() >= 4).then(|| i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

/// Windows stores `Bias` as UTC = local + Bias (minutes); local time is
/// therefore UTC - Bias, i.e. the `TzOffset` to add to UTC is `-Bias`.
fn bias_to_tz_offset(total_bias_minutes: i32) -> TzOffset {
    TzOffset(-total_bias_minutes)
}

pub fn detect_from_system_hive(path: &Path) -> Result<TzOffset, String> {
    let mut hive = Hive::open(path, &[], true).map_err(|e| format!("cannot open hive: {e}"))?;

    let current_control_set = hive
        .get_key("Select")
        .and_then(|key| read_i32(&key, "Current"))
        .ok_or_else(|| r"could not resolve Select\Current control set".to_string())?;

    let key_path = format!(r"ControlSet{current_control_set:03}\Control\TimeZoneInformation");
    let tzi_key = hive
        .get_key(&key_path)
        .ok_or_else(|| format!("{key_path} not found"))?;

    let total_bias = if let Some(active) = read_i32(&tzi_key, "ActiveTimeBias") {
        active
    } else {
        let bias = read_i32(&tzi_key, "Bias").ok_or("no Bias value found")?;
        let standard_bias = read_i32(&tzi_key, "StandardBias").unwrap_or(0);
        bias + standard_bias
    };

    Ok(bias_to_tz_offset(total_bias))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn bias_to_tz_offset_inverts_sign() {
        // UTC-5 (Eastern Standard Time) stores Bias = 300.
        assert_eq!(bias_to_tz_offset(300), TzOffset(-300));
        // UTC+1 (e.g. CET) stores Bias = -60.
        assert_eq!(bias_to_tz_offset(-60), TzOffset(60));
        assert_eq!(bias_to_tz_offset(0), TzOffset(0));
    }

    /// Same "skip if fixture absent" convention as
    /// `triage-registry/src/hive.rs`'s `opens_software_hive_and_reads_root`.
    fn find_system_hive() -> Option<PathBuf> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
        if !root.exists() {
            return None;
        }
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_name().and_then(|s| s.to_str()) == Some("SYSTEM") {
                    return Some(path);
                }
            }
        }
        None
    }

    #[test]
    fn detects_offset_from_real_system_hive_if_available() {
        let Some(path) = find_system_hive() else {
            eprintln!("SKIP: no SYSTEM hive in test captures");
            return;
        };
        let tz = detect_from_system_hive(&path).expect("detect offset");
        // Sanity bound: any real-world UTC offset falls within -12h..+14h.
        assert!(tz.0 >= -720 && tz.0 <= 840);
    }

    #[test]
    fn missing_hive_file_is_an_error() {
        let result = detect_from_system_hive(Path::new("/nonexistent/SYSTEM"));
        assert!(result.is_err());
    }
}
