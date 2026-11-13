//! A cell's value split into the fields it packs, for the value viewer.
//!
//! The text is the case's, so it is untrusted: every input gives an answer,
//! at least one part and at most `MAX_PARTS`, in time linear in its length.
//! Each way of splitting walks the text once; JSON goes through
//! `serde_json`, which reads the top-level object's values as raw text
//! without recursing into them.

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;
use serde_json::Value;
use std::fmt;

/// Parts one value is split into at most; the rest is folded into the last.
pub const MAX_PARTS: usize = 1_000;

/// One field of a packed value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ValuePart {
    pub key: Option<String>,
    pub value: String,
}

/// Hayabusa's field separator in `Details` and `ExtraFieldInfo`.
const HAYABUSA_SEPARATOR: char = '¦';

/// `text` split into the fields it packs, by the first of these that finds
/// fields in it:
///
/// 1. a JSON object (after surrounding whitespace): one part per top-level
///    entry in the order written, repeated keys too, each value its raw JSON
///    text as written (a string with its quotes), trimmed;
/// 2. Hayabusa's `¦`-separated fields, each `Key: value` (split at the first
///    `: `; a field without one is unkeyed), at least two of them;
/// 3. tab- or newline-separated fields (`\r\n` too), unkeyed, at least two
///    of them;
/// 4. otherwise one unkeyed part holding the whole text, as for an empty
///    text.
///
/// Empty fields are dropped. Past `MAX_PARTS - 1` fields the rest is folded,
/// unsplit and unkeyed, into the last part: the rest of the text as written,
/// or for JSON an object of the remaining entries (keys re-quoted, values as
/// written).
pub fn split_value(text: &str) -> Vec<ValuePart> {
    if let Some(parts) = json_object(text) {
        return parts;
    }
    if text.contains(HAYABUSA_SEPARATOR) {
        if let Some(parts) = fields(text, |c| c == HAYABUSA_SEPARATOR, hayabusa_field) {
            return parts;
        }
    }
    if text.contains(['\t', '\n']) {
        if let Some(parts) = fields(text, |c| c == '\t' || c == '\n', line_field) {
            return parts;
        }
    }
    vec![ValuePart {
        key: None,
        value: text.to_string(),
    }]
}

/// The parts of a JSON object with at least one key; `None` for anything
/// else, including a document `serde_json` refuses.
///
/// Each value is its raw JSON text as the cell wrote it, trimmed: a string
/// keeps its quotes and escapes (`"1"` stays apart from `1`), a number every
/// digit. Nothing is parsed into a `Value`, so no number is converted, and
/// every entry is kept in order, a repeated key too. serde_json skips over
/// a raw value with a loop and a heap stack rather than by recursion, so a
/// deeply nested value is kept whole, not refused.
fn json_object(text: &str) -> Option<Vec<ValuePart>> {
    let trimmed = text.trim();
    if !(trimmed.starts_with('{') && trimmed.ends_with('}')) {
        return None;
    }
    let RawEntries(entries) = serde_json::from_str(trimmed).ok()?;
    if entries.is_empty() {
        return None;
    }
    let mut entries = entries.into_iter();
    let mut parts: Vec<ValuePart> = entries
        .by_ref()
        .take(MAX_PARTS - 1)
        .map(|(key, raw)| ValuePart {
            key: Some(key),
            value: raw.get().trim().to_string(),
        })
        .collect();
    // The rest as one object: each key re-quoted, each value as written.
    let rest: Vec<String> = entries
        .map(|(key, raw)| format!("{}:{}", Value::String(key), raw.get().trim()))
        .collect();
    if !rest.is_empty() {
        parts.push(ValuePart {
            key: None,
            value: format!("{{{}}}", rest.join(",")),
        });
    }
    Some(parts)
}

/// A JSON object's top-level entries in the order written, each value as
/// its raw text, repeated keys kept.
struct RawEntries<'a>(Vec<(String, &'a RawValue)>);

impl<'de> Deserialize<'de> for RawEntries<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;
        impl<'de> Visitor<'de> for Entries {
            type Value = RawEntries<'de>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<RawEntries<'de>, A::Error> {
                let mut out = Vec::new();
                while let Some(entry) = map.next_entry::<String, &'de RawValue>()? {
                    out.push(entry);
                }
                Ok(RawEntries(out))
            }
        }
        deserializer.deserialize_map(Entries)
    }
}

/// One `¦` field: trimmed; `Key: value` split at the first `: ` when the key
/// is not blank. `None` for a blank field.
fn hayabusa_field(piece: &str) -> Option<ValuePart> {
    let piece = piece.trim();
    if piece.is_empty() {
        return None;
    }
    Some(match piece.split_once(": ") {
        Some((key, value)) if !key.trim().is_empty() => ValuePart {
            key: Some(key.trim().to_string()),
            value: value.trim().to_string(),
        },
        _ => ValuePart {
            key: None,
            value: piece.to_string(),
        },
    })
}

