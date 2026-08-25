use clap::Parser;
use pe_triage::PeTool;
use triage_cli::args::CommonArgs;
use triage_cli::runner::RunOptions;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "PETriage",
    version,
    long_version = LONG_VERSION,
    about = "Windows Prefetch parser producing PECmd-compatible CSV and JSON",
    before_help = concat!(
        "PETriage version ",
        env!("CARGO_PKG_VERSION"),
        "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
    )
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    /// Comma-separated keywords to flag (PECmd semantics; temp and tmp are always included)
    #[arg(short = 'k', long, value_delimiter = ',')]
    keywords: Vec<String>,

    /// Deduplicate identical prefetch files by SHA-1 (first discovered wins)
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    dedupe: bool,
}

fn main() {
    let cli = Cli::parse();

    // PECmd's default keywords are always active; user keywords add to them.
    let mut keywords: Vec<String> = vec!["temp".into(), "tmp".into()];
    for k in &cli.keywords {
        let k = k.trim().to_lowercase();
        if !k.is_empty() && !keywords.contains(&k) {
            keywords.push(k);
        }
    }

    let tool = PeTool { keywords };
    let code = triage_cli::runner::run_with_options(
        &tool,
        &cli.common,
        env!("CARGO_PKG_VERSION"),
        RunOptions { dedupe: cli.dedupe },
    );
    std::process::exit(code);
}
