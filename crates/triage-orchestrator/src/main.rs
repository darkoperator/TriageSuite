use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use triage_core::error::RunExit;
use triage_orchestrator::capture::HostCapture;
use triage_orchestrator::execute::{self, OutputOpts, ToolRunResult};
use triage_orchestrator::external::{self, ExternalConfig, ResolvedConfig};
use triage_orchestrator::file_name_lossy;
use triage_orchestrator::input::{self, PrepareOptions, EXTRACTED_DIR};
use triage_orchestrator::manifest::{self, HostEntry, Manifest};
use triage_orchestrator::progress_ui::{self, ProgressUi};
use triage_orchestrator::registry::{self, ToolEntry, ToolOptions};

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>"
);

#[derive(Parser)]
#[command(name = "TriageSuite", version, long_version = LONG_VERSION,
    about = "Run every TriageSuite parser over a Velociraptor capture")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Detect a capture and run all applicable parsers over it
    Run(RunArgs),
}

#[derive(Args)]
struct RunArgs {
    /// Capture: a Velociraptor collection, a folder of collections, a .zip
    /// collection, or a folder of .zip captures
    capture: PathBuf,
    /// Output root
    #[arg(long)]
    out: PathBuf,
    /// Write CSV output (default on if neither --csv nor --json given)
    #[arg(long)]
    csv: bool,
    /// Write NDJSON output
    #[arg(long)]
    json: bool,
    /// Only run these tools (comma-separated keys, e.g. pe,evtx,mft)
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Skip these tools (comma-separated keys)
    #[arg(long, value_delimiter = ',')]
    skip: Vec<String>,
    /// Replace existing output files
    #[arg(long)]
    overwrite: bool,
    /// Max tools to run concurrently per host (default: CPU count)
    #[arg(long)]
    jobs: Option<usize>,
    /// Max memory-heavy tools to run concurrently (default: 1)
    #[arg(long, default_value_t = 1)]
    heavy_jobs: usize,
    /// Disable progress bars (colored status markers are kept on a TTY)
    #[arg(long)]
    no_progress: bool,
    /// Inspect every file for SQLite content (requires --only sqle or a list containing sqle)
    #[arg(long, requires = "only")]
    hunt: bool,
    /// Skip BrowserTriage's derived _Timeline dataset, which is routinely
    /// larger than all of its typed datasets combined
    #[arg(long)]
    no_timeline: bool,
    /// Optional TOML config for hayabusa/takajo (see docs/tools/TriageSuite.md, "Config and profiles")
    #[arg(long)]
    config: Option<PathBuf>,
    /// Named profile to apply from --config (must exist under [profiles.<name>])
    #[arg(long, requires = "config")]
    profile: Option<String>,
}

fn main() {
    let exit = match Cli::parse().command {
        Command::Run(args) => run(args),
    };
    std::process::exit(exit.code());
}

/// Print `Error: <e>` to stderr and exit with `code`.
fn die(e: impl std::fmt::Display, code: RunExit) -> ! {
    eprintln!("Error: {e}");
    std::process::exit(code.code());
}

/// Everything `run_host` needs that is the same for every host.
struct RunContext<'a> {
    tools: &'a [ToolEntry],
    tool_keys: Vec<String>,
    out: &'a Path,
    out_opts: OutputOpts,
    external: ResolvedConfig,
    jobs: usize,
    heavy_jobs: usize,
    ui: ProgressUi,
}

/// Run-wide artifact counts that decide the final exit status.
#[derive(Default)]
struct Totals {
    successful: u64,
    failed: u64,
    terminal: Option<RunExit>,
}

impl Totals {
    fn absorb(&mut self, r: &ToolRunResult) {
        self.successful += r.parsed;
        self.failed += r.failed;
        if r.error.is_some() {
            self.failed += 1;
        }
        if matches!(r.exit, Some(RunExit::OutputFailure)) {
            self.terminal = Some(RunExit::OutputFailure);
        }
    }

    fn exit(&self) -> RunExit {
        execute::aggregate_exit(self.successful, self.failed, self.terminal)
    }
}

