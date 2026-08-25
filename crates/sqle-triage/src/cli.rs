//! SQLETriage flags. Known database names are selected by default; `--hunt`
//! explicitly enables content inspection of every discovered file.

use clap::Args;

#[derive(Args, Debug)]
pub struct SqleArgs {
    /// Inspect every file by content instead of limiting discovery to known
    /// SQLite database filename patterns.
    #[arg(long)]
    pub hunt: bool,

    /// Disable dedupe (default: skip databases whose SHA-1 was already seen).
    #[arg(long = "no-dedupe")]
    pub no_dedupe: bool,

    /// Suppress BLOB values in query output.
    #[arg(long = "noblob")]
    pub noblob: bool,

    /// Refresh the bundled map corpus from upstream GitHub, then exit.
    #[arg(long = "sync")]
    pub sync: bool,
}
