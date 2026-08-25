use clap::Parser;
use evtx_triage::cli::EvtxArgs;
use evtx_triage::{maps_embed, sync, EvtxTool};
use std::collections::HashSet;
use std::sync::Mutex;
use triage_cli::args::CommonArgs;
use triage_evtx::{MapIndex, ParseOptions};

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(
    name = "EvtxTriage",
    version,
    long_version = LONG_VERSION,
    about = "Windows .evtx event-log parser producing EvtxECmd-compatible CSV and JSON",
    before_help = concat!(
        "EvtxTriage version ",
        env!("CARGO_PKG_VERSION"),
        "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
    )
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
    #[command(flatten)]
    evtx: EvtxArgs,
}

fn build_options(a: &EvtxArgs) -> Result<ParseOptions, String> {
    let mut opts = ParseOptions::new();
    opts.include_ids = a.id.iter().copied().collect();
    opts.exclude_ids = a.ex.iter().copied().collect();
    opts.channel_filter = a.ch.clone();
    opts.tdt_threshold_secs = a.tdt;
    use time::format_description::well_known::Rfc3339;
    if let Some(sd) = &a.sd {
        opts.start_date = Some(
            time::OffsetDateTime::parse(sd, &Rfc3339)
                .map_err(|_| format!("invalid --sd date: {sd}"))?,
        );
    }
    if let Some(ed) = &a.ed {
        opts.end_date = Some(
            time::OffsetDateTime::parse(ed, &Rfc3339)
                .map_err(|_| format!("invalid --ed date: {ed}"))?,
        );
    }
    Ok(opts)
}

fn main() {
    let cli = Cli::parse();

    if cli.evtx.sync {
        match sync::sync_maps() {
            Ok(n) => {
                eprintln!("EvtxTriage: synced {n} maps to resources/evtx-maps (rebuild to embed)")
            }
            Err(e) => {
                eprintln!("EvtxTriage: sync failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let opts = match build_options(&cli.evtx) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("EvtxTriage: {e}");
            std::process::exit(2);
        }
    };
    let maps = match &cli.evtx.maps {
        Some(dir) => MapIndex::load(dir),
        None => maps_embed::load_bundled(),
    };
    let tool = EvtxTool {
        maps,
        opts,
        split: cli.evtx.split,
        used_stems: Mutex::new(HashSet::new()),
    };
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
