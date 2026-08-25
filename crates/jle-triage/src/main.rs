use std::path::PathBuf;

use clap::Parser;
use jle_triage::JleTool;
use triage_cli::args::CommonArgs;
use triage_jumplist::appid::AppIdTable;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "JLETriage",
    version,
    long_version = LONG_VERSION,
    about = "Windows Jump List parser producing JLECmd-compatible CSV and JSON",
    before_help = concat!(
        "JLETriage version ",
        env!("CARGO_PKG_VERSION"),
        "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
    )
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    /// Examine all files as jump-list candidates (content validation still applies)
    #[arg(long)]
    all: bool,

    /// Dump the parsed LNK detail to the console (no effect on CSV/JSON)
    #[arg(long)]
    ld: bool,

    /// Dump the parsed directory detail to the console (no effect on CSV/JSON)
    #[arg(long)]
    fd: bool,

    /// Surface numbered streams not referenced by the DestList
    #[arg(long = "withDir")]
    with_dir: bool,

    /// Path to a custom AppIDs map file ("appid"|"description" per line)
    #[arg(long = "appIds")]
    app_ids: Option<PathBuf>,

    /// Code page for legacy (non-Unicode) strings
    #[arg(long, default_value_t = 1252)]
    cp: u16,
}

fn main() {
    let cli = Cli::parse();

    // `--ld`/`--fd` toggle JLECmd's verbose console rendering of the embedded
    // LNK / directory detail; JLETriage produces CSV/JSON only, so they are
    // accepted for CLI parity but do not affect output.
    let _ = (cli.ld, cli.fd);

    // Seed the AppId table from the built-in set, then layer any user overrides.
    let mut app_ids = AppIdTable::with_builtin();
    if let Some(path) = &cli.app_ids {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                app_ids.load_user_file(&contents);
            }
            Err(e) => {
                eprintln!(
                    "warning: could not read --appIds file {}: {e}",
                    path.display()
                );
            }
        }
    }

    let tool = JleTool {
        all: cli.all,
        with_dir: cli.with_dir,
        codepage: cli.cp,
        app_ids,
    };
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
