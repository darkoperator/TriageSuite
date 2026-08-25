use crate::args::{self, CommonArgs};
use crate::banner::banner;
use crate::progress;
use std::path::PathBuf;
use triage_core::attribution::Attributor;
use triage_core::discovery;
use triage_core::error::{RunExit, TriageError};
use triage_core::output::layout::OutputLayoutMode;
use triage_core::output::router::{OutputRouter, RouterOptions};
use triage_core::summary::RunSummary;
use triage_core::tool::{Tool, Validation};

/// Tool-specific runner options (CommonArgs stays spec-common).
#[derive(Default)]
pub struct RunOptions {
    /// SHA-1 content dedupe between validation and parsing
    /// (tools expose this as --dedupe; first discovered file wins).
    pub dedupe: bool,
}

/// Drive a tool through the full pipeline. Returns the process exit code.
/// All console output goes to stderr; stdout is never written (spec 3.3).
pub fn run(tool: &dyn Tool, args: &CommonArgs, version: &str) -> i32 {
    run_with_options(tool, args, version, RunOptions::default())
}

/// Drive a tool through the full pipeline with tool-specific options.
/// Returns the process exit code.
pub fn run_with_options(
    tool: &dyn Tool,
    args: &CommonArgs,
    version: &str,
    opts: RunOptions,
) -> i32 {
    crate::logging::init(args.debug, args.trace);
    eprintln!("{}", banner(tool.binary_name(), version));

    match run_inner(tool, args, &opts) {
        Ok(code) => code.code(),
        Err(e) => {
            eprintln!("Error: {e}");
            e.run_exit().code()
        }
    }
}

/// Resolve the output identity for an artifact given the tool's scope.
/// SystemWide -> always system; UserSpecific -> derive (unattributable ->
/// unknown); UserElseSystem -> derive, but non-user (special profile OR
/// unattributable) -> system.
pub fn resolve_identity(
    scope: triage_core::tool::Scope,
    attributor: &mut triage_core::attribution::Attributor,
    path: &std::path::Path,
) -> triage_core::attribution::Identity {
    use triage_core::attribution::Identity;
    use triage_core::tool::Scope;
    match scope {
        Scope::SystemWide => Identity::System,
        Scope::UserSpecific => attributor.identity_for(path),
        Scope::UserElseSystem => match attributor.identity_for(path) {
            Identity::User(u) => Identity::User(u),
            _ => Identity::System,
        },
    }
}

/// Outcome of parsing a validated file set (records are computed by the
/// caller via `router.finish()`, since sinks close there).
pub struct ParseOutcome {
    pub parsed: u64,
    pub failed: u64,
    pub emitted: u64,
    pub abort: Option<TriageError>,
}

/// Parse an already-validated, already-deduped file set through `router`.
/// Attribution and identity routing match the standalone runner exactly.
pub fn parse_validated(
    tool: &dyn Tool,
    valid: &[PathBuf],
    router: &mut OutputRouter,
    quiet: bool,
    progress: &mut dyn crate::progress::Progress,
) -> ParseOutcome {
    let mut attributor = Attributor::new();
    let scope = tool.scope();
    let mut out = ParseOutcome {
        parsed: 0,
        failed: 0,
        emitted: 0,
        abort: None,
    };
    for path in valid {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        progress.file_start(&name);
        if !quiet {
            progress.info(&format!("Processing: {name}"));
        }
        // Attribution uses the artifact's real path. `derive_user` anchors on
        // the capture's drive-root segment, so a host filesystem prefix (e.g.
        // a capture stored under /Users/analyst/cases/...) is ignored and the
        // owning user is taken from inside the capture. This works identically
        // in directory-scan and explicit-file modes.
        let identity = resolve_identity(scope, &mut attributor, path);
        router.set_identity(identity);
        match tool.parse(path, router) {
            Ok(n) => {
                out.parsed += 1;
                out.emitted += n;
            }
            Err(e @ TriageError::Output { .. }) => {
                out.abort = Some(e);
                progress.file_done();
                break;
            }
            Err(e) => {
                out.failed += 1;
                eprintln!("Warning: {e}");
            }
        }
        progress.file_done();
    }
    out
}

