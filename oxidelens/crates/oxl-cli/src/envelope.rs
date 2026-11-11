//! The result envelope every command prints, and the wrapper that marks
//! evidence text as data.
//!
//! An AI agent reads this output, and evidence can contain text aimed at it.
//! So anything that came from evidence or a loose file travels inside an
//! `untrusted` string whose boundary the evidence's author cannot predict.

use oxl_engine::EngineError;
use serde_json::{json, Value};

/// 16 hex characters from the OS random source, new on every call.
pub fn boundary() -> String {
    let mut bytes = [0u8; 8];
    // An unpredictable boundary is the whole point; a clock or pid fallback
    // would be guessable, so failure here stops the process.
    getrandom::fill(&mut bytes).expect("the operating system's random source is unavailable");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `value` as compact JSON between boundary-tagged markers.
pub fn untrusted(value: &Value, boundary: &str) -> String {
    format!("<untrusted_data boundary={boundary}>\n{value}\n</untrusted_data boundary={boundary}>")
}

/// Wrap `value` with a fresh boundary.
pub fn wrap(value: &Value) -> String {
    untrusted(value, &boundary())
}

pub fn ok(command: &str, data: Value, truncated: bool) -> Value {
    json!({"ok": true, "command": command, "data": data, "truncated": truncated})
}

pub fn err(command: &str, e: &EngineError) -> Value {
    json!({"ok": false, "command": command, "error": {"code": e.code(), "message": e.to_string()}})
}

pub fn exit_code(e: &EngineError) -> i32 {
    match e {
        EngineError::Invalid(_) => 2,
        EngineError::NotFound(_) => 3,
        EngineError::Refused(_) => 4,
        EngineError::CaseLocked(_) => 5,
        EngineError::NeedsUpgrade { .. } => 6,
        EngineError::Db(_) | EngineError::Io(_) => 10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_is_16_hex_and_fresh() {
        let (a, b) = (boundary(), boundary());
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn wrapper_shape() {
        assert_eq!(
            untrusted(&json!({"a": 1}), "B"),
            "<untrusted_data boundary=B>\n{\"a\":1}\n</untrusted_data boundary=B>"
        );
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code(&EngineError::Refused("x".into())), 4);
        assert_eq!(exit_code(&EngineError::NotFound("x".into())), 3);
    }
}
