//! WxTTriage tool-specific flags. WxTCmd's only output-shaping option is
//! `--dt` (datetime format); the suite renders ISO-8601 via WinTimestamp, so
//! `--dt` is accepted for CLI parity but does not change output.

use clap::Args;

#[derive(Args, Debug)]
pub struct WxtArgs {
    /// Datetime format (accepted for WxTCmd parity; output is ISO-8601).
    #[arg(long = "dt", default_value = "yyyy-MM-dd HH:mm:ss")]
    pub dt: String,
}
