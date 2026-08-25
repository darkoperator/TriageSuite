use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
    Run {
        /// Capture directory (a Velociraptor collection or a folder of collections)
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
        /// Optional TOML config for hayabusa/takajo (see docs/superpowers/specs/2026-08-22-hayabusa-takajo-config-design.md)
        #[arg(long)]
        config: Option<PathBuf>,
        /// Named profile to apply from --config (must exist under [profiles.<name>])
        #[arg(long, requires = "config")]
        profile: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Run {
            capture,
            out,
            csv,
            json,
            only,
            skip,
            overwrite,
            jobs,
            heavy_jobs,
            no_progress,
            hunt,
            config,
            profile,
        } => {
            triage_orchestrator::progress_ui::print_banner();
            // Default to CSV when neither flag given.
            let (want_csv, want_json) = if !csv && !json {
                (true, false)
            } else {
                (csv, json)
            };
            let csv_root = want_csv.then(|| out.clone());
            let json_root = want_json.then(|| out.clone());

            let (capture_type, hosts) = match triage_orchestrator::capture::enumerate(&capture) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(3);
                }
            };
            // `hayabusa`/`takajo` are external-tool keys, not in-process `Tool` registry keys
            // (`registry::ALL_KEYS`), so they must not reach `select_with_hunt`'s --only/--skip
            // validation (it would reject them as unknown). Strip them out for that call only;
            // the raw `skip` vec (with them still present) is consulted separately below to
            // toggle `resolved_external`.
            let registry_skip: Vec<String> = skip
                .iter()
                .filter(|k| k.as_str() != "hayabusa" && k.as_str() != "takajo")
                .cloned()
                .collect();
            let tools = match triage_orchestrator::registry::select_with_hunt(
                &only,
                &registry_skip,
                hunt,
            ) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            };
            let external_config_text = match &config {
                Some(path) => match std::fs::read_to_string(path) {
                    Ok(text) => text,
                    Err(e) => {
                        eprintln!("Error: cannot read config {}: {e}", path.display());
                        std::process::exit(2);
                    }
                },
                None => String::new(),
            };
            let parsed_external = match triage_orchestrator::external_config::ExternalConfig::parse(
                &external_config_text,
            ) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            };
            let mut resolved_external = match parsed_external.resolve(profile.as_deref()) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            };
            // `hayabusa`/`takajo` aren't in the in-process tool registry (they're external
            // binaries, not `Tool` impls), so `registry::select_with_hunt` can't validate or
            // act on them as --only/--skip keys. Handle them here as a direct override on the
            // resolved config instead, independent of the registry's own --only/--skip
            // validation for tools like `pe`/`evtx`/etc, which must keep working exactly as
            // today.
            if skip.iter().any(|k| k == "hayabusa") {
                resolved_external.hayabusa.enabled = false;
            }
            if skip.iter().any(|k| k == "takajo") {
                resolved_external.takajo.enabled = false;
            }
            let tool_keys: Vec<String> = tools.iter().map(|t| t.key.to_string()).collect();
            let jobs = jobs.unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1)
            });

            let run_id = triage_orchestrator::manifest::run_id();
            let started = triage_orchestrator::manifest::now_iso();
            let out_opts = triage_orchestrator::execute::OutputOpts {
                csv_root: csv_root.clone(),
                json_root: json_root.clone(),
                overwrite,
                run_id: run_id.clone(),
                hunt,
            };

            let ui = triage_orchestrator::progress_ui::ProgressUi::new(no_progress);
            let mut host_entries = Vec::new();
            let mut successful_artifacts = 0u64;
            let mut failed_artifacts = 0u64;
            let mut terminal_exit = None;
            for host in &hosts {
                ui.host_header(&host.host, &host.os);
                // Exclude the output root from discovery if it lives under the capture.
                let exclude: Vec<PathBuf> = [csv_root.clone(), json_root.clone()]
                    .into_iter()
                    .flatten()
                    .collect();
                let index = triage_orchestrator::execute::build_index(
                    &host.artifact_root,
                    &tools,
                    &exclude,
                );
                let results = triage_orchestrator::execute::run_tools_bounded(
                    &tool_keys,
                    host,
                    &index,
                    &out_opts,
                    jobs,
                    heavy_jobs,
                    Some(&ui),
                );
                let mut tool_reports = Vec::new();
                for r in results {
                    successful_artifacts += r.parsed;
                    failed_artifacts += r.failed;
                    if r.error.is_some() {
                        failed_artifacts += 1;
                    }
                    if matches!(r.exit, Some(triage_core::error::RunExit::OutputFailure)) {
                        terminal_exit = Some(triage_core::error::RunExit::OutputFailure);
                    }
                    tool_reports.push(triage_orchestrator::manifest::ToolEntryReport {
                        tool: r.binary_name,
                        key: r.key,
                        files_matched: r.files_matched,
                        discovered_candidates: r.files_matched,
                        supported: r.supported,
                        unsupported: r.unsupported,
                        corrupt: r.corrupt,
                        unreadable: r.unreadable,
                        parsed: r.parsed,
                        failed: r.failed,
                        deduplicated: r.deduplicated,
                        records: r.records,
                        output_paths: r.output_paths,
                        reason_samples: r.reason_samples,
                        error: r.error,
                    });
                }
                let external_tools = triage_orchestrator::external::run_external_tools_for_host(
                    &resolved_external,
                    host,
                    &out,
                );
                host_entries.push(triage_orchestrator::manifest::HostEntry {
                    host: host.host.clone(),
                    output_id: host.output_id.clone(),
                    os: host.os.clone(),
                    collection: host
                        .collection_dir
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    inaccessible_entries: index.inaccessible,
                    tools: tool_reports,
                    external_tools,
                });
            }
            let finished = triage_orchestrator::manifest::now_iso();
            let exit = triage_orchestrator::execute::aggregate_exit(
                successful_artifacts,
                failed_artifacts,
                terminal_exit,
            );
            let manifest = triage_orchestrator::manifest::build(
                capture_type,
                &run_id,
                &started,
                &finished,
                exit.code(),
                host_entries,
            );
            if let Err(e) = triage_orchestrator::manifest::write(&manifest, &out) {
                eprintln!("Error: cannot write manifest: {e}");
                std::process::exit(4);
            }
            std::process::exit(exit.code());
        }
    }
}
