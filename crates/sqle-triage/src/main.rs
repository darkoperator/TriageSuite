use clap::Parser;
use sqle_triage::cli::SqleArgs;
use sqle_triage::SqleTool;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "SQLETriage",
    version,
    long_version = LONG_VERSION,
    about = "SQLECmd-compatible SQLite map engine producing CSV and JSON",
    before_help = concat!(
        "SQLETriage version ",
        env!("CARGO_PKG_VERSION"),
        "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
    )
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    #[command(flatten)]
    sqle: SqleArgs,
}

fn main() {
    let cli = Cli::parse();

    if cli.sqle.sync {
        match sqle_triage::sync::sync_maps() {
            Ok(n) => {
                eprintln!("SQLETriage: synced {n} maps");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("SQLETriage: sync failed: {e}");
                std::process::exit(1);
            }
        }
    }

    let tool = SqleTool::new(cli.sqle.hunt, !cli.sqle.no_dedupe, cli.sqle.noblob);
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
