//! Render notatin values into RECmd's `ValueType`/`ValueData` string forms,
//! and apply `.reb` BinaryConvert conversions (BuildBatchCsvOut, Program.cs).
//! The exact ValueData spellings are pinned by the RECmd compat fixtures; the
//! mappings below are the documented starting point.

use notatin::cell_key_value::{CellKeyValue, CellKeyValueDataTypes};
use notatin::cell_value::CellValue;

/// A rendered value as it appears in BatchCsvOut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedValue {
    pub value_type: String,
    pub value_data: String,
}

/// RECmd `ValueType` string for a notatin data type (Registry-lib spellings).
pub fn value_type_string(dt: CellKeyValueDataTypes) -> &'static str {
    use CellKeyValueDataTypes as T;
    match dt {
        T::REG_NONE => "RegNone",
        T::REG_SZ => "RegSz",
        T::REG_EXPAND_SZ => "RegExpandSz",
        T::REG_BIN => "RegBinary",
        T::REG_DWORD => "RegDword",
        T::REG_DWORD_BIG_ENDIAN => "RegDwordBigEndian",
        T::REG_LINK => "RegLink",
        T::REG_MULTI_SZ => "RegMultiSz",
        T::REG_RESOURCE_LIST => "RegResourceList",
        T::REG_FULL_RESOURCE_DESCRIPTOR => "RegFullResourceDescription",
        T::REG_RESOURCE_REQUIREMENTS_LIST => "RegResourceRequirementsList",
        T::REG_QWORD => "RegQword",
        T::REG_FILETIME => "RegFileTime",
        _ => "RegUnknown",
    }
}

/// Uppercase dash-separated hex (`DE-AD-BE-EF`) — C# BitConverter.ToString.
pub fn bytes_to_hex_dashed(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join("-")
}

/// Render a value's `ValueData` from its decoded CellValue (no BinaryConvert).
pub fn render_value_data(content: &CellValue) -> String {
    match content {
        CellValue::None => String::new(),
        CellValue::String(s) => s.clone(),
        CellValue::MultiString(parts) => parts.join(" "),
        CellValue::U32(n) => n.to_string(),
        CellValue::I32(n) => n.to_string(),
        CellValue::U64(n) => n.to_string(),
        CellValue::I64(n) => n.to_string(),
        CellValue::Binary(b) => bytes_to_hex_dashed(b),
        CellValue::Error => String::new(),
    }
}

/// Render `(ValueType, ValueData)` for a value. Binary types report
/// `(Binary data)` for ValueData (batch overrides this when IncludeBinary).
pub fn render(value: &CellKeyValue) -> RenderedValue {
    let vt = value_type_string(value.data_type).to_string();
    let (content, _) = value.get_content();
    let data = if value.data_type == CellKeyValueDataTypes::REG_BIN {
        "(Binary data)".to_string()
    } else {
        render_value_data(&content)
    };
    RenderedValue {
        value_type: vt,
        value_data: data,
    }
}

use chrono::{DateTime, TimeZone, Utc};
use triage_core::timestamp::WinTimestamp;

/// Convert a chrono UTC datetime to WinTimestamp (no direct ctor exists).
fn win_from_dt(dt: DateTime<Utc>) -> WinTimestamp {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos())
}

/// WinTimestamp -> Option<String>: None/empty render becomes None so callers
/// can fall back to the default ValueData (matching RECmd's catch blocks).
fn ts_opt(ts: WinTimestamp) -> Option<String> {
    let s = ts.to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// The `.reb` BinaryConvert kinds (ReBatch.cs Key.BinConvert).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryConvert {
    None,
    Filetime,
    Ip,
    Epoch,
    Sid,
    Systemtime,
    DateTimeTicks,
    Ole,
}

impl BinaryConvert {
    pub fn from_reb(s: &str) -> BinaryConvert {
        match s.trim().to_ascii_lowercase().as_str() {
            "filetime" => BinaryConvert::Filetime,
            "ip" => BinaryConvert::Ip,
            "epoch" => BinaryConvert::Epoch,
            "sid" => BinaryConvert::Sid,
            "systemtime" => BinaryConvert::Systemtime,
            "datetimeticks" => BinaryConvert::DateTimeTicks,
            "ole" => BinaryConvert::Ole,
            _ => BinaryConvert::None,
        }
    }
}

