use crate::external_config::{HayabusaConfig, TakajoConfig};
use std::ffi::OsString;
use std::path::Path;

fn os<S: AsRef<std::ffi::OsStr>>(s: S) -> OsString {
    s.as_ref().to_os_string()
}

/// Flags always passed for non-interactive, ISO-8601/UTC, unattended timeline generation
/// (spec: "Orchestrator-hardcoded constants" — never user-configurable).
const HARDCODED_FLAGS: &[&str] = &["--no-wizard", "--quiet", "--no-color", "--ISO-8601"];

fn push_opt(args: &mut Vec<OsString>, flag: &str, value: &Option<String>) {
    if let Some(v) = value {
        if !v.is_empty() {
            args.push(os(flag));
            args.push(os(v));
        }
    }
}

fn push_flag(args: &mut Vec<OsString>, flag: &str, enabled: bool) {
    if enabled {
        args.push(os(flag));
    }
}

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

pub fn hayabusa_csv_args(
    cfg: &HayabusaConfig,
    input_dir: &Path,
    output_file: &Path,
) -> Vec<OsString> {
    let mut args = vec![os("csv-timeline"), os("--directory"), os(input_dir)];
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
        os("json-timeline"),
        os("--directory"),
        os(input_dir),
        os("--JSONL-output"),
    ];
    args.extend(HARDCODED_FLAGS.iter().copied().map(os));
    args.extend(shared_args(cfg));
    args.push(os("--output"));
    args.push(os(output_file));
    args
}

pub fn takajo_automagic_args(
    cfg: &TakajoConfig,
    timeline_jsonl: &Path,
    output_dir: &Path,
) -> Vec<OsString> {
    let mut args = vec![
        os("automagic"),
        os("-t"),
        os(timeline_jsonl),
        os("-o"),
        os(output_dir),
    ];
    push_opt(&mut args, "--level", &cfg.level);
    if cfg.display_table {
        args.push(os("--displayTable"));
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external_config::TakajoConfig;

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
                "csv-timeline",
                "--directory",
                "/ev",
                "--no-wizard",
                "--quiet",
                "--no-color",
                "--ISO-8601",
                "--output",
                "/out/timeline.csv",
            ]
        );
    }

    #[test]
    fn json_args_force_jsonl_output_flag() {
        let args = hayabusa_json_args(&base(), Path::new("/ev"), Path::new("/out/timeline.jsonl"));
        let strs: Vec<String> = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(strs.contains(&"--JSONL-output".to_string()));
        assert_eq!(strs[0], "json-timeline");
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

    #[test]
    fn automagic_args_wire_timeline_input_and_output_dir() {
        let cfg = TakajoConfig::default();
        let args = takajo_automagic_args(
            &cfg,
            Path::new("/out/Hayabusa/timeline.jsonl"),
            Path::new("/out/Takajo"),
        );
        let strs: Vec<String> = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            strs,
            vec![
                "automagic",
                "-t",
                "/out/Hayabusa/timeline.jsonl",
                "-o",
                "/out/Takajo",
            ]
        );
    }

    #[test]
    fn automagic_args_include_level_and_display_table_when_set() {
        let cfg = TakajoConfig {
            level: Some("low".to_string()),
            display_table: true,
            ..Default::default()
        };
        let args = takajo_automagic_args(&cfg, Path::new("/t.jsonl"), Path::new("/o"));
        let strs: Vec<String> = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(strs.windows(2).any(|w| w == ["--level", "low"]));
        assert!(strs.contains(&"--displayTable".to_string()));
    }
}
