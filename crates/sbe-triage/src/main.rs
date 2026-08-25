use clap::Parser;
use sbe_triage::cli::ShellbagArgs;
use sbe_triage::ShellbagTool;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "SBETriage",
    version,
    long_version = LONG_VERSION,
    about = "Windows shellbags parser producing SBECmd-compatible CSV and JSON",
    before_help = concat!(
        "SBETriage version ",
        env!("CARGO_PKG_VERSION"),
        "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
    )
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    #[command(flatten)]
    shellbag: ShellbagArgs,
}

fn main() {
    let cli = Cli::parse();

    // `--dedupe` and `--dt` are accepted for SBECmd CLI parity but do not
    // affect output in the current implementation.
    let _ = (cli.shellbag.dedupe, cli.shellbag.dt);

    let tool = ShellbagTool {
        no_logs: cli.shellbag.no_logs,
    };
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
