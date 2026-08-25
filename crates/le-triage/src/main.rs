use clap::Parser;
use le_triage::LeTool;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "LETriage",
    version,
    long_version = LONG_VERSION,
    about = "Windows Shell Link (.lnk) parser producing LECmd-compatible CSV and JSON",
    before_help = concat!(
        "LETriage version ",
        env!("CARGO_PKG_VERSION"),
        "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
    )
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    /// Only emit LNKs whose target volume is removable
    #[arg(short = 'r', long = "removable-only")]
    removable_only: bool,

    /// Examine all files as LNK candidates (content validation still applies)
    #[arg(long)]
    all: bool,

    /// Suppress Target ID console detail (does not affect CSV/JSON)
    #[arg(long)]
    nid: bool,

    /// Suppress Extra Data console detail (does not affect CSV/JSON)
    #[arg(long)]
    neb: bool,

    /// Code page for legacy (non-Unicode) strings
    #[arg(long, default_value_t = 1252)]
    cp: u16,
}

fn main() {
    let cli = Cli::parse();

    // `--nid`/`--neb` toggle LECmd's verbose console rendering of Target IDs
    // and Extra Data blocks; LETriage produces CSV/JSON only, so they are
    // accepted for CLI parity but do not affect output.
    let _ = (cli.nid, cli.neb);

    let tool = LeTool {
        removable_only: cli.removable_only,
        all: cli.all,
        codepage: cli.cp,
    };
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
