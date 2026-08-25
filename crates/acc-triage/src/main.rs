use acc_triage::cli::AppCompatArgs;
use acc_triage::AppCompatTool;
use clap::Parser;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "AppCompatTriage",
    version,
    long_version = LONG_VERSION,
    about = "Windows AppCompatCache/ShimCache parser producing AppCompatCacheParser-compatible CSV and JSON",
    before_help = concat!("AppCompatTriage version ", env!("CARGO_PKG_VERSION"))
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
    #[command(flatten)]
    appcompat: AppCompatArgs,
}

fn main() {
    let cli = Cli::parse();
    let tool = AppCompatTool {
        no_logs: cli.appcompat.no_logs,
    };
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
