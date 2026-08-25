use clap::Parser;
use rb_triage::RbTool;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "RBTriage",
    version,
    long_version = LONG_VERSION,
    about = "Windows Recycle Bin ($I/INFO2) parser producing RBCmd-compatible CSV and JSON",
    before_help = concat!(
        "RBTriage version ",
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
    let code = triage_cli::runner::run(&RbTool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
