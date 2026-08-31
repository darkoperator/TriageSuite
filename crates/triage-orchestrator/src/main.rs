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
            no_timeline,
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

            // External-tool keys are not in-process `Tool` registry keys
            // (`registry::ALL_KEYS`), so they must not reach `select_with`'s
            // --only/--skip validation, which would reject them as unknown. Strip them
            // out for that call only; the raw `skip` vec is consulted again below to
            // force-disable them on the resolved config.
            //
            // Deliberately asymmetric: only `skip` is filtered. `--only hayabusa` must
            // keep erroring with "unknown tool key", because --only selects which
            // in-process parsers run and an external tool is not one of them.
            let external_keys = triage_orchestrator::external::registry::keys();
            let registry_skip: Vec<String> = skip
                .iter()
                .filter(|k| !external_keys.contains(&k.as_str()))
                .cloned()
                .collect();
            let tool_options = triage_orchestrator::registry::ToolOptions { hunt, no_timeline };
            let tools = match triage_orchestrator::registry::select_with(
                &only,
                &registry_skip,
                tool_options,
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
            let parsed_external =
                match triage_orchestrator::external::ExternalConfig::parse(&external_config_text) {
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
            // `--skip <external key>` is an unconditional, CLI-level force-disable for
            // one run: it wins over whatever the config file or the selected profile set
            // `enabled` to, without needing to edit either.
            for key in &external_keys {
                if skip.iter().any(|k| k == key) {
                    triage_orchestrator::external::registry::get(key)
                        .expect("registry::keys() entries all resolve through get()")
                        .disable(&mut resolved_external);
                }
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
                tools: tool_options,
            };

            let ui = triage_orchestrator::progress_ui::ProgressUi::new(no_progress);

            // Resolve the input last, after every cheap validation above: a
            // typo'd --only or a malformed --config must fail in milliseconds,
            // not after extracting hundreds of gigabytes.
            let prep_opts = triage_orchestrator::input::PrepareOptions {
                reuse_existing: !overwrite,
                ..Default::default()
            };
            let prepared =
                match triage_orchestrator::input::prepare(&capture, &out, &prep_opts, &ui) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(triage_core::error::RunExit::InputMissing.code());
                    }
                };
            let capture_type = prepared.capture_type;
            let hosts = &prepared.hosts;

            let mut host_entries = Vec::new();
            let mut successful_artifacts = 0u64;
            let mut failed_artifacts = 0u64;
            let mut terminal_exit = None;
            for host in hosts {
                ui.host_header(&host.host, &host.os);
                // Exclude the output root from discovery if it lives under the capture.
                //
                // An extracted archive's artifact root lives *inside* the output
                // root (`<out>/_extracted/...`), so excluding `<out>` wholesale
                // would hide the very evidence we just unpacked. Drop any exclude
                // that contains this host's artifact root; tool output never lands
                // under it, so nothing self-discovers.
                let exclude: Vec<PathBuf> = [
                    csv_root.clone(),
                    json_root.clone(),
                    Some(out.join(triage_orchestrator::input::EXTRACTED_DIR)),
                ]
                .into_iter()
                .flatten()
                .filter(|p| !host.artifact_root.starts_with(p))
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
                for report in &external_tools {
                    ui.external_tool_finished(report);
                }
                host_entries.push(triage_orchestrator::manifest::HostEntry {
                    host: host.host.clone(),
                    output_id: host.output_id.clone(),
                    os: host.os.clone(),
                    collection: host
                        .collection_dir
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    source_archive: host
                        .source_archive
                        .as_ref()
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
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
                triage_orchestrator::manifest::archive_entries(
                    &prepared.extractions,
                    &prepared.skipped,
                    &out,
                ),
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
