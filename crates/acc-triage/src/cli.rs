//! AppCompatTriage CLI flags.

#[derive(Debug, clap::Args)]
pub struct AppCompatArgs {
    /// Skip pairing the SYSTEM hive with its `.LOG1`/`.LOG2` siblings.
    #[arg(long = "nl")]
    pub no_logs: bool,
}
