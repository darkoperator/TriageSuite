//! The command line, as clap parses it. Tasks that add commands add a
//! variant here and a function in `cmd/`.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "oxidelens",
    version,
    about = "Work an OxideLens case: load TriageSuite output, query it, and record findings"
)]
pub struct Cli {
    /// The case file (required by every command except `capabilities` and `case new`).
    #[arg(long, global = true, env = "OXIDELENS_CASE")]
    pub case: Option<PathBuf>,
    /// Who is acting: analyst:<name> or agent:<name>.
    #[arg(long = "as", global = true, env = "OXIDELENS_ACTOR")]
    pub actor: Option<String>,
    /// Compact one-line JSON instead of pretty-printed.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a case or show its details.
    #[command(subcommand)]
    Case(CaseCmd),
    /// Load a run's output or a loose file into the case.
    #[command(subcommand)]
    Load(LoadCmd),
    /// List every source loaded into the case.
    Sources,
    /// Act on one source.
    #[command(subcommand)]
    Source(SourceCmd),
    /// Print the version, command list and limits. Needs no case.
    Capabilities,
}

#[derive(Subcommand, Debug)]
pub enum CaseCmd {
    /// Create a new case file.
    New {
        path: PathBuf,
        #[arg(long)]
        name: String,
    },
    /// Show the case's name, format and creator.
    Info,
}

#[derive(Subcommand, Debug)]
pub enum LoadCmd {
    /// Load a TriageSuite run output root.
    Run { out_root: PathBuf },
    /// Load a loose CSV or TSV file.
    File {
        path: PathBuf,
        /// The relation's name; derived from the file name when absent.
        #[arg(long)]
        name: Option<String>,
        /// The host the file's rows describe.
        #[arg(long)]
        host: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SourceCmd {
    /// Remove a source (analyst only).
    Remove {
        id: i64,
        /// Remove even when annotations on its rows will be orphaned.
        #[arg(long)]
        confirm_orphans: bool,
    },
}

impl Command {
    /// The name the envelope reports for this command.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Case(CaseCmd::New { .. }) => "case new",
            Command::Case(CaseCmd::Info) => "case info",
            Command::Load(LoadCmd::Run { .. }) => "load run",
            Command::Load(LoadCmd::File { .. }) => "load file",
            Command::Sources => "sources",
            Command::Source(SourceCmd::Remove { .. }) => "source remove",
            Command::Capabilities => "capabilities",
        }
    }
}
