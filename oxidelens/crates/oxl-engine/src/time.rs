//! Time as text.
//!
//! Time values cross the engine as UTC ISO 8601 text and are cast inside
//! DuckDB; nothing here converts to or from an epoch. Every timestamp the
//! engine itself generates has one shape, `YYYY-MM-DDTHH:MM:SS.ffffffZ`:
//! six fractional digits, because that is DuckDB's `TIMESTAMP` resolution,
//! and a fixed width, so that the text sorts in time order.

use chrono::Utc;

/// The variable that pins "now", so a test can predict every timestamp the
/// engine writes. Ignored unless it passes [`parse_utc_text`].
const NOW_VAR: &str = "OXIDELENS_NOW";

/// The current time in the canonical 6-digit `Z` form.
pub fn now_iso() -> String {
    if let Some(pinned) = std::env::var(NOW_VAR)
        .ok()
        .and_then(|text| parse_utc_text(&text))
    {
        return pinned;
    }
    let now = Utc::now();
    // `dt_to_iso8601` writes 7 fractional digits; the canonical form has 6.
    // The fallback is unreachable for a clock value but keeps this total.
    parse_utc_text(&triage_core::timestamp::dt_to_iso8601(now))
        .unwrap_or_else(|| now.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string())
}

/// Accept `YYYY-MM-DDTHH:MM:SS[.f{1,7}]Z` and nothing else, returned in the
/// canonical form (the fraction cut or padded to 6 digits).
///
/// Every field is range-checked by comparison, including the day against
/// its month, so that text accepted here is text DuckDB will also cast. No
/// zone other than `Z`, no space separator, no leap second.
pub fn parse_utc_text(text: &str) -> Option<String> {
    let b = text.as_bytes();
    if b.len() < 20 || b.last() != Some(&b'Z') {
        return None;
    }
    let separators = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')];
    if separators.iter().any(|&(i, c)| b[i] != c) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<u32> {
        b[range].iter().try_fold(0u32, |acc, &d| {
            d.is_ascii_digit().then(|| acc * 10 + u32::from(d - b'0'))
        })
    };
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;

    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if year == 0 || day == 0 || day > month_days || hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    // What sits between the seconds and the `Z`: nothing, or `.` and 1 to 7 digits.
    let fraction = &b[19..b.len() - 1];
    let digits = match fraction.split_first() {
        None => &[][..],
        Some((b'.', digits)) if (1..=7).contains(&digits.len()) => digits,
        Some(_) => return None,
    };
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut micros: String = digits.iter().take(6).map(|&d| char::from(d)).collect();
    while micros.len() < 6 {
        micros.push('0');
    }
    // Bytes 0..19 are all ASCII (checked above), so this slice is on a
    // character boundary.
    Some(format!("{}.{micros}Z", &text[..19]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_canonical() {
        let now = now_iso();
        assert_eq!(parse_utc_text(&now).as_deref(), Some(now.as_str()), "{now}");
        assert_eq!(now.len(), "2026-01-05T02:13:58.204000Z".len());
    }

    #[test]
    fn parse_survives_every_short_and_odd_input() {
        // Each prefix of a valid value, and the value with each byte
        // replaced, must return rather than panic.
        let valid = "2026-01-05T02:13:58.2040000Z";
        for end in 0..valid.len() {
            let _ = parse_utc_text(&valid[..end]);
        }
        for i in 0..valid.len() {
            for c in ['\0', 'é', '9', ':', 'Z', '.'] {
                let mut s: Vec<char> = valid.chars().collect();
                s[i] = c;
                let _ = parse_utc_text(&s.into_iter().collect::<String>());
            }
        }
    }
}
