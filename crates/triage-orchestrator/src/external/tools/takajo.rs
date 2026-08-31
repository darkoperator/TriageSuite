use super::hayabusa::JSONL_SLOT;
use crate::external::args::{os, push_opt};
use crate::external::config::{ConfigError, ResolvedConfig, TakajoConfig};
use crate::external::tool::{
    Artifacts, ExternalTool, HostContext, Invocation, OutputDirPolicy, OutputSpec, Requirement,
};
use std::ffi::OsString;
use std::path::Path;

/// Output subdirectory under each host's output root.
const DIR: &str = "Takajo";

pub struct Takajo;

impl ExternalTool for Takajo {
    fn key(&self) -> &'static str {
        "takajo"
    }

    fn enabled(&self, cfg: &ResolvedConfig) -> bool {
        cfg.takajo.enabled
    }

    fn disable(&self, cfg: &mut ResolvedConfig) {
        cfg.takajo.enabled = false;
    }

    fn bin<'a>(&self, cfg: &'a ResolvedConfig) -> &'a str {
        &cfg.takajo.bin
    }

    /// Rejected at config-load time rather than skipped per host: this
    /// combination can never be satisfied by any capture, so failing before any
    /// evidence is touched is strictly more useful than failing once per host.
    fn validate(&self, cfg: &ResolvedConfig) -> Result<(), ConfigError> {
        if cfg.takajo.enabled && !cfg.hayabusa.json {
            return Err(ConfigError(
                "takajo.enabled = true requires hayabusa.json = true \
                 (takajo automagic needs Hayabusa's JSONL output)"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Takajo never touches raw evidence — it only post-processes Hayabusa's
    /// JSONL timeline. Declaring that as a requirement, rather than checking it
    /// inside `plan`, is what makes the missing-prerequisite report take
    /// precedence over "binary not found on PATH".
    fn requires(&self) -> Option<Requirement> {
        Some(Requirement {
            slot: JSONL_SLOT,
            report_name: "takajo-automagic",
            skipped_message: "skipped: hayabusa did not produce a JSONL timeline for this host",
        })
    }

    fn plan(
        &self,
        cfg: &ResolvedConfig,
        ctx: &HostContext<'_>,
        prior: &Artifacts,
    ) -> Vec<Invocation> {
        let jsonl = prior
            .get(JSONL_SLOT)
            .expect("the requires() gate runs before plan() and guarantees this slot");
        let dir = ctx.host_dir.join(DIR);
        vec![Invocation {
            report_name: "takajo-automagic",
            args: takajo_automagic_args(&cfg.takajo, jsonl, &dir),
            work_dir: dir.clone(),
            // `automagic -o` creates the leaf itself and refuses to run if it
            // already exists, so only the parent may be pre-created.
            dir_policy: OutputDirPolicy::ToolCreatesLeaf,
            outputs: OutputSpec::Path(dir),
            publishes: None,
        }]
    }
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