/// One tab- or newline-separated field, without a `\r` that ended its line.
/// `None` for an empty field.
fn line_field(piece: &str) -> Option<ValuePart> {
    let piece = piece.strip_suffix('\r').unwrap_or(piece);
    if piece.trim().is_empty() {
        return None;
    }
    Some(ValuePart {
        key: None,
        value: piece.to_string(),
    })
}

/// `text` split at every character `is_separator` accepts, each piece made a
/// part by `part` (`None` drops it); `None` unless at least two parts come
/// of it. Past `MAX_PARTS - 1` parts, the text from the next field to the
/// end is the last part, as written (trimmed of surrounding whitespace).
///
/// One pass: the pieces are walked in order, and the check for a field
/// after the cap's last one walks the remaining text once and stops.
fn fields(
    text: &str,
    is_separator: impl Fn(char) -> bool + Copy,
    part: fn(&str) -> Option<ValuePart>,
) -> Option<Vec<ValuePart>> {
    let mut parts = Vec::new();
    let mut start = 0;
    for piece in text.split(is_separator) {
        let piece_start = start;
        // `split` yields the text between separators; each separator taken
        // here is one character, of its own UTF-8 length.
        start += piece.len();
        let separator_len = text[start..].chars().next().map_or(0, char::len_utf8);
        start += separator_len;
        let Some(found) = part(piece) else {
            continue;
        };
        if parts.len() == MAX_PARTS - 1 {
            let after = &text[start.min(text.len())..];
            let more = separator_len > 0 && after.split(is_separator).any(|p| part(p).is_some());
            if more {
                parts.push(ValuePart {
                    key: None,
                    value: text[piece_start..].trim().to_string(),
                });
                break;
            }
        }
        parts.push(found);
        if parts.len() == MAX_PARTS {
            break;
        }
    }
    (parts.len() >= 2).then_some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(key: Option<&str>, value: &str) -> ValuePart {
        ValuePart {
            key: key.map(str::to_string),
            value: value.to_string(),
        }
    }

    fn whole(text: &str) -> Vec<ValuePart> {
        vec![kv(None, text)]
    }

    #[test]
    fn split_hayabusa_details() {
        assert_eq!(
            split_value("User: Administrator ¦ SrcIP: 203.0.113.7"),
            vec![
                kv(Some("User"), "Administrator"),
                kv(Some("SrcIP"), "203.0.113.7")
            ]
        );
        // A field without `: ` keeps its text, unkeyed; a value may hold
        // `: ` itself; empty fields are dropped.
        assert_eq!(
            split_value("Cmdline: cmd /c echo a: b ¦ orphan ¦  ¦ Tgt: x"),
            vec![
                kv(Some("Cmdline"), "cmd /c echo a: b"),
                kv(None, "orphan"),
                kv(Some("Tgt"), "x"),
            ]
        );
        // One field alone is not split.
        assert_eq!(split_value("User: svc_backup"), whole("User: svc_backup"));
    }

    #[test]
    fn split_json_object() {
        assert_eq!(
            split_value(r#" {"b": "two", "a": 1, "n": null, "o": {"y": [1, 2], "x": true}} "#),
            vec![
                // Top-level entries in the order written, each value its raw
                // JSON text as written: a string keeps its quotes, a nested
                // value its own spacing.
                kv(Some("b"), r#""two""#),
                kv(Some("a"), "1"),
                kv(Some("n"), "null"),
                kv(Some("o"), r#"{"y": [1, 2], "x": true}"#),
            ]
        );
        assert_eq!(
            split_value(r#"{"User":"Administrator"}"#),
            vec![kv(Some("User"), r#""Administrator""#)]
        );
        // A string and a number stay apart; escapes stay as written.
        assert_eq!(
            split_value(r#"{"s": "1", "i": 1, "e": "a\"bé"}"#),
            vec![
                kv(Some("s"), r#""1""#),
                kv(Some("i"), "1"),
                kv(Some("e"), r#""a\"bé""#),
            ]
        );
        // A repeated key keeps every value, in order.
        assert_eq!(
            split_value(r#"{"k": 1, "j": 2, "k": 3}"#),
            vec![kv(Some("k"), "1"), kv(Some("j"), "2"), kv(Some("k"), "3")]
        );
        // A number keeps every digit, never converted.
        assert_eq!(
            split_value(r#"{"n": 123456789012345678901234567890, "f": 0.10000000000000000000001}"#),
            vec![
                kv(Some("n"), "123456789012345678901234567890"),
                kv(Some("f"), "0.10000000000000000000001"),
            ]
        );
        // Not an object: an array, a scalar, an empty object.
        assert_eq!(split_value("[1,2]"), whole("[1,2]"));
        assert_eq!(split_value("{}"), whole("{}"));
        // JSON wins over the other separators it may contain.
        assert_eq!(
            split_value("{\"k\": \"a ¦ b\\tc\"}"),
            vec![kv(Some("k"), "\"a ¦ b\\tc\"")]
        );
    }

    #[test]
    fn a_deeply_nested_json_value_is_kept_whole() {
        // 200 levels, past serde_json's recursion limit of 128 for a parsed
        // `Value`: the raw value is skipped over without recursion, so the
        // object still splits and the value comes back as written.
        for depth in [200, 100_000] {
            let value = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
            let text = format!(r#"{{"a": {value}, "b": "x"}}"#);
            assert_eq!(
                split_value(&text),
                vec![kv(Some("a"), &value), kv(Some("b"), r#""x""#)],
                "depth {depth}"
            );
        }
        // Unbalanced at depth: refused as JSON, so one whole part.
        let broken = format!(r#"{{"a": {}1}}"#, "[".repeat(200));
        assert_eq!(split_value(&broken), whole(&broken));
    }

    #[test]
    fn split_tabs_and_newlines() {
        assert_eq!(
            split_value("alpha\tbeta\r\ngamma\n\ndelta"),
            vec![
                kv(None, "alpha"),
                kv(None, "beta"),
                kv(None, "gamma"),
                kv(None, "delta"),
            ]
        );
        // A trailing newline alone does not make a second field.
        assert_eq!(split_value("only\n"), whole("only\n"));
    }

    #[test]
    fn split_plain_text_is_one_part() {
        for text in [
            "",
            "plain",
            "C:\\Windows\\System32\\svchost.exe -k netsvcs",
            "<img src=x onerror=alert(1)>",
            "\u{202E}gnp.exe",
            "=HYPERLINK(\"http://x\")",
        ] {
            assert_eq!(split_value(text), whole(text), "{text:?}");
        }
    }

    #[test]
    fn split_is_total() {
        let big_bar = "¦".repeat(1 << 19);
        let big_fields = "k: v ¦ ".repeat(150_000);
        let big_lines = "x\n".repeat(500_000);
        let deep = format!("{{\"a\":{}1{}}}", "[".repeat(5_000), "]".repeat(5_000));
        let wide = format!(
            "{{{}}}",
            (0..5_000)
                .map(|i| format!("\"k{i}\":{i}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let cases: Vec<String> = vec![
            "¦".into(),
            " ¦ ".into(),
            ": ".into(),
            ": ¦ : ".into(),
            "{".into(),
            "{\"a\": 1".into(),
            "{\"a\": 1}}".into(),
            "\0".into(),
            "a\0¦\0b".into(),
            "\t".into(),
            "\n\n\n".into(),
            "\r".into(),
            big_bar.clone(),
            big_fields.clone(),
            big_lines.clone(),
            deep.clone(),
            wide.clone(),
        ];
        for text in &cases {
            let parts = split_value(text);
            assert!(!parts.is_empty(), "{:?}", &text[..text.len().min(20)]);
            assert!(parts.len() <= MAX_PARTS);
        }
        // A lone separator is not a field.
        assert_eq!(split_value("¦"), whole("¦"));
        assert_eq!(split_value(": "), whole(": "));
        assert_eq!(split_value("{\"a\": 1"), whole("{\"a\": 1"));
        assert_eq!(split_value("\0"), whole("\0"));
        assert_eq!(split_value(&big_bar), whole(&big_bar));
        assert_eq!(
            split_value(&deep),
            vec![kv(
                Some("a"),
                &format!("{}1{}", "[".repeat(5_000), "]".repeat(5_000))
            )]
        );
        assert_eq!(
            split_value("a\0¦\0b"),
            vec![kv(None, "a\0"), kv(None, "\0b")]
        );

        // Past the cap the rest is folded, unsplit, into the last part.
        let parts = split_value(&big_fields);
        assert_eq!(parts.len(), MAX_PARTS);
        assert!(parts[..MAX_PARTS - 1]
            .iter()
            .all(|p| *p == kv(Some("k"), "v")));
        let last = &parts[MAX_PARTS - 1];
        assert_eq!(last.key, None);
        let folded = 150_000 - (MAX_PARTS - 1);
        assert!(last.value.starts_with("k: v ¦ k: v"));
        assert_eq!(last.value.matches("k: v").count(), folded);

        let parts = split_value(&big_lines);
        assert_eq!(parts.len(), MAX_PARTS);
        assert_eq!(
            parts[MAX_PARTS - 1].value.matches('x').count(),
            500_000 - (MAX_PARTS - 1)
        );

        let parts = split_value(&wide);
        assert_eq!(parts.len(), MAX_PARTS);
        assert_eq!(parts[0], kv(Some("k0"), "0"));
        assert_eq!(parts[MAX_PARTS - 2], kv(Some("k998"), "998"));
        let last = &parts[MAX_PARTS - 1];
        assert_eq!(last.key, None);
        let rest: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&last.value).unwrap();
        assert_eq!(rest.len(), 5_000 - (MAX_PARTS - 1));
        assert_eq!(rest.keys().next().map(String::as_str), Some("k999"));
    }
}
