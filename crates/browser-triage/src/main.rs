use browser_triage::BrowserTool;
use clap::Parser;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "BrowserTriage",
    version,
    long_version = LONG_VERSION,
    about = "Parses Chromium and Firefox browser artifacts (history, downloads, cookies, autofill, bookmarks, login metadata, keyword searches, extensions) into typed CSV/JSON plus a derived timeline",
    before_help = concat!("BrowserTriage version ", env!("CARGO_PKG_VERSION"))
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    /// Skip the derived _Timeline dataset (it can be several times larger than
    /// the typed datasets on a busy profile)
    #[arg(long)]
    no_timeline: bool,
}

fn main() {
    let cli = Cli::parse();
    let tool = BrowserTool::new(cli.no_timeline);
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
