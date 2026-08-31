use crate::external::args::{os, push_flag, push_opt};
use crate::external::config::{HayabusaConfig, ResolvedConfig};
use crate::external::tool::{
    Artifacts, ExternalTool, HostContext, Invocation, OutputDirPolicy, OutputSpec, Publish,
};
use std::ffi::OsString;
use std::path::Path;

/// Output subdirectory under each host's output root.
const DIR: &str = "Hayabusa";

/// `logon-summary`'s `--output` is a filename prefix, not a file — Hayabusa
/// derives a variable number of `<prefix>-*.csv` names from it.
const LOGON_SUMMARY_PREFIX: &str = "logon-summary";

/// The JSONL timeline Takajo's `automagic` consumes. Named here, next to the
/// invocation that produces it, rather than in Takajo, which only reads it.
pub const JSONL_SLOT: &str = "hayabusa.jsonl";

pub struct Hayabusa;

impl ExternalTool for Hayabusa {
    fn key(&self) -> &'static str {
        "hayabusa"
    }

    fn enabled(&self, cfg: &ResolvedConfig) -> bool {
        cfg.hayabusa.enabled
    }

    fn disable(&self, cfg: &mut ResolvedConfig) {
        cfg.hayabusa.enabled = false;
    }

    fn bin<'a>(&self, cfg: &'a ResolvedConfig) -> &'a str {
        &cfg.hayabusa.bin
    }

    fn publishable_slots(&self) -> &'static [&'static str] {
        &[JSONL_SLOT]
    }

    fn plan(
        &self,
        cfg: &ResolvedConfig,
        ctx: &HostContext<'_>,
        _prior: &Artifacts,
    ) -> Vec<Invocation> {
        let hayabusa = &cfg.hayabusa;
        let dir = ctx.host_dir.join(DIR);
        let input = &ctx.host.artifact_root;
        let mut plan = Vec::new();

        if hayabusa.csv {
            let out_file = dir.join("timeline.csv");
            plan.push(Invocation {
                report_name: "hayabusa-csv",
                args: hayabusa_csv_args(hayabusa, input, &out_file),
                work_dir: dir.clone(),
                dir_policy: OutputDirPolicy::CreateIfMissing,
                outputs: OutputSpec::Path(out_file),
                publishes: None,
            });
        }
        if hayabusa.json {
            let out_file = dir.join("timeline.jsonl");
            plan.push(Invocation {
                report_name: "hayabusa-json",
                args: hayabusa_json_args(hayabusa, input, &out_file),
                work_dir: dir.clone(),
                dir_policy: OutputDirPolicy::CreateIfMissing,
                outputs: OutputSpec::Path(out_file.clone()),
                // The driver publishes this only if the file really exists
                // afterwards, so the Takajo chain is gated on the JSONL being on
                // disk rather than on a zero exit code.
                publishes: Some(Publish {
                    slot: JSONL_SLOT,
                    path: out_file,
                }),
            });
        }
        if hayabusa.logon_summary {
            let prefix_path = dir.join(LOGON_SUMMARY_PREFIX);
            plan.push(Invocation {
                report_name: "hayabusa-logon-summary",
                args: hayabusa_logon_summary_args(hayabusa, input, &prefix_path),
                work_dir: dir.clone(),
                dir_policy: OutputDirPolicy::CreateIfMissing,
                outputs: OutputSpec::PrefixedIn {
                    dir: dir.clone(),
                    prefix: LOGON_SUMMARY_PREFIX.to_string(),
                },
                publishes: None,
            });
        }
        plan
    }
}

/// Flags always passed for non-interactive, ISO-8601/UTC, unattended timeline generation
/// (spec: "Orchestrator-hardcoded constants" — never user-configurable). Lowercase long
/// names match Hayabusa >= 4.0 (all long option names were standardized to lowercase;
/// `--ISO-8601` became `--iso-8601`. #1909). `dfir-timeline`-only: `logon-summary` does
/// not accept `-w/--no-wizard` (confirmed against the real 4.0.0 binary — it errors with
/// "unexpected argument '--no-wizard' found").
const HARDCODED_FLAGS: &[&str] = &["--no-wizard", "--quiet", "--no-color", "--iso-8601"];

