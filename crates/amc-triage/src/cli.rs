//! AmcacheTriage CLI flags.

#[derive(Debug, clap::Args)]
pub struct AmcacheArgs {
    /// Skip pairing the hive with its `.LOG1`/`.LOG2` transaction log siblings.
    #[arg(long = "nl")]
    pub no_logs: bool,
}