fn run(args: RunArgs) -> RunExit {
    progress_ui::print_banner();

    // Cheap validation first: a typo'd --only or a malformed --config must
    // fail in milliseconds, not after extracting hundreds of gigabytes.
    let tool_options = ToolOptions {
        hunt: args.hunt,
        no_timeline: args.no_timeline,
    };
    let tools = select_tools(&args.only, &args.skip, tool_options);
    let external =
        load_external_config(args.config.as_deref(), args.profile.as_deref(), &args.skip);
    let jobs = args.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });

    // Default to CSV when neither flag given.
    let (want_csv, want_json) = if !args.csv && !args.json {
        (true, false)
    } else {
        (args.csv, args.json)
    };
    let run_id = manifest::run_id();
    let started = manifest::now_iso();
    let ctx = RunContext {
        tools: &tools,
        tool_keys: tools.iter().map(|t| t.key.to_string()).collect(),
        out: &args.out,
        out_opts: OutputOpts {
            csv_root: want_csv.then(|| args.out.clone()),
            json_root: want_json.then(|| args.out.clone()),
            overwrite: args.overwrite,
            run_id: run_id.clone(),
            tools: tool_options,
        },
        external,
        jobs,
        heavy_jobs: args.heavy_jobs,
        ui: ProgressUi::new(args.no_progress),
    };

    let prep_opts = PrepareOptions {
        reuse_existing: !args.overwrite,
        ..Default::default()
    };
    let prepared = input::prepare(&args.capture, &args.out, &prep_opts, &ctx.ui)
        .unwrap_or_else(|e| die(e, RunExit::InputMissing));

    let mut totals = Totals::default();
    let hosts: Vec<HostEntry> = prepared
        .hosts
        .iter()
        .map(|host| run_host(&ctx, host, &mut totals))
        .collect();

    let exit = totals.exit();
    let manifest = Manifest {
        schema_version: manifest::SCHEMA_VERSION,
        run_id,
        orchestrator_version: manifest::ORCHESTRATOR_VERSION.into(),
        started_utc: started,
        finished_utc: manifest::now_iso(),
        capture_type: prepared.capture_type,
        final_exit_status: exit.code(),
        archives: manifest::archive_entries(&prepared.extractions, &prepared.skipped, &args.out),
        hosts,
    };
    if let Err(e) = manifest::write(&manifest, &args.out) {
        die(
            format!("cannot write manifest: {e}"),
            RunExit::OutputFailure,
        );
    }
    exit
}

/// Validate `--only`/`--skip` against the in-process registry and build the
/// selected tools.
///
/// External-tool keys are not in-process registry keys, so they must not reach
/// the registry's validation, which would reject them as unknown. Only `skip`
/// is filtered, deliberately: `--only hayabusa` must keep erroring, because
/// --only selects which in-process parsers run and an external tool is not one
/// of them.
fn select_tools(only: &[String], skip: &[String], opts: ToolOptions) -> Vec<ToolEntry> {
    let external_keys = external::registry::keys();
    let registry_skip: Vec<String> = skip
        .iter()
        .filter(|k| !external_keys.contains(&k.as_str()))
        .cloned()
        .collect();
    registry::select_with(only, &registry_skip, opts).unwrap_or_else(|e| die(e, RunExit::Usage))
}

/// Read and resolve `--config`/`--profile`, then apply `--skip <external key>`,
/// which is an unconditional CLI-level force-disable for one run: it wins over
/// whatever the config file or the selected profile set `enabled` to.
fn load_external_config(
    config: Option<&Path>,
    profile: Option<&str>,
    skip: &[String],
) -> ResolvedConfig {
    let text = match config {
        Some(path) => std::fs::read_to_string(path).unwrap_or_else(|e| {
            die(
                format!("cannot read config {}: {e}", path.display()),
                RunExit::Usage,
            )
        }),
        None => String::new(),
    };
    let mut resolved = ExternalConfig::parse(&text)
        .and_then(|parsed| parsed.resolve(profile))
        .unwrap_or_else(|e| die(e, RunExit::Usage));
    for tool in external::registry::ALL {
        if skip.iter().any(|k| k == tool.key()) {
            tool.disable(&mut resolved);
        }
    }
    resolved
}

/// Discover, run every in-process tool, then every external tool, over one
/// host, and fold the results into its manifest entry.
fn run_host(ctx: &RunContext, host: &HostCapture, totals: &mut Totals) -> HostEntry {
    ctx.ui.host_header(&host.host, &host.os);
    // Exclude the output root from discovery if it lives under the capture.
    //
    // An extracted archive's artifact root lives *inside* the output root
    // (`<out>/_extracted/...`), so excluding `<out>` wholesale would hide the
    // very evidence we just unpacked. Drop any exclude that contains this
    // host's artifact root; tool output never lands under it, so nothing
    // self-discovers.
    let exclude: Vec<PathBuf> = [
        ctx.out_opts.csv_root.clone(),
        ctx.out_opts.json_root.clone(),
        Some(ctx.out.join(EXTRACTED_DIR)),
    ]
    .into_iter()
    .flatten()
    .filter(|p| !host.artifact_root.starts_with(p))
    .collect();
    let index = execute::build_index(&host.artifact_root, ctx.tools, &exclude);
    let results = execute::run_tools_bounded(
        &ctx.tool_keys,
        host,
        &index,
        &ctx.out_opts,
        ctx.jobs,
        ctx.heavy_jobs,
        Some(&ctx.ui),
    );
    for r in &results {
        totals.absorb(r);
    }
    let external_tools = external::run_external_tools_for_host(&ctx.external, host, ctx.out);
    for report in &external_tools {
        ctx.ui.external_tool_finished(report);
    }
    HostEntry {
        host: host.host.clone(),
        output_id: host.output_id.clone(),
        os: host.os.clone(),
        collection: file_name_lossy(&host.collection_dir),
        source_archive: host.source_archive.as_deref().map(file_name_lossy),
        inaccessible_entries: index.inaccessible,
        tools: results.into_iter().map(Into::into).collect(),
        external_tools,
    }
}
