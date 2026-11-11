//! The `oxidelens` command line: parse arguments, call the engine, print one
//! JSON envelope. Every rule lives in the engine; nothing is re-implemented
//! here.

mod args;
mod cmd;
pub mod envelope;

use args::Cli;
use clap::error::ErrorKind;
use clap::Parser;
use oxl_engine::EngineError;
use serde_json::Value;
use std::ffi::OsString;
use std::io::Write;

/// Run the CLI over `args` (the first is the program name), writing the
/// envelope to `out`. Returns the process exit code.
pub fn run(args: impl IntoIterator<Item = OsString>, out: &mut dyn Write) -> i32 {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                let _ = write!(out, "{e}");
                return 0;
            }
            let err = EngineError::Invalid(e.to_string().trim_end().to_string());
            // Usage errors happen before a command is known, and the flag
            // choosing the format may not have parsed: print compact.
            emit(out, &envelope::err("usage", &err), true);
            return envelope::exit_code(&err);
        }
    };
    let name = cli.command.name();
    match cmd::execute(&cli) {
        Ok(reply) => {
            emit(
                out,
                &envelope::ok(name, reply.data, reply.truncated),
                cli.json,
            );
            0
        }
        Err(e) => {
            emit(out, &envelope::err(name, &e), cli.json);
            envelope::exit_code(&e)
        }
    }
}

fn emit(out: &mut dyn Write, doc: &Value, compact: bool) {
    let text = if compact {
        doc.to_string()
    } else {
        serde_json::to_string_pretty(doc).unwrap_or_else(|_| doc.to_string())
    };
    // A closed pipe is the reader's choice; there is nobody left to tell.
    let _ = writeln!(out, "{text}");
}