/// Apply a BinaryConvert over raw value bytes, returning the rendered string,
/// or `None` if the bytes don't fit (caller falls back to default ValueData).
pub fn apply_binary_convert(kind: BinaryConvert, raw: &[u8]) -> Option<String> {
    match kind {
        BinaryConvert::None => None,
        BinaryConvert::Filetime => {
            let n = u64::from_le_bytes(raw.get(0..8)?.try_into().ok()?);
            ts_opt(WinTimestamp::from_filetime(n))
        }
        BinaryConvert::Epoch => {
            let secs = u32::from_le_bytes(raw.get(0..4)?.try_into().ok()?);
            ts_opt(WinTimestamp::from_unix(secs as i64))
        }
        BinaryConvert::Ip => {
            let n = u32::from_le_bytes(raw.get(0..4)?.try_into().ok()?);
            // .NET IPAddress(long) is low-byte-first (0x0100007F => 127.0.0.1),
            // so to_le_bytes order matches RECmd; do not 'fix' to to_be_bytes.
            let b = n.to_le_bytes();
            Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]))
        }
        BinaryConvert::DateTimeTicks => {
            let ticks = i64::from_le_bytes(raw.get(0..8)?.try_into().ok()?);
            // .NET ticks: 100ns since 0001-01-01. Convert to Unix seconds+nanos.
            const TICKS_PER_SEC: i64 = 10_000_000;
            const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
            let diff = ticks.checked_sub(UNIX_EPOCH_TICKS)?;
            let secs = diff.div_euclid(TICKS_PER_SEC);
            let nanos = (diff.rem_euclid(TICKS_PER_SEC) * 100) as u32;
            ts_opt(WinTimestamp::from_unix_nanos(secs, nanos))
        }
        BinaryConvert::Ole => {
            let f = f64::from_le_bytes(raw.get(0..8)?.try_into().ok()?);
            // OLE Automation date: days since 1899-12-30.
            // RECmd's OLE branch uniquely does NOT call .ToUniversalTime() (unlike
            // the other date conversions); we treat the OLE date as UTC. If a real
            // OLE-typed value ever appears in the Task 8 fixtures, this is the
            // first place to check for a divergence.
            let base = Utc.with_ymd_and_hms(1899, 12, 30, 0, 0, 0).single()?;
            let dt: DateTime<Utc> =
                base + chrono::Duration::milliseconds((f * 86_400_000.0) as i64);
            ts_opt(win_from_dt(dt))
        }
        BinaryConvert::Systemtime => {
            let rd = |o: usize| -> Option<i32> {
                Some(i16::from_le_bytes(raw.get(o..o + 2)?.try_into().ok()?) as i32)
            };
            let (y, mo, d) = (rd(0)?, rd(2)?, rd(6)?); // wYear, wMonth, wDay (skip wDayOfWeek)
            let (h, mi, s) = (rd(8)?, rd(10)?, rd(12)?);
            let ms = rd(14)?; // wMilliseconds (offset 14), passed to DateTime ctor by RECmd
            let dt = Utc
                .with_ymd_and_hms(y, mo as u32, d as u32, h as u32, mi as u32, s as u32)
                .single()?
                + chrono::Duration::milliseconds(ms as i64);
            ts_opt(win_from_dt(dt))
        }
        BinaryConvert::Sid => {
            // Non-Windows parity with RECmd's macOS path.
            Some(format!(
                "<SID conversion only available on Windows. Using bytes instead>: {}",
                bytes_to_hex_dashed(raw)
            ))
        }
    }
}

/// Best-effort raw value bytes (RECmd `ValueDataRaw`). notatin's raw-byte
/// accessor is crate-private, so reconstruct from the decoded CellValue:
/// Binary returns bytes verbatim; integer types return little-endian bytes
/// (exactly what the .reb BinaryConvert conversions read). String/None/Error
/// yield empty — DFIRBatch only applies BinaryConvert to binary/dword/qword.
pub fn raw_bytes(value: &CellKeyValue) -> Vec<u8> {
    let (content, _) = value.get_content();
    match content {
        CellValue::Binary(b) => b,
        CellValue::U32(n) => n.to_le_bytes().to_vec(),
        CellValue::I32(n) => n.to_le_bytes().to_vec(),
        CellValue::U64(n) => n.to_le_bytes().to_vec(),
        CellValue::I64(n) => n.to_le_bytes().to_vec(),
        _ => Vec::new(),
    }
}

