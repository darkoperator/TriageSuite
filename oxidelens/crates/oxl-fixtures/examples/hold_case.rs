//! Hold a case file open from a second process.
//!
//! Usage: `hold_case <case.duckdb>`. Opens the case, prints `held` on its own
//! line once the file is locked, and keeps it open until stdin closes. The
//! case-lock test needs the lock held by another *process*: DuckDB's lock is
//! the operating system's, and a second open inside one process is not the
//! conflict an analyst running two tools would hit.

use std::io::{Read, Write};

fn main() {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: hold_case <case.duckdb>");
        std::process::exit(2);
    };
    let case = match oxl_engine::Case::open(std::path::Path::new(&path)) {
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