/// Same non-interactive/ISO-8601 intent as `HARDCODED_FLAGS`, minus `--no-wizard` for
/// `logon-summary`, which has no wizard mode and rejects the flag outright.
const LOGON_SUMMARY_HARDCODED_FLAGS: &[&str] = &["--quiet", "--no-color", "--iso-8601"];

fn shared_args(cfg: &HayabusaConfig) -> Vec<OsString> {
    let mut args = Vec::new();
    push_opt(&mut args, "--rules", &cfg.rules);
    push_opt(&mut args, "--rules-config", &cfg.rules_config);
    push_opt(&mut args, "--min-level", &cfg.min_level);
    push_opt(&mut args, "--profile", &cfg.profile);
    if let Some(threads) = cfg.threads {
        args.push(os("--threads"));
        args.push(os(threads.to_string()));
    }
    push_flag(&mut args, "--clobber", cfg.clobber);
    push_flag(&mut args, "--sort", cfg.sort);
    push_flag(&mut args, "--scan-all-evtx-files", cfg.scan_all_evtx_files);
    push_flag(&mut args, "--enable-all-rules", cfg.enable_all_rules);
    push_flag(&mut args, "--enable-noisy-rules", cfg.enable_noisy_rules);
    push_flag(
        &mut args,
        "--enable-deprecated-rules",
        cfg.enable_deprecated_rules,
    );
    push_flag(
        &mut args,
        "--enable-unsupported-rules",
        cfg.enable_unsupported_rules,
    );
    push_flag(&mut args, "--proven-rules", cfg.proven_rules);
    push_opt(&mut args, "--time-offset", &cfg.time_offset);
    push_opt(&mut args, "--timeline-start", &cfg.timeline_start);
    push_opt(&mut args, "--timeline-end", &cfg.timeline_end);
    args
}

/// Hayabusa >= 4.0 merged `csv-timeline`/`json-timeline` into a single `dfir-timeline`
/// command; format is chosen with `-t`/`--output-type` (csv/json/jsonl) instead of by
/// which command you ran. The old `json-timeline -L/--JSONL-output` flag is gone — use
/// `--output-type jsonl` (#1906).
pub fn hayabusa_csv_args(
    cfg: &HayabusaConfig,
    input_dir: &Path,
    output_file: &Path,
) -> Vec<OsString> {
    let mut args = vec![
        os("dfir-timeline"),
        os("--directory"),
        os(input_dir),
        os("--output-type"),
        os("csv"),
    ];
    args.extend(HARDCODED_FLAGS.iter().copied().map(os));
    args.extend(shared_args(cfg));
    args.push(os("--output"));
    args.push(os(output_file));
    args
}

pub fn hayabusa_json_args(
    cfg: &HayabusaConfig,
    input_dir: &Path,
    output_file: &Path,
) -> Vec<OsString> {
    let mut args = vec![
        os("dfir-timeline"),
        os("--directory"),
        os(input_dir),
        os("--output-type"),
        os("jsonl"),
    ];
    args.extend(HARDCODED_FLAGS.iter().copied().map(os));
    args.extend(shared_args(cfg));
    args.push(os("--output"));
    args.push(os(output_file));
    args
}

