use amc_triage::cli::AmcacheArgs;
use amc_triage::AmcacheTool;
use clap::Parser;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "AmcacheTriage",
    version,
    long_version = LONG_VERSION,
    about = "Windows Amcache.hve parser producing AmcacheParser-compatible CSV and JSON",
    before_help = concat!("AmcacheTriage version ", env!("CARGO_PKG_VERSION"))
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
    #[command(flatten)]
    amcache: AmcacheArgs,
}

fn main() {
    let cli = Cli::parse();
    let tool = AmcacheTool {
        no_logs: cli.amcache.no_logs,
    };
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
