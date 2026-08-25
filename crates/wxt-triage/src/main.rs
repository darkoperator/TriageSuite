use clap::Parser;
use triage_cli::args::CommonArgs;
use wxt_triage::cli::WxtArgs;
use wxt_triage::WxtTool;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "WxTTriage",
    version,
    long_version = LONG_VERSION,
    about = "Windows Timeline (ActivitiesCache.db) parser producing WxTCmd-compatible CSV and JSON",
    before_help = concat!(
        "WxTTriage version ",
        env!("CARGO_PKG_VERSION"),
        "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
    )
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    #[command(flatten)]
    wxt: WxtArgs,
}

fn main() {
    let cli = Cli::parse();
    // `--dt` accepted for WxTCmd parity; output is ISO-8601 via WinTimestamp.
    let _ = cli.wxt.dt;
    let tool = WxtTool;
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