/// `logon-summary` is not sigma-rule-based, so only the flags it actually documents are
/// passed — NOT the full `shared_args()` set (most of which, e.g. `--rules`/`--enable-*-
/// rules`/`--min-level`/`--profile`/`--sort`/`--scan-all-evtx-files`, are `dfir-timeline`-
/// only and confirmed absent from `logon-summary --help` on the real 4.0.0 binary).
/// `--output` here is a filename PREFIX: Hayabusa writes two CSVs from it (confirmed:
/// `<prefix>-successful.csv`/`<prefix>-failed.csv`, and neither is written at all if no
/// logon events are found) — the exact suffixes aren't hardcoded anywhere in this crate;
/// callers discover the actual files written by listing the output directory instead.
pub fn hayabusa_logon_summary_args(
    cfg: &HayabusaConfig,
    input_dir: &Path,
    output_prefix: &Path,
) -> Vec<OsString> {
    let mut args = vec![os("logon-summary"), os("--directory"), os(input_dir)];
    args.extend(LOGON_SUMMARY_HARDCODED_FLAGS.iter().copied().map(os));
    if let Some(threads) = cfg.threads {
        args.push(os("--threads"));
        args.push(os(threads.to_string()));
    }
    push_flag(&mut args, "--clobber", cfg.clobber);
    push_opt(&mut args, "--time-offset", &cfg.time_offset);
    push_opt(&mut args, "--timeline-start", &cfg.timeline_start);
    push_opt(&mut args, "--timeline-end", &cfg.timeline_end);
    args.push(os("--output"));
    args.push(os(output_prefix));
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> HayabusaConfig {
        HayabusaConfig::default()
    }

    #[test]
    fn csv_args_carry_hardcoded_flags_and_input_output_paths() {
        let args = hayabusa_csv_args(&base(), Path::new("/ev"), Path::new("/out/timeline.csv"));
        let strs: Vec<String> = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            strs,
            vec![
                "dfir-timeline",
                "--directory",
                "/ev",
                "--output-type",
                "csv",
                "--no-wizard",
                "--quiet",
                "--no-color",
                "--iso-8601",
                "--output",
                "/out/timeline.csv",
            ]
        );
    }

    #[test]
    fn json_args_use_dfir_timeline_with_jsonl_output_type() {
        let args = hayabusa_json_args(&base(), Path::new("/ev"), Path::new("/out/timeline.jsonl"));
        let strs: Vec<String> = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(strs[0], "dfir-timeline");
        assert!(strs.windows(2).any(|w| w == ["--output-type", "jsonl"]));
        assert!(!strs.contains(&"--JSONL-output".to_string()));
        assert!(!strs.contains(&"json-timeline".to_string()));
    }

    #[test]
    fn logon_summary_args_use_directory_and_output_prefix_without_rule_flags() {
        let mut cfg = base();
        cfg.min_level = Some("high".to_string()); // dfir-timeline-only, must not leak in
        cfg.timeline_start = Some("2026-01-01T00:00:00Z".to_string());
        let args = hayabusa_logon_summary_args(&cfg, Path::new("/ev"), Path::new("/out/logon"));
        let strs: Vec<String> = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(strs[0], "logon-summary");
        assert!(strs.windows(2).any(|w| w == ["--directory", "/ev"]));
        assert!(strs.windows(2).any(|w| w == ["--output", "/out/logon"]));
        assert!(strs
            .windows(2)
            .any(|w| w == ["--timeline-start", "2026-01-01T00:00:00Z"]));
        assert!(strs.contains(&"--iso-8601".to_string()));
        // logon-summary has no wizard mode and rejects the flag outright.
        assert!(!strs.contains(&"--no-wizard".to_string()));
        assert!(!strs.contains(&"--min-level".to_string()));
        assert!(!strs.contains(&"--rules".to_string()));
        assert!(!strs.contains(&"--sort".to_string()));
    }

    #[test]
    fn optional_fields_only_emit_flags_when_set() {
        let mut cfg = base();
        cfg.min_level = Some("high".to_string());
        cfg.proven_rules = true;
        cfg.threads = Some(8);
        let args = hayabusa_csv_args(&cfg, Path::new("/ev"), Path::new("/out.csv"));
        let strs: Vec<String> = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(strs.windows(2).any(|w| w == ["--min-level", "high"]));
        assert!(strs.contains(&"--proven-rules".to_string()));
        assert!(strs.windows(2).any(|w| w == ["--threads", "8"]));
        // still nothing for fields that were never set
        assert!(!strs.contains(&"--rules".to_string()));
        assert!(!strs.contains(&"--sort".to_string()));
    }

    #[test]
    fn empty_string_optional_field_is_treated_as_unset() {
        let mut cfg = base();
        cfg.min_level = Some(String::new());
        let args = hayabusa_csv_args(&cfg, Path::new("/ev"), Path::new("/o"));
        let strs: Vec<String> = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(!strs.contains(&"--min-level".to_string()));
    }
}
