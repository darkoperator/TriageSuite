use anydesk_triage::AnyDeskTool;
use clap::Parser;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "AnyDeskTriage",
    version,
    long_version = LONG_VERSION,
    about = "Parses AnyDesk trace/connection logs (ad.trace/ad_svc.trace/connection_trace.txt) into one row per line [provisional format]",
    before_help = concat!("AnyDeskTriage version ", env!("CARGO_PKG_VERSION"))
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
}

fn main() {
    let cli = Cli::parse();
    let tool = AnyDeskTool;
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
