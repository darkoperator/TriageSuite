use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use triage_core::error::RunExit;
use triage_orchestrator::capture::{CaptureType, HostCapture};
use triage_orchestrator::execute::{self, OutputOpts, ToolRunResult};
use triage_orchestrator::external::{self, ExternalConfig, ResolvedConfig};
use triage_orchestrator::file_name_lossy;
use triage_orchestrator::input::{self, PrepareOptions, EXTRACTED_DIR};
use triage_orchestrator::manifest::{self, HostEntry, Manifest};
use triage_orchestrator::progress_ui::{self, ProgressUi};
use triage_orchestrator::registry::{self, ToolEntry, ToolOptions};
use triage_orchestrator::validate::{self, ValidationReport};

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
    // `RunArgs` grew past clippy's large-enum-variant threshold once
    // --start/--end were added; boxing it keeps `Command` itself small
    // without shrinking `RunArgs`.
    Run(Box<RunArgs>),
    /// Check a capture for the artifacts the parsers need, without
    /// processing it
    Validate {
        /// Collector ZIP, folder of ZIPs, or mounted capture directory
        input: PathBuf,
    },
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
    /// Skip EvtxTriage's per-source-log (channel) individual CSV exports,
    /// which are written by default
    #[arg(long)]
    no_individual: bool,
    /// Optional TOML config for hayabusa/takajo (see docs/tools/TriageSuite.md, "Config and profiles")
    #[arg(long)]
    config: Option<PathBuf>,
    /// Named profile to apply from --config (must exist under [profiles.<name>])
    #[arg(long, requires = "config")]
    profile: Option<String>,
    /// Output tree shape. `velo` (default) writes a VeloProcessor-shaped
    /// category tree; `native` writes the per-tool, per-identity tree.
    #[arg(long, value_enum, default_value_t = triage_orchestrator::velo::Layout::Velo)]
    layout: triage_orchestrator::velo::Layout,
    /// Skip SHA256 hashing of generated output. Hashing reads every output
    /// file, which for MFT-sized CSVs means re-reading several GB.
    #[arg(long)]
    skip_hashes: bool,
    /// Skip the pre-flight capture-validation gate. The gate rejects
    /// deliberately minimal test fixtures along with genuinely broken
    /// captures, so a synthetic capture built for testing needs this.
    #[arg(long)]
    no_validate: bool,
    /// Run-wide time-range floor, ISO 8601 UTC. Passthrough only: it reaches
    /// EvtxTriage and Hayabusa (Takajo inherits through Hayabusa's already-
    /// filtered JSONL) -- every other tool has no shared notion of "the"
    /// record timestamp to filter on, so it emits its full output regardless
    /// and the manifest/logs/SysInfo report all say so.
    #[arg(long)]
    start: Option<String>,
    /// Run-wide time-range ceiling, ISO 8601 UTC. Paired with `--start`
    /// above; `--end` before `--start` is a usage error (exit 2).
    #[arg(long)]
    end: Option<String>,
}

/// Parse `--start`/`--end` as ISO 8601 UTC and reject `end < start`.
/// Malformed dates and an inverted range are both usage errors (exit 2, via
/// `RunExit::Usage`) -- caught here, before extraction, alongside the other
/// cheap validation `run` already does first.
fn parse_time_range(
    start: Option<&str>,
    end: Option<&str>,
) -> (
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
) {
    let parse = |raw: &str, flag: &str| -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(raw)
            .unwrap_or_else(|e| die(format!("invalid {flag} date {raw}: {e}"), RunExit::Usage))
            .with_timezone(&chrono::Utc)
    };
    let start = start.map(|s| parse(s, "--start"));
    let end = end.map(|s| parse(s, "--end"));
    if let (Some(s), Some(e)) = (start, end) {
        if e < s {
            die("--end must not be before --start", RunExit::Usage);
        }
    }
    (start, end)
}

