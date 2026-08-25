//! Windows `.evtx` event-log parser + Zimmerman maps engine, vendored from the
//! standalone EVTXTriage project. `EventRecord` derives `Serialize` (EvtxECmd
//! column names) ready for the TriageSuite OutputRouter. CLI/discovery/output
//! and the maps corpus live in the `evtx-triage` binary crate.

pub mod error;
pub mod maps;
pub mod parser;
pub mod record;

pub use error::{EvtxTriageError, Result};
pub use maps::MapIndex;
pub use parser::{parse_evtx_file, visit_evtx_file, ParseOptions, VisitError};
pub use record::EventRecord;
