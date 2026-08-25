//! SBETriage CLI flags. Mirrors `re-triage/src/cli.rs` / `triage-cli` pattern.

/// SBETriage-specific command-line flags.
#[derive(Debug, clap::Args)]
pub struct ShellbagArgs {
    /// Skip pairing hives with their `.LOG1`/`.LOG2` transaction log siblings
    /// (allow dirty hives without logs).
    #[arg(long = "nl")]
    pub no_logs: bool,

    /// Deduplicate shellbag records (accepted for SBECmd CLI parity; no effect
    /// on output in the current implementation).
    #[arg(long = "dedupe")]
    pub dedupe: bool,

    /// DateTime format string for display (accepted for SBECmd CLI parity; the
    /// suite always emits ISO-8601 timestamps regardless of this flag).
    #[arg(long = "dt")]
    pub dt: Option<String>,
}
