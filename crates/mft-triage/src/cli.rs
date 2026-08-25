use std::path::PathBuf;

/// MFTriage-specific flags (alongside the shared CommonArgs).
#[derive(Debug, clap::Args)]
pub struct MftArgs {
    /// Include DOS (8.3) short names in $MFT output
    #[arg(long)]
    pub sn: bool,

    /// Include all $FILE_NAME (0x30) timestamps, not only when they differ from 0x10
    #[arg(long)]
    pub at: bool,

    /// Also emit the $MFT file-listing dataset
    #[arg(long)]
    pub fl: bool,

    /// $MFT used to resolve $J parent paths (default: the sibling $MFT next to $J)
    #[arg(long)]
    pub mft: Option<PathBuf>,
}
