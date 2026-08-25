use clap::Parser;
use mft_triage::cli::MftArgs;
use mft_triage::MftTool;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "MFTriage",
    version,
    long_version = LONG_VERSION,
    about = "NTFS $MFT / $J / $Boot parser producing MFTECmd-compatible CSV and JSON",
    before_help = concat!(
        "MFTriage version ",
        env!("CARGO_PKG_VERSION"),
        "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
    )
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    #[command(flatten)]
    mft: MftArgs,
}

fn main() {
    let cli = Cli::parse();
    let tool = MftTool {
        sn: cli.mft.sn,
        at: cli.mft.at,
        fl: cli.mft.fl,
        mft: cli.mft.mft,
    };
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
