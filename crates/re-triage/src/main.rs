use clap::Parser;
use re_triage::cli::RegistryArgs;
use re_triage::RegistryTool;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "RETriage",
    version,
    long_version = LONG_VERSION,
    about = "Windows Registry parser producing RECmd-compatible CSV and JSON",
    before_help = concat!(
        "RETriage version ",
        env!("CARGO_PKG_VERSION"),
        "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
    )
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    #[command(flatten)]
    registry: RegistryArgs,
}

fn main() {
    let cli = Cli::parse();
    let tool = RegistryTool {
        no_logs: cli.registry.no_logs,
        recover_deleted: cli.registry.recover,
        batch_file: cli.registry.batch_file,
        no_plugins: cli.registry.no_plugins,
        search_key: cli.registry.search_key,
        search_value: cli.registry.search_value,
        search_data: cli.registry.search_data,
        use_regex: cli.registry.use_regex,
        literal: cli.registry.literal,
        min_size: cli.registry.min_size,
    };
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
