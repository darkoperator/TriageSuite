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
    // An unpredictable boundary is the whole point; a clock or pid fallback
    // would be guessable, so failure here stops the process.
    try_boundary().expect("the operating system's random source is unavailable")
}

/// `boundary`, or `None` when the operating system's random source fails.
/// A debug build fails it on purpose while `OXIDELENS_TEST_BOUNDARY_FAIL` is
/// set, so the tests can reach that path.
fn try_boundary() -> Option<String> {
    #[cfg(debug_assertions)]
    if std::env::var_os("OXIDELENS_TEST_BOUNDARY_FAIL").is_some() {
        return None;
    }
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
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
    // The message can quote evidence (a column name, a path, a cell value) or
    // the caller's own text, so it is data, not tool output.
    json!({"ok": false, "command": command, "error": {
        "code": e.code(),
        "untrusted": wrap(&json!({"message": e.to_string()})),
    }})
}

/// The reply to a command that panicked: the code `engine`, like any other
/// fault of the program, and the panic's text inside the wrapper.
///
/// It runs after the command's panic was caught, where a second panic would
/// end the process with nothing printed, so it never panics: when the random
/// source fails it wraps a fixed message in `FALLBACK_BOUNDARY`. The panic's
/// text is left out of that reply, because a boundary anyone can predict
/// must not enclose text that may quote evidence.
pub fn panicked(command: &str, what: &str) -> Value {
    let untrusted = match try_boundary() {
        Some(b) => untrusted(
            &json!({"message": format!("engine: the command stopped on a fault: {what}")}),
            &b,
        ),
        None => untrusted(
            &json!({"message": "engine: the command stopped on a fault, and the random \
                                source the reply's wrapper needs failed too"}),
            FALLBACK_BOUNDARY,
        ),
    };
    json!({"ok": false, "command": command, "error": {
        "code": "engine",
        "untrusted": untrusted,
    }})
}

/// The boundary of the panic reply when the random source cannot give one.
pub const FALLBACK_BOUNDARY: &str = "oxidelens-fixed-fallback";

/// The exit code of a panic: that of the `engine` code.
pub const PANIC_EXIT: i32 = 10;

pub fn exit_code(e: &EngineError) -> i32 {
    match e {
        EngineError::Invalid(_) => 2,
        EngineError::NotFound(_) => 3,
        EngineError::Refused(_) => 4,
        EngineError::CaseLocked(_) => 5,
        EngineError::NeedsUpgrade { .. } => 6,
        EngineError::Db(_) | EngineError::Io(_) => 10,
        EngineError::Internal(_) => 11,
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
    fn error_message_is_wrapped_and_code_is_not() {
        let v = err("x", &EngineError::NotFound("secret-path".into()));
        assert_eq!(v["error"]["code"], "not_found");
        assert!(v["error"].get("message").is_none());
        let text = v["error"]["untrusted"].as_str().unwrap();
        assert!(text.starts_with("<untrusted_data boundary="));
        assert!(text.contains("secret-path"));
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code(&EngineError::Refused("x".into())), 4);
        assert_eq!(exit_code(&EngineError::NotFound("x".into())), 3);
        let internal = EngineError::Internal("x".into());
        assert_eq!((exit_code(&internal), internal.code()), (11, "internal"));
    }
}
