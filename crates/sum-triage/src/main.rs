use clap::Parser;
use sum_triage::SumTool;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "SumETriage",
    version,
    long_version = LONG_VERSION,
    about = "Windows SUM / User Access Logging parser producing SumECmd-compatible CSV and JSON",
    before_help = concat!(
        "SumETriage version ",
        env!("CARGO_PKG_VERSION"),
        "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
    )
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
}

fn main() {
    let cli = Cli::parse();
    let tool = SumTool;
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
