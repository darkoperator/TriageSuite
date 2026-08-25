//! SrumETriage flags. `--software` overrides the auto-located SOFTWARE hive
//! used for SID->username resolution.

use clap::Args;

#[derive(Args, Debug)]
pub struct SrumArgs {
    /// Path to a SOFTWARE hive for SID->username resolution (overrides the
    /// auto-located hive in the SRUDB's capture subtree).
    #[arg(long = "software")]
    pub software: Option<std::path::PathBuf>,
}
