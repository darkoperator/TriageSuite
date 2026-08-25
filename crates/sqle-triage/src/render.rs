//! Render a SQLite cell value to a CSV/JSON string. Maps do their own date
//! formatting in SQL (via `datetime(...)`), so this is purely type→text.

use triage_sqlite::SqliteValue;

/// Marker written in place of BLOB content when `--noblob` is set. SQLECmd's
/// exact marker is confirmed against fixtures; update this literal if the
/// fixture run shows a different string.
pub const NOBLOB_MARKER: &str = "<BLOB>";

/// Render a value to its cell text. `noblob` replaces BLOB content with
/// `NOBLOB_MARKER`; otherwise a BLOB is rendered as lowercase hex.
pub fn render_cell(v: &SqliteValue, noblob: bool) -> String {
    match v {
        SqliteValue::Null => String::new(),
        SqliteValue::Integer(i) => i.to_string(),
        SqliteValue::Real(f) => render_real(*f),
        SqliteValue::Text(s) => s.clone(),
        SqliteValue::Blob(b) => {
            if noblob {
                NOBLOB_MARKER.to_string()
            } else {
                // SQLECmd renders BLOBs as "0x" + uppercase hex (confirmed against
                // fixtures, e.g. Windows_ActivitiesCacheDB Payload column).
                let hex: String = b.iter().map(|byte| format!("{byte:02X}")).collect();
                format!("0x{hex}")
            }
        }
    }
}

/// Render a REAL the way SQLECmd (.NET) renders it: integer-valued finite
/// floats are rendered without a decimal point (e.g. `0.0` → `"0"`,
/// `5.0` → `"5"`). Non-integer values use Rust's default `f64` Display
/// (which matches .NET's round-trip format for the values SQLite produces).
/// Pinned against fixtures.
fn render_real(f: f64) -> String {
    if f == f.trunc() && f.is_finite() {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_scalar_types() {
        assert_eq!(render_cell(&SqliteValue::Null, false), "");
        assert_eq!(render_cell(&SqliteValue::Integer(42), false), "42");
        assert_eq!(render_cell(&SqliteValue::Text("hi".into()), false), "hi");
    }

    #[test]
    fn blob_hex_vs_noblob() {
        // SQLECmd renders BLOBs as "0x" + uppercase hex (confirmed against fixtures).
        let b = SqliteValue::Blob(vec![0x00, 0xff, 0x10]);
        assert_eq!(render_cell(&b, false), "0x00FF10");
        assert_eq!(render_cell(&b, true), NOBLOB_MARKER);
    }

    #[test]
    fn real_rendering() {
        // Integer-valued REALs render without decimal point (matches SQLECmd/.NET).
        assert_eq!(render_cell(&SqliteValue::Real(0.0), false), "0");
        assert_eq!(render_cell(&SqliteValue::Real(2.0), false), "2");
        assert_eq!(render_cell(&SqliteValue::Real(5.0), false), "5");
        assert_eq!(render_cell(&SqliteValue::Real(-3.0), false), "-3");
        // Non-integer REALs keep their decimal places.
        assert_eq!(render_cell(&SqliteValue::Real(1.5), false), "1.5");
        assert_eq!(render_cell(&SqliteValue::Real(0.206503), false), "0.206503");
    }
}
