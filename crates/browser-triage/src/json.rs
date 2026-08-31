//! Total accessors over `serde_json::Value`.
//!
//! Same principle as the SQL accessors: reading a field that is missing, null
//! or of an unexpected type must never cost the record. Every function here
//! returns a usable value and lets the caller decide whether the absence is
//! worth a `Notes` entry.

use serde_json::Value;

/// A string field. A JSON number or bool renders to its text rather than
/// vanishing, because Chromium stores several numeric fields as strings and
/// some builds disagree about which.
pub fn text(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// An integer field, accepting the decimal *strings* Chromium uses for its
/// large timestamps (`date_added` is WebKit microseconds stored as a string,
/// because JSON numbers cannot hold it precisely in every implementation).
pub fn int(value: &Value, key: &str) -> Option<i64> {
    match value.get(key) {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

/// A boolean field rendered as `True`/`False`, or empty when absent — never
/// defaulting an unknown to `False`.
pub fn bool_str(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::Bool(b)) => if *b { "True" } else { "False" }.to_string(),
        Some(Value::Number(n)) => match n.as_i64() {
            Some(0) => "False".to_string(),
            Some(_) => "True".to_string(),
            None => String::new(),
        },
        _ => String::new(),
    }
}

/// An array field, or an empty slice.
pub fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).map_or(&[], |v| v)
}

/// Join a string array with `|`, skipping non-strings. Used for permission
/// lists.
pub fn joined_strings(value: &Value, key: &str) -> String {
    array(value, key)
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_is_total_over_missing_null_and_wrong_types() {
        let v = json!({"s": "x", "n": 5, "b": true, "null": null, "arr": [1]});
        assert_eq!(text(&v, "s"), "x");
        assert_eq!(text(&v, "n"), "5");
        assert_eq!(text(&v, "b"), "true");
        assert_eq!(text(&v, "null"), "");
        assert_eq!(text(&v, "arr"), "");
        assert_eq!(text(&v, "absent"), "");
    }

    /// Chromium writes WebKit microseconds as a decimal string, so a
    /// number-only accessor would silently lose every bookmark timestamp.
    #[test]
    fn int_accepts_the_decimal_strings_chromium_uses_for_timestamps() {
        let v = json!({"date_added": "13344473600000000", "n": 7, "bad": "abc"});
        assert_eq!(int(&v, "date_added"), Some(13_344_473_600_000_000));
        assert_eq!(int(&v, "n"), Some(7));
        assert_eq!(int(&v, "bad"), None);
        assert_eq!(int(&v, "absent"), None);
    }

    #[test]
    fn bool_str_distinguishes_absent_from_false() {
        let v = json!({"t": true, "f": false, "zero": 0, "one": 1});
        assert_eq!(bool_str(&v, "t"), "True");
        assert_eq!(bool_str(&v, "f"), "False");
        assert_eq!(bool_str(&v, "zero"), "False");
        assert_eq!(bool_str(&v, "one"), "True");
        assert_eq!(bool_str(&v, "absent"), "");
    }

    #[test]
    fn arrays_and_joins_tolerate_absence_and_mixed_types() {
        let v = json!({"perms": ["tabs", 1, "cookies"], "empty": [], "s": "x"});
        assert_eq!(joined_strings(&v, "perms"), "tabs|cookies");
        assert_eq!(joined_strings(&v, "empty"), "");
        assert_eq!(joined_strings(&v, "s"), "");
        assert_eq!(array(&v, "absent").len(), 0);
    }
}
