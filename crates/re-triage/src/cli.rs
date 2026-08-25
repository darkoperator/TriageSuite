//! RETriage CLI flags. Mirrors `jle-triage/src/main.rs` / `triage-cli` pattern.

use std::path::PathBuf;

/// RETriage-specific command-line flags.
#[derive(Debug, clap::Args)]
pub struct RegistryArgs {
    /// Custom `.reb` batch file (default: embedded DFIRBatch.reb)
    #[arg(long = "bn")]
    pub batch_file: Option<PathBuf>,

    /// Skip pairing hives with their `.LOG1`/`.LOG2` transaction log siblings
    #[arg(long = "nl")]
    pub no_logs: bool,

    /// Recover deleted registry records (slower; requires full hive parse).
    /// Matches RECmd default (--recover default is true in Program.cs).
    /// Pass --no-recover to skip deleted record recovery.
    #[arg(long = "recover", default_value_t = true, action = clap::ArgAction::Set)]
    pub recover: bool,

    // ── Search flags (mutually exclusive with batch mode when present) ────────
    /// Search for <term> in key names (--sk)
    #[arg(long = "sk")]
    pub search_key: Option<String>,

    /// Search for <term> in value names (--sv)
    #[arg(long = "sv")]
    pub search_value: Option<String>,

    /// Search for <term> in value data (--sd)
    #[arg(long = "sd")]
    pub search_data: Option<String>,

    /// Treat search terms as regular expressions
    #[arg(long = "regex")]
    pub use_regex: bool,

    /// Force literal (substring) matching even if --regex is set
    #[arg(long = "literal")]
    pub literal: bool,

    /// Minimum value data size in bytes (for --sd)
    #[arg(long = "minSize", default_value_t = 0)]
    pub min_size: usize,

    /// Disable all plugin dispatch (RECmd global DisablePlugin). When set,
    /// every key takes the default dump path regardless of plugin matches.
    /// Use this to reproduce RECmd output without plugins, or to compare
    /// against the `*__batch_noplugins.csv` fixtures (engine regression guard).
    #[arg(long = "no-plugins")]
    pub no_plugins: bool,
}
