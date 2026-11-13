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
            // clap's text opens with its own `error: `; the envelope says
            // `invalid:` already, so one prefix is enough.
            let text = help_text.trim_end();
            let err =
                EngineError::Invalid(text.strip_prefix("error: ").unwrap_or(text).to_string());
            // The flag choosing the format may not have parsed, so it is read
            // from the raw arguments, as for help.
            emit(out, &envelope::err("usage", &err), compact);
            return envelope::exit_code(&err);
        }
    };
    let name = cli.command.name();
    // A panic anywhere below still ends in one envelope and a code, never a
    // bare crash that leaves stdout empty for the reader.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(debug_assertions)]
        if std::env::var_os("OXIDELENS_TEST_PANIC").is_some() {
            panic!("OXIDELENS_TEST_PANIC is set");
        }
        cmd::execute(&cli)
    }));
    let result = match result {
        Ok(result) => result,
        Err(payload) => {
            let what = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "no message".to_string());
            emit(out, &envelope::panicked(name, &what), cli.json);
            return envelope::PANIC_EXIT;
        }
    };
    match result {
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
