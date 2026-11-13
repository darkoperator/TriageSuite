//! The events the app sends the window, and where they go.
//!
//! Each payload is one of the structs below and nothing else: no cell text,
//! relation or host name from the case ever rides on an event. The window
//! re-reads what it shows when one arrives.

use serde::Serialize;

/// After every successful write: `CaseChanged`.
pub const CASE_CHANGED: &str = "case-changed";
/// A long write (a load) began: `JobStarted`.
pub const JOB_STARTED: &str = "job-started";
/// That write ended: `JobFinished`.
pub const JOB_FINISHED: &str = "job-finished";
/// The settings file could not be read and was set aside:
/// `settings::DamagedSettings`. Sent once, as the app starts, which may be
/// before the window listens: `settings_get` reports the same until the
/// settings are next saved.
pub const SETTINGS_DAMAGED: &str = "settings-damaged";

/// What changed in the case, and the app's change counter after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
pub struct CaseChanged {
    /// What the write touched (`sources`, `marks`, `views`, `settings`...),
    /// so a screen re-reads only what it shows.
    pub kinds: Vec<String>,
    /// Rises with every open, close and successful write; an answer read
    /// before a higher generation is stale.
    pub generation: u64,
}

/// A job's kind: `load_run`, `load_file` or `export`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
pub struct JobStarted {
    pub kind: String,
    /// What the status bar says: the file or folder name being loaded.
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
pub struct JobFinished {
    pub kind: String,
    pub ok: bool,
    /// The error's message when the job failed.
    pub message: Option<String>,
}

/// Where events go: the window in the app, a recorder in tests.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

/// Send `payload` as `event` through `sink`. A payload that cannot be
/// serialised is a bug in this module's structs; it is dropped rather than
/// sent half-made.
pub fn send(sink: &dyn EventSink, event: &str, payload: &impl Serialize) {
    if let Ok(value) = serde_json::to_value(payload) {
        sink.emit(event, value);
    }
}

/// Events to every window of the running app.
pub struct TauriSink<R: tauri::Runtime>(pub tauri::AppHandle<R>);

impl<R: tauri::Runtime> EventSink for TauriSink<R> {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        use tauri::Emitter;
        // A window that is gone cannot be told; the case is unaffected.
        let _ = self.0.emit(event, payload);
    }
}

/// Events to nowhere, for an `AppState` nothing listens to.
pub struct NoSink;

impl EventSink for NoSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}
}
