use clap::Parser;
use stub_triage::StubTool;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "StubTriage",
    version,
    long_version = LONG_VERSION,
    about = "TriageSuite pipeline verification tool (not a forensic parser)",
    before_help = concat!(
        "StubTriage version ",
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
    let code = triage_cli::runner::run(&StubTool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
