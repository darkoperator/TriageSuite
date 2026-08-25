use clap::Parser;
use lol_triage::{refdata::LolRefs, update_refs, LolTool};
use std::path::PathBuf;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(clap::Args)]
struct LolArgs {
    /// Directory containing loldrivers_refs.json and lolrmm_refs.json.
    /// For a normal scan: where to read from (default: the reference data
    /// embedded in this binary at build time). With --update-refs: where to
    /// write the freshly fetched snapshots (default: crates/lol-triage/refdata
    /// in this source checkout — a maintainer-only workflow).
    #[arg(long)]
    refs: Option<PathBuf>,

    /// Fetch the latest LOLDrivers/LOLRMM data and write reduced reference
    /// snapshots to --refs (or the source checkout's refdata/ by default),
    /// then exit. Requires network access. Cannot be combined with
    /// -d/-f/--csv/--json.
    #[arg(long)]
    update_refs: bool,
}

#[derive(Parser)]
#[command(
    name = "LolTriage",
    version,
    long_version = LONG_VERSION,
    about = "Cross-references AmcacheTriage/AppCompatTriage/MFTriage/PETriage/RBTriage output against LOLDrivers and LOLRMM reference lists",
    before_help = concat!("LolTriage version ", env!("CARGO_PKG_VERSION"))
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
    #[command(flatten)]
    lol: LolArgs,
}

fn main() {
    let cli = Cli::parse();

    if cli.lol.update_refs {
        if cli.common.directory.is_some()
            || !cli.common.files.is_empty()
            || cli.common.csv.is_some()
            || cli.common.json.is_some()
        {
            eprintln!("Error: --update-refs cannot be combined with -d/-f/--csv/--json");
            std::process::exit(2);
        }
        let out_dir = cli
            .lol
            .refs
            .clone()
            .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/refdata")));
        match update_refs::run(&out_dir) {
            Ok((drivers, rmm)) => {
                eprintln!(
                    "wrote {drivers} LOLDrivers entries, {rmm} LOLRMM entries to {}",
                    out_dir.display()
                );
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(2);
            }
        }
    }

    let refs = match &cli.lol.refs {
        Some(dir) => LolRefs::load(dir)
            .map_err(|e| format!("cannot load reference data from {}: {e}", dir.display())),
        None => LolRefs::embedded()
            .map_err(|e| format!("cannot load the reference data embedded in this binary: {e}")),
    };
    let refs = match refs {
        Ok(r) => r,
        Err(message) => {
            eprintln!("Error: {message}");
            std::process::exit(2);
        }
    };
    let tool = LolTool { refs };
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
