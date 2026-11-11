//! Hold a case file open from a second process.
//!
//! Usage: `hold_case <case.duckdb> [read]`. Opens the case (for reading only
//! when `read` is given, as a command that only reads does), prints `held` on
//! its own line once the file is locked, and keeps it open until stdin
//! closes. The case-lock tests need the lock held by another *process*:
//! DuckDB's lock is the operating system's, and a second open inside one
//! process is not the conflict an analyst running two tools would hit.

use std::io::{Read, Write};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: hold_case <case.duckdb> [read]");
        std::process::exit(2);
    };
    let path = std::path::Path::new(&path);
    let opened = match args.next() {
        None => oxl_engine::Case::open(path),
        Some(mode) if mode == "read" => oxl_engine::Case::open_read_only(path),
        Some(_) => {
            eprintln!("usage: hold_case <case.duckdb> [read]");
            std::process::exit(2);
        }
    };
    let case = match opened {
        Ok(case) => case,
        Err(e) => {
            eprintln!("hold_case: {e}");
            std::process::exit(1);
        }
    };
    let mut out = std::io::stdout();
    if writeln!(out, "held").and_then(|()| out.flush()).is_err() {
        std::process::exit(1);
    }
    // Read until EOF: the parent closing stdin is the signal to let go.
    let _ = std::io::stdin().read_to_end(&mut Vec::new());
    drop(case);
}
