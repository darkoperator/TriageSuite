use clap::Parser;
use srume_triage::cli::SrumArgs;
use srume_triage::SrumeTool;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "SrumETriage",
    version,
    long_version = LONG_VERSION,
    about = "Windows SRUM (SRUDB.dat) parser producing SrumECmd-compatible CSV and JSON",
    before_help = concat!(
        "SrumETriage version ",
        env!("CARGO_PKG_VERSION"),
        "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
    )
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    #[command(flatten)]
    srum: SrumArgs,
}

fn main() {
    let cli = Cli::parse();
    let tool = SrumeTool {
        software: cli.srum.software,
    };
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
