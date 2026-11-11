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

/// Every command path clap knows, nested ones as `parent child`, a command
/// with subcommands excluded (only the leaves run). The contract tests hold
/// this to `capabilities`, so a command cannot be added and left undescribed.
pub fn command_paths() -> Vec<String> {
    fn walk(cmd: &clap::Command, prefix: &str, out: &mut Vec<String>) {
        for sub in cmd.get_subcommands() {
            let path = if prefix.is_empty() {
                sub.get_name().to_string()
            } else {
                format!("{prefix} {}", sub.get_name())
            };
            if sub.has_subcommands() {
                walk(sub, &path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&<Cli as clap::CommandFactory>::command(), "", &mut out);
    out
}

/// Run the CLI over `args` (the first is the program name), writing the
/// envelope to `out`. Returns the process exit code.
pub fn run(args: impl IntoIterator<Item = OsString>, out: &mut dyn Write) -> i32 {
    let args: Vec<OsString> = args.into_iter().collect();
    // Help and usage errors end parsing before `--json` is known, so the
    // output format is read from the raw arguments for them.
    let compact = args.iter().any(|a| a == "--json");
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(e) => {
            let help_text = e.to_string();
            match e.kind() {
                ErrorKind::DisplayHelp => {
                    // Tool-generated, but it echoes argv[0] and argument
                    // names, so it travels inside the wrapper like the rest.
                    let data = serde_json::json!({
                        "untrusted": envelope::wrap(&serde_json::json!({"help": help_text})),
                    });
                    emit(out, &envelope::ok("help", data, false), compact);
                    return 0;
                }
                ErrorKind::DisplayVersion => {
                    let data = serde_json::json!({"version": env!("CARGO_PKG_VERSION")});
                    emit(out, &envelope::ok("version", data, false), compact);
                    return 0;
                }
                _ => {}
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