fn run_inner(
    tool: &dyn Tool,
    args: &CommonArgs,
    opts: &RunOptions,
) -> Result<RunExit, TriageError> {
    args::validate(args)?;

    let mut router = OutputRouter::new(
        tool.binary_name(),
        tool.datasets(),
        RouterOptions {
            csv_root: args.csv.clone(),
            json_root: args.json.clone(),
            csvf: args.csvf.clone(),
            jsonf: args.jsonf.clone(),
            pretty: args.pretty,
            overwrite: args.overwrite,
            run_stamp: Some(triage_core::output::router::run_stamp()),
            layout_mode: if args.nested_output {
                OutputLayoutMode::Nested
            } else {
                OutputLayoutMode::Flat
            },
        },
    )?;

    let mut progress = progress::auto();
    let mut summary = RunSummary::default();

    // --- Discover or take explicit files ---
    let candidates: Vec<PathBuf> = if let Some(dir) = &args.directory {
        if !dir.is_dir() {
            return Err(TriageError::InputMissing { path: dir.clone() });
        }
        let exclude = router.roots();
        // Scope the closure so the mutable borrow of `progress` ends before
        // progress is used again below (borrow checker requires this).
        let report = {
            discovery::discover(dir, tool.patterns(), &exclude, &mut |p| {
                progress.discovery_tick(p)
            })
        };
        summary.inaccessible = report.inaccessible;
        report.files
    } else {
        for f in &args.files {
            if !f.is_file() {
                return Err(TriageError::InputMissing { path: f.clone() });
            }
        }
        args.files.clone()
    };
    summary.discovered = candidates.len() as u64;

    // --- Content validation (spec 3.2: never extension-only) ---
    let mut valid: Vec<PathBuf> = Vec::new();
    for path in candidates {
        match tool.validate(&path) {
            Validation::Supported => {
                summary.supported += 1;
                valid.push(path);
            }
            Validation::Unsupported { reason } => {
                summary.unsupported += 1;
                summary.skipped += 1;
                tracing::warn!("skipping {}: {reason}", path.display());
            }
            Validation::Corrupt { reason } => {
                summary.corrupt += 1;
                summary.failed += 1;
                tracing::warn!("corrupt {}: {reason}", path.display());
            }
            Validation::Unreadable { error } => {
                summary.unreadable += 1;
                summary.failed += 1;
                tracing::warn!("unreadable {}: {error}", path.display());
            }
        }
    }
    summary.validated = valid.len() as u64;

    // --- SHA-1 content dedupe (first discovered wins) ---
    if opts.dedupe {
        let mut set = triage_core::dedupe::DedupeSet::new();
        let mut unique = Vec::with_capacity(valid.len());
        for path in valid {
            match set.insert(&path) {
                Ok(true) => unique.push(path),
                Ok(false) => {
                    summary.deduped += 1;
                    tracing::debug!("dedupe: skipping {}", path.display());
                }
                Err(e) => {
                    summary.failed += 1;
                    eprintln!("Warning: cannot hash {}: {e}", path.display());
                }
            }
        }
        valid = unique;
    }

    if valid.is_empty() {
        progress.begin(0);
        progress.finish("Completed");
        eprintln!("Warning: no supported artifacts found");
        eprintln!("{summary}");
        // Exit 0 per spec 3.6 when nothing was found. If files validated but
        // then failed hashing during dedupe, summary.failed > 0 and exit()
        // correctly reports those as artifact failures instead.
        return Ok(summary.exit());
    }

    // --- Parse ---
    progress.begin(valid.len() as u64);
    let outcome = parse_validated(tool, &valid, &mut router, args.quiet, progress.as_mut());
    summary.parsed += outcome.parsed;
    summary.failed += outcome.failed;
    let mut abort = outcome.abort;

    // --- Epilogue: always close sinks, finish progress, print summary ---
    match router.finish() {
        Ok(records) => summary.records = records,
        Err(e) => {
            let _ = abort.get_or_insert(e);
        }
    };
    let exit = match &abort {
        Some(_) => RunExit::OutputFailure,
        None => summary.exit(),
    };
    progress.finish(match exit {
        RunExit::Success => "Completed",
        RunExit::Partial => "Partial",
        _ => "Failed",
    });
    eprintln!("{summary}");
    match abort {
        Some(e) => Err(e),
        None => Ok(exit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::NullProgress;
    use std::path::Path;
    use triage_core::attribution::{Attributor, Identity};
    use triage_core::output::dataset::DatasetSpec;
    use triage_core::tool::Scope;

    /// Minimal `Tool` fixture: fails for any path whose filename contains
    /// "fail", succeeds (emitting 1 record) for everything else. Used to
    /// exercise `parse_validated` without any real forensic-format parsing.
    struct FakeTool;

    impl Tool for FakeTool {
        fn binary_name(&self) -> &'static str {
            "FakeTool"
        }
        fn patterns(&self) -> &[&'static str] {
            &[]
        }
        fn validate_legacy(&self, _path: &Path) -> bool {
            true
        }
        fn datasets(&self) -> &'static [DatasetSpec] {
            &[]
        }
        fn scope(&self) -> Scope {
            Scope::SystemWide
        }
        fn parse(&self, path: &Path, _out: &mut OutputRouter) -> Result<u64, TriageError> {
            let is_fail = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.contains("fail"))
                .unwrap_or(false);
            if is_fail {
                Err(TriageError::Artifact {
                    path: path.to_path_buf(),
                    message: "boom".into(),
                })
            } else {
                Ok(1)
            }
        }
    }

    fn empty_router() -> OutputRouter {
        OutputRouter::new(
            "FakeTool",
            &[],
            RouterOptions {
                csv_root: None,
                json_root: None,
                csvf: None,
                jsonf: None,
                pretty: false,
                overwrite: false,
                run_stamp: None,
                layout_mode: OutputLayoutMode::Flat,
            },
        )
        .unwrap()
    }

    #[test]
    fn parse_validated_counts_parsed_and_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let ok_path = tmp.path().join("ok.txt");
        let fail_path = tmp.path().join("fail.txt");
        std::fs::write(&ok_path, b"x").unwrap();
        std::fs::write(&fail_path, b"x").unwrap();
        let valid = vec![ok_path, fail_path];

        let mut router = empty_router();
        let mut progress = NullProgress;
        let outcome = parse_validated(&FakeTool, &valid, &mut router, true, &mut progress);

        assert_eq!(outcome.parsed, 1);
        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.emitted, 1);
        assert!(outcome.abort.is_none());
    }

    #[test]
    fn user_else_system_routes_nonuser_to_system() {
        let mut a = Attributor::new();
        // a capture path with no derivable user -> System (not Unknown)
        assert_eq!(
            resolve_identity(
                Scope::UserElseSystem,
                &mut a,
                Path::new("C%3A/ProgramData/Microsoft/x.lnk")
            ),
            Identity::System
        );
        // an in-capture user path -> that user
        assert_eq!(
            resolve_identity(
                Scope::UserElseSystem,
                &mut a,
                Path::new("C%3A/Users/alice/Recent/x.lnk")
            ),
            Identity::User("alice".into())
        );
        // a special profile under UserElseSystem -> System
        assert_eq!(
            resolve_identity(
                Scope::UserElseSystem,
                &mut a,
                Path::new("C%3A/Windows/ServiceProfiles/LocalService/x.lnk")
            ),
            Identity::System
        );
    }

    #[test]
    fn user_specific_routes_nonuser_to_unknown() {
        let mut a = Attributor::new();
        assert_eq!(
            resolve_identity(
                Scope::UserSpecific,
                &mut a,
                Path::new("C%3A/ProgramData/x.lnk")
            ),
            Identity::Unknown
        );
    }

    #[test]
    fn system_wide_is_always_system() {
        let mut a = Attributor::new();
        assert_eq!(
            resolve_identity(
                Scope::SystemWide,
                &mut a,
                Path::new("C%3A/Users/alice/x.pf")
            ),
            Identity::System
        );
    }
}