fn main() {
    let exit = match Cli::parse().command {
        Command::Run(args) => run(*args),
        Command::Validate { input } => validate_subcommand(&input),
    };
    std::process::exit(exit.code());
}

/// Print a `ValidationReport` the same way for both the `validate`
/// subcommand and the `run` pre-flight gate.
fn print_validation_report(input: &Path, report: &ValidationReport) {
    for warning in &report.warnings {
        eprintln!("Warning: {}: {warning}", input.display());
    }
    for error in &report.errors {
        eprintln!("Error: {}: {error}", input.display());
    }
}

/// `TriageSuite validate <input>`: exit 0 when every capture the input
/// holds is valid (warnings still print), 3 when any is not. Reuses
/// `RunExit::InputMissing`, whose spec code is already 3, rather than
/// hand-rolling a new exit mapping.
///
/// Unlike `run`, one bad capture here fails the whole check: `validate`
/// answers "is this input fit to process", and a folder with a deficient
/// archive in it is not, even though `run` would go ahead with the rest.
fn validate_subcommand(input: &Path) -> RunExit {
    let checked = validate::validate_input(input);
    let mut valid = true;
    for (path, report) in &checked {
        print_validation_report(path, report);
        valid &= report.valid;
    }
    if valid {
        RunExit::Success
    } else {
        RunExit::InputMissing
    }
}