/// Raw bytes for `PluginValue.raw` (C# `ValueDataRaw` equivalent for all types).
///
/// Like `raw_bytes` but also handles string types: REG_SZ and REG_EXPAND_SZ are
/// encoded as UTF-16LE with a 2-byte null terminator (matching what Windows stores
/// on disk and what RECmd's `ValueDataRaw` holds for string values).
///
/// Takes a pre-decoded `CellValue` so the caller can derive both this and
/// `value_data` from a single `get_content()` call.
pub fn plugin_raw_bytes(content: &CellValue) -> Vec<u8> {
    match content {
        CellValue::String(s) => {
            let mut bytes: Vec<u8> = s.encode_utf16().flat_map(|w| w.to_le_bytes()).collect();
            bytes.extend_from_slice(&[0u8, 0u8]); // UTF-16LE null terminator
            bytes
        }
        CellValue::Binary(b) => b.clone(),
        CellValue::U32(n) => n.to_le_bytes().to_vec(),
        CellValue::I32(n) => n.to_le_bytes().to_vec(),
        CellValue::U64(n) => n.to_le_bytes().to_vec(),
        CellValue::I64(n) => n.to_le_bytes().to_vec(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_dashed_matches_bitconverter() {
        assert_eq!(
            bytes_to_hex_dashed(&[0xde, 0xad, 0xbe, 0xef]),
            "DE-AD-BE-EF"
        );
        assert_eq!(bytes_to_hex_dashed(&[]), "");
    }

    #[test]
    fn dword_is_decimal() {
        assert_eq!(render_value_data(&CellValue::U32(4096)), "4096");
    }

    #[test]
    fn multistring_space_joined() {
        let v = CellValue::MultiString(vec!["a".into(), "b".into()]);
        assert_eq!(render_value_data(&v), "a b");
    }

    #[test]
    fn none_is_empty() {
        assert_eq!(render_value_data(&CellValue::None), "");
    }
}

#[cfg(test)]
mod convert_tests {
    use super::*;

    #[test]
    fn filetime_known_value() {
        // 2009-07-14 04:20:36.5135000 UTC == 0x01CA0451AD6E4000.
        let ft: u64 = 0x01CA_0451_AD6E_4000;
        let out = apply_binary_convert(BinaryConvert::Filetime, &ft.to_le_bytes()).unwrap();
        assert!(out.starts_with("2009-07-14"), "got {out}");
    }

    #[test]
    fn ip_le_octets() {
        // bytes 0x08 0x08 0x08 0x08 -> 8.8.8.8
        let out = apply_binary_convert(BinaryConvert::Ip, &[8, 8, 8, 8]).unwrap();
        assert_eq!(out, "8.8.8.8");
    }

    #[test]
    fn ip_non_uniform_byte_order() {
        // [1,2,3,4] must render 1.2.3.4, not 4.3.2.1 — locks LE byte order.
        let out = apply_binary_convert(BinaryConvert::Ip, &[1, 2, 3, 4]).unwrap();
        assert_eq!(out, "1.2.3.4");
    }

    #[test]
    fn systemtime_includes_milliseconds() {
        // 2020-01-02 03:04:05.678 UTC
        // SYSTEMTIME layout (8 x LE i16): year, month, dayOfWeek, day, hour, minute, second, millis
        let mut raw = [0u8; 16];
        raw[0..2].copy_from_slice(&2020i16.to_le_bytes()); // wYear
        raw[2..4].copy_from_slice(&1i16.to_le_bytes()); // wMonth
        raw[4..6].copy_from_slice(&4i16.to_le_bytes()); // wDayOfWeek (ignored)
        raw[6..8].copy_from_slice(&2i16.to_le_bytes()); // wDay
        raw[8..10].copy_from_slice(&3i16.to_le_bytes()); // wHour
        raw[10..12].copy_from_slice(&4i16.to_le_bytes()); // wMinute
        raw[12..14].copy_from_slice(&5i16.to_le_bytes()); // wSecond
        raw[14..16].copy_from_slice(&678i16.to_le_bytes()); // wMilliseconds
        let out = apply_binary_convert(BinaryConvert::Systemtime, &raw).unwrap();
        // WinTimestamp renders 7 fractional digits; assert millisecond-precision prefix.
        assert!(out.starts_with("2020-01-02T03:04:05.678"), "got {out}");
    }

    #[test]
    fn too_short_returns_none() {
        assert!(apply_binary_convert(BinaryConvert::Filetime, &[0, 0]).is_none());
    }
}
