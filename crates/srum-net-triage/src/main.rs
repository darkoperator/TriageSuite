use clap::Parser;
use srum_net_triage::aggregate::{BusinessHours, TzOffset};
use srum_net_triage::SrumNetTool;
use std::path::PathBuf;
use triage_cli::args::CommonArgs;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(clap::Args)]
struct SrumNetArgs {
    /// Local UTC offset used to bucket activity by hour-of-day and calendar
    /// day, as "+HH:MM" or "-HH:MM" (also accepts "UTC"). Default: UTC, or
    /// the value auto-detected from --system-hive if given. Always wins
    /// over --system-hive when both are passed. SrumETriage's Timestamp
    /// column is UTC, so without a real offset, hour-of-day and
    /// business-hours results reflect UTC, not the host's local time.
    #[arg(long)]
    tz: Option<TzOffset>,

    /// Optional SYSTEM hive to auto-detect the local UTC offset from
    /// (ControlSet\Control\TimeZoneInformation), instead of passing --tz
    /// by hand. Ignored if --tz is also given. Falls back to UTC with a
    /// warning if the hive can't be opened or no usable bias value is
    /// found. Reads a single static offset snapshot, not a full
    /// per-timestamp DST calculation.
    #[arg(long)]
    system_hive: Option<PathBuf>,

    /// Local business-hours window used to flag off-hours activity in the
    /// hourly fingerprint dataset, as "HH:MM-HH:MM". Supports overnight
    /// windows (e.g. "22:00-06:00").
    #[arg(long, default_value = "08:00-18:00")]
    business_hours: BusinessHours,
}

fn resolve_tz(explicit: Option<TzOffset>, system_hive: Option<&PathBuf>) -> TzOffset {
    if let Some(tz) = explicit {
        return tz;
    }
    let Some(hive_path) = system_hive else {
        return TzOffset(0);
    };
    match srum_net_triage::timezone::detect_from_system_hive(hive_path) {
        Ok(tz) => tz,
        Err(e) => {
            eprintln!(
                "Warning: could not auto-detect timezone from {}: {e}; using UTC",
                hive_path.display()
            );
            TzOffset(0)
        }
    }
}

#[derive(Parser)]
#[command(
    name = "SrumNetTriage",
    version,
    long_version = LONG_VERSION,
    about = "Rolls up SrumETriage's NetworkUsage/NetworkConnection output into per-day exfil-volume and per-hour-of-day activity tables",
    before_help = concat!("SrumNetTriage version ", env!("CARGO_PKG_VERSION"))
)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
    #[command(flatten)]
    srum_net: SrumNetArgs,
}

fn main() {
    let cli = Cli::parse();
    let tz = resolve_tz(cli.srum_net.tz, cli.srum_net.system_hive.as_ref());
    let tool = SrumNetTool {
        tz,
        business_hours: cli.srum_net.business_hours,
    };
    let code = triage_cli::runner::run(&tool, &cli.common, env!("CARGO_PKG_VERSION"));
    std::process::exit(code);
}
