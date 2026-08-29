use std::path::PathBuf;

/// EvtxTriage-specific flags (alongside the shared CommonArgs).
#[derive(Debug, clap::Args)]
pub struct EvtxArgs {
    /// Maps directory override (default: the bundled corpus)
    #[arg(long)]
    pub maps: Option<PathBuf>,

    /// Also write one output file per source .evtx, named after the source file,
    /// in addition to the combined aggregate output
    #[arg(long)]
    pub split: bool,

    /// Refresh the bundled maps corpus from GitHub, then exit
    #[arg(long)]
    pub sync: bool,

    /// Include only these Event IDs (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub id: Vec<u32>,

    /// Exclude these Event IDs (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub ex: Vec<u32>,

    /// Include only channels containing this substring (case-insensitive)
    #[arg(long)]
    pub ch: Option<String>,

    /// Start datetime UTC ISO 8601 — skip events before this
    #[arg(long)]
    pub sd: Option<String>,

    /// End datetime UTC ISO 8601 — skip events after this
    #[arg(long)]
    pub ed: Option<String>,

    /// Time-discrepancy threshold in seconds. Accepted for compatibility but
    /// currently unused: the TimeDiscrepancy columns were dropped for EvtxECmd
    /// CSV parity.
    #[arg(long, default_value = "1.0")]
    pub tdt: f64,
}
