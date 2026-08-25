use std::path::PathBuf;
use triage_core::error::TriageError;

/// Common parameters shared by every binary (spec sections 3.2 and 3.3).
/// Tools embed this with `#[command(flatten)]`.
#[derive(Debug, clap::Args)]
pub struct CommonArgs {
    /// Velociraptor capture root or artifact directory to search recursively
    #[arg(short = 'd', long, conflicts_with = "files")]
    pub directory: Option<PathBuf>,

    /// Explicit artifact file (repeatable)
    #[arg(short = 'f', long = "file")]
    pub files: Vec<PathBuf>,

    /// Write Zimmerman-compatible CSV output beneath this directory
    #[arg(long)]
    pub csv: Option<PathBuf>,

    /// Write Zimmerman-compatible JSON output beneath this directory
    #[arg(long)]
    pub json: Option<PathBuf>,

    /// Override the default CSV basename
    #[arg(long)]
    pub csvf: Option<String>,

    /// Override the default JSON basename
    #[arg(long)]
    pub jsonf: Option<String>,

    /// Pretty-print JSON (whitespace only; ignored for NDJSON-framed tools)
    #[arg(long)]
    pub pretty: bool,

    /// Allow replacement of existing output files
    #[arg(long)]
    pub overwrite: bool,

    /// Preserve the legacy nested output layout under <root>/<ToolName>/<identity>/
    #[arg(long)]
    pub nested_output: bool,

    /// Emit debug-level diagnostics to stderr
    #[arg(long)]
    pub debug: bool,

    /// Emit trace-level diagnostics to stderr (implies --debug)
    #[arg(long)]
    pub trace: bool,

    /// Suppress per-file informational messages (progress and summary remain)
    #[arg(short = 'q', long)]
    pub quiet: bool,
}

/// Cross-argument validation clap cannot express (spec 3.2/3.3).
pub fn validate(args: &CommonArgs) -> Result<(), TriageError> {
    if args.directory.is_none() && args.files.is_empty() {
        return Err(TriageError::Usage(
            "one of --directory or --file is required".into(),
        ));
    }
    if args.csv.is_none() && args.json.is_none() {
        return Err(TriageError::Usage(
            "at least one of --csv or --json is required".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> CommonArgs {
        CommonArgs {
            directory: None,
            files: vec![],
            csv: None,
            json: None,
            csvf: None,
            jsonf: None,
            pretty: false,
            overwrite: false,
            nested_output: false,
            debug: false,
            trace: false,
            quiet: false,
        }
    }

    #[test]
    fn requires_an_input() {
        assert!(validate(&base()).is_err());
    }

    #[test]
    fn requires_an_output() {
        let mut a = base();
        a.directory = Some("x".into());
        assert!(validate(&a).is_err());
        a.csv = Some("out".into());
        assert!(validate(&a).is_ok());
    }

    #[test]
    fn file_mode_with_json_only_is_valid() {
        let mut a = base();
        a.files = vec!["a.stub".into()];
        a.json = Some("out".into());
        assert!(validate(&a).is_ok());
    }
}