/// Apply the pre-flight gate to every enumerated collection, returning the
/// ones to process and appending a skip record for each one rejected.
///
/// A rejected collection is skipped, not fatal: the rest of the run still
/// proceeds, so one deficient archive in an engagement's folder costs that
/// host and no other.
///
/// The skip is recorded against the *source archive* whenever the whole
/// archive is what got rejected, because `archives[]` is chain-of-custody
/// output: naming the extracted directory instead would put a directory's
/// inode size and a null hash under fields called `archive_path`,
/// `size_bytes` and `sha256`, and wrong values there are worse than absent
/// ones. `manifest::archive_entries` folds that skip into the archive's own
/// entry rather than emitting a second one for the same path. An archive
/// that *also* yielded a collection which passed was not itself skipped, so
/// that rejection is recorded under the rejected collection's own path.
fn gate_collections(
    hosts: Vec<HostCapture>,
    skipped: &mut Vec<input::SkippedArchive>,
) -> Vec<HostCapture> {
    let mut accepted: Vec<HostCapture> = Vec::new();
    let mut rejected: Vec<(HostCapture, String)> = Vec::new();
    for host in hosts {
        let report = validate::validate_capture(&host.collection_dir);
        print_validation_report(&host.collection_dir, &report);
        if report.valid {
            accepted.push(host);
        } else {
            rejected.push((host, report.errors.join("; ")));
        }
    }
    for (host, reason) in rejected {
        let whole_archive_rejected = host.source_archive.as_ref().is_some_and(|src| {
            !accepted
                .iter()
                .any(|kept| kept.source_archive.as_deref() == Some(src))
        });
        let archive = match (whole_archive_rejected, host.source_archive) {
            (true, Some(src)) => src,
            _ => host.collection_dir,
        };
        skipped.push(input::SkippedArchive { archive, reason });
    }
    accepted
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

/// Record a failed write in one collection's output-compat pipeline -- the
/// VeloResults copy, the source hash log, the SysInfo report, the Timeline
/// Explorer sessions, the output hash walk.
///
/// These five used to call `die`, which exits the process before the manifest
/// is written: the same defect as the rejected-input path, and worse, because
/// over a reused `--out` it left the *previous* run's successful
/// `run_manifest.json` standing next to this run's partial output. The message
/// still prints exactly as `die` printed it, and `terminal` still makes the
/// run exit 4 -- `aggregate_exit` gives `OutputFailure` precedence over every
/// artifact count, and never clears it -- but the run now reaches the manifest
/// write, and the remaining hosts still get processed rather than being
/// abandoned on one collection's I/O failure.
///
/// The message also goes into this host's `output_errors`, because console
/// output is transient and the manifest is the chain-of-custody record: a
/// `final_exit_status: 4` with two normal-looking host entries says the run
/// failed without saying which collection's output is incomplete.
fn output_failed(totals: &mut Totals, host_errors: &mut Vec<String>, message: String) {
    eprintln!("Error: {message}");
    totals.terminal = Some(RunExit::OutputFailure);
    host_errors.push(message);
}

fn run(args: RunArgs) -> RunExit {
    progress_ui::print_banner();

    // Cheap validation first: a typo'd --only, a malformed --config, or an
    // inverted --start/--end must fail in milliseconds, not after extracting
    // hundreds of gigabytes.
    let (start, end) = parse_time_range(args.start.as_deref(), args.end.as_deref());
    let tool_options = ToolOptions {
        hunt: args.hunt,
        no_timeline: args.no_timeline,
        no_individual: args.no_individual,
        start,
        end,
    };
    let tools = select_tools(&args.only, &args.skip, tool_options);
    let mut external =
        load_external_config(args.config.as_deref(), args.profile.as_deref(), &args.skip);
    // CLI --start/--end take precedence over whatever the config file or
    // selected profile set for Hayabusa's own timeline bounds -- an
    // unconditional overwrite, not a merge, matching how --skip already wins
    // over the config file for `enabled`.
    if let Some(start) = start {
        external.hayabusa.timeline_start =
            Some(start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    if let Some(end) = end {
        external.hayabusa.timeline_end =
            Some(end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    let time_range_notice = triage_orchestrator::time_range_notice(start, end);
    if let Some(notice) = &time_range_notice {
        println!("{notice}");
    }
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
            layout: args.layout,
            velo_stamp: triage_core::output::router::velo_run_stamp(),
            skip_hashes: args.skip_hashes,
            time_range_notice: time_range_notice.clone(),
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
    let prepared = match input::prepare(&args.capture, &args.out, &prep_opts, &ctx.ui) {
        Ok(prepared) => prepared,
        Err(rejection) => return reject_input(&args, run_id, started, rejection),
    };

    // Pre-flight gate: catch a collection missing whole artifact classes (or
    // a double-zipped one) before spending time parsing it.
    //
    // It runs here, after enumeration, because what the user pointed `run`
    // at may be a container -- a folder of collector ZIPs, or of collections
    // -- and a container's own file list is archive names rather than
    // artifacts. Enumeration is what turns every accepted input shape into
    // the individual collections the criteria actually describe.
    let mut skipped = prepared.skipped;
    let to_run = if args.no_validate {
        prepared.hosts
    } else {
        gate_collections(prepared.hosts, &mut skipped)
    };

    let mut totals = Totals::default();
    let hosts: Vec<HostEntry> = to_run
        .iter()
        .map(|host| run_host(&ctx, host, &mut totals))
        .collect();

    // `prepare` never returns an empty host list, so nothing to run means
    // the gate rejected every collection: the run's status is then the
    // input's, not the tools'.
    let exit = if hosts.is_empty() {
        RunExit::InputMissing
    } else {
        totals.exit()
    };
    let manifest = Manifest {
        schema_version: manifest::SCHEMA_VERSION,
        run_id,
        orchestrator_version: manifest::ORCHESTRATOR_VERSION.into(),
        started_utc: started,
        finished_utc: manifest::now_iso(),
        capture_type: prepared.capture_type,
        final_exit_status: exit.code(),
        archives: manifest::archive_entries(
            &prepared.extractions,
            &skipped,
            &args.out,
            args.skip_hashes,
        ),
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

/// Record an input that `input::prepare` refused, then exit 3.
///
/// A rejected run is still a run, and a run leaves a chain-of-custody
/// record. This used to exit through `die`, which writes nothing: an analyst
/// was left with exit 3 on a terminal and no file saying which input was
/// refused or why — and worse, with a reused `--out`, `run_manifest.json`
/// stayed the *previous* run's successful manifest, a success record for a
/// run that never happened.
///
/// The rejection is described entirely in fields the manifest already has:
/// `hosts` is empty because nothing was processed, `final_exit_status` is 3,
/// and `archives[]` carries one entry per refused input with the reason in
/// its `error`.
fn reject_input(
    args: &RunArgs,
    run_id: String,
    started: String,
    rejection: input::PrepareRejection,
) -> RunExit {
    eprintln!("Error: {}", rejection.reason);
    let mut skipped = rejection.skipped;
    // The path the user actually pointed `run` at is recorded as skipped
    // too, carrying the run-level reason — "not found", or a folder whose
    // archives were every one of them unusable — which no per-archive entry
    // states. Unless an archive skip already names that exact path: a lone
    // `.zip` input is both the input and the archive, and it keeps its own,
    // more specific reason rather than gaining a second, vaguer record of
    // the same file. An extraction that *failed* is exempt for the same
    // reason: its entry already carries the filesystem error that stopped it,
    // and `manifest::archive_entries` folds a skip into an existing entry, so
    // pushing one here would overwrite a named cause with the run-level
    // verdict. An extraction that *succeeded* has no reason of its own, and
    // is not exempt: that fold is how its entry gains one, rather than a
    // second entry appearing for the same path.
    let already_named = skipped.iter().any(|s| s.archive == args.capture)
        || rejection
            .extractions
            .iter()
            .any(|r| r.archive == args.capture && r.error.is_some());
    if !already_named {
        skipped.push(input::SkippedArchive {
            archive: args.capture.clone(),
            reason: rejection.reason,
        });
    }
    let exit = RunExit::InputMissing;
    let manifest = Manifest {
        schema_version: manifest::SCHEMA_VERSION,
        run_id,
        orchestrator_version: manifest::ORCHESTRATOR_VERSION.into(),
        started_utc: started,
        finished_utc: manifest::now_iso(),
        // Nothing was identified, so nothing is claimed: see
        // `CaptureType::Unidentified`.
        capture_type: CaptureType::Unidentified,
        final_exit_status: exit.code(),
        archives: manifest::archive_entries(
            &rejection.extractions,
            &skipped,
            &args.out,
            args.skip_hashes,
        ),
        hosts: Vec::new(),
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
    // Failures in this collection's output-compat writes, collected for its
    // manifest entry by `output_failed` below.
    let mut output_errors: Vec<String> = Vec::new();
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
    // Process logs are a Velo-layout concept (`crate::velo::proclog`): under
    // `--layout native` there is no `Processed-<HOST>-<stamp>` directory of
    // this shape to hold one, matching how the hashing block below is gated
    // on the same `Layout::Velo` condition. `collection_dir_for` is the one
    // place that condition lives, shared with `execute.rs`'s own process-log
    // gating so the two cannot drift.
    let collection_dir = triage_orchestrator::velo::collection_dir_for(
        ctx.out_opts.layout,
        ctx.out,
        &host.output_id,
        &ctx.out_opts.velo_stamp,
    );
    let external_tools = external::run_external_tools_for_host(
        &ctx.external,
        host,
        ctx.out,
        collection_dir.as_deref(),
        ctx.out_opts.overwrite,
    );
    for report in &external_tools {
        ctx.ui.external_tool_finished(report);
    }

    // VeloResults is copied unconditionally (not gated on `--skip-hashes`):
    // it is a passthrough of Velociraptor's own results, not part of the
    // hashing work that flag exists to let an analyst skip on MFT-sized
    // output. It still has to land before the hash walk below, so its files
    // are covered by `CaseInfo/<stamp>_OutputHashes.txt` like every other
    // output-producing step.
    if let Some(collection_dir) = &collection_dir {
        if let Err(e) = triage_orchestrator::velo::veloresults::copy_velo_results(
            &host.collection_dir,
            collection_dir,
        ) {
            output_failed(
                totals,
                &mut output_errors,
                format!("cannot copy Velociraptor results: {e}"),
            );
        }
    }

    // The closing block of the per-collection pipeline: the source hash log,
    // the SysInfo report, the Timeline Explorer sessions, and last of all
    // the output hash walk. Every other output-producing step for this
    // collection -- tool output, external tools, the copied VeloResults
    // tree -- runs above this point, and the three writes inside the block
    // all precede `write_output_hashes`, so its walk covers all of them.
    // Anything added later that writes into this collection directory must
    // be inserted above the `write_output_hashes` call, never after it.
    if let Some(collection_dir) = &collection_dir {
        // The only two steps `--skip-hashes` suppresses are the two that
        // compute SHA256 digests: this one (which hashes the source archive)
        // and `write_output_hashes` below (which hashes every generated
        // file). The SysInfo report and the Timeline Explorer sessions
        // between them are not hashing work and are written either way --
        // they used to sit inside this gate, and `--only le --skip-hashes`
        // consequently produced no `CaseInfo/` and no `Sessions/` at all,
        // which is not what the flag is documented (or named) to do.
        //
        // Written before `write_output_hashes` walks the collection, on
        // purpose: the hash log becomes one more file in that walk, so it
        // ends up covered by its own sibling record. Its absence would be a
        // gap in the chain of custody, not a special case to exclude.
        if !ctx.out_opts.skip_hashes {
            if let Err(e) = triage_orchestrator::velo::hashes::write_source_hash_log(
                collection_dir,
                &ctx.out_opts.velo_stamp,
                host.source_archive.as_deref(),
            ) {
                output_failed(
                    totals,
                    &mut output_errors,
                    format!("cannot write source hash log: {e}"),
                );
            }
        }
        // Written before `write_output_hashes` walks the collection, same
        // reasoning as the source hash log above: the SysInfo report becomes
        // one more file in that walk rather than a gap in the chain of
        // custody. `Ok(None)` means RETriage produced no output for this
        // collection, which is not a failure.
        if let Err(e) = triage_orchestrator::velo::sysinfo::write_sysinfo(
            collection_dir,
            &ctx.out_opts.velo_stamp,
            ctx.out_opts.time_range_notice.as_deref(),
        ) {
            output_failed(
                totals,
                &mut output_errors,
                format!("cannot write sysinfo report: {e}"),
            );
        }
        // Written before `write_output_hashes` walks the collection, same
        // reasoning as the source hash log and SysInfo report above:
        // `Sessions/*.tle_sess` files become part of that walk rather than a
        // gap in the chain of custody.
        if let Err(e) = triage_orchestrator::velo::sessions::write_sessions(collection_dir) {
            output_failed(
                totals,
                &mut output_errors,
                format!("cannot write Timeline Explorer sessions: {e}"),
            );
        }
        // The second of the two hashing steps, and the last step of the
        // per-collection pipeline either way: it walks everything written
        // above, including the SysInfo report and the sessions.
        if !ctx.out_opts.skip_hashes {
            if let Err(e) = triage_orchestrator::velo::hashes::write_output_hashes(
                collection_dir,
                &ctx.out_opts.velo_stamp,
            ) {
                output_failed(
                    totals,
                    &mut output_errors,
                    format!("cannot write output hashes: {e}"),
                );
            }
        }
    }

    HostEntry {
        host: host.host.clone(),
        output_id: host.output_id.clone(),
        os: host.os.clone(),
        collection: file_name_lossy(&host.collection_dir),
        source_archive: host.source_archive.as_deref().map(file_name_lossy),
        inaccessible_entries: index.inaccessible,
        output_errors,
        tools: results
            .into_iter()
            .map(|r| {
                let mut report: manifest::ToolEntryReport = r.into();
                report.time_filter = manifest::time_filter_for(&report.key, &ctx.out_opts.tools);
                report
            })
            .collect(),
        external_tools,
    }
}
