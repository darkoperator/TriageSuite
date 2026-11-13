//! The command line, as clap parses it. Tasks that add commands add a
//! variant here and a function in `cmd/`.

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use oxl_engine::DEFAULT_LIMIT;
use std::path::PathBuf;

/// `query --timeout` when not given, and the most it may be: the engine's.
pub const QUERY_SECONDS_DEFAULT: u64 = oxl_engine::SQL_TIME_LIMIT.as_secs();
pub const QUERY_SECONDS_MAX: u64 = oxl_engine::SQL_TIME_LIMIT_MAX.as_secs();

#[derive(Parser, Debug)]
#[command(
    name = "oxidelens",
    version,
    disable_help_subcommand = true,
    about = "Work an OxideLens case: load TriageSuite output, query it, and record findings"
)]
pub struct Cli {
    /// The case file (required by every command except `capabilities`; `case new` takes it when given no path).
    #[arg(long, global = true, env = "OXIDELENS_CASE", hide_env_values = true)]
    pub case: Option<PathBuf>,
    /// Who is acting: analyst:<name> or agent:<name>.
    #[arg(
        long = "as",
        global = true,
        env = "OXIDELENS_ACTOR",
        hide_env_values = true
    )]
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
    /// List relations, or describe one relation's columns.
    Schema {
        /// The relation to describe; every relation is listed without it.
        relation: Option<String>,
    },
    /// Show a page of a relation's rows.
    Rows {
        relation: String,
        #[command(flatten)]
        query: QueryArgs,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Count the rows a query selects.
    Count {
        relation: String,
        #[command(flatten)]
        query: QueryArgs,
    },
    /// Group rows by one level of a column list.
    Groups {
        relation: String,
        /// Columns to group by, outermost first.
        #[arg(long, value_delimiter = ',', required = true)]
        by: Vec<String>,
        /// The keys of the levels above, as a JSON array of strings and nulls.
        #[arg(long)]
        parent: Option<String>,
        #[command(flatten)]
        query: QueryArgs,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Count rows per time bucket.
    Histogram {
        relation: String,
        /// The timestamp column; the relation's primary time when absent.
        #[arg(long)]
        column: Option<String>,
        #[arg(long, default_value_t = 60)]
        buckets: u32,
        #[command(flatten)]
        query: QueryArgs,
    },
    /// Count distinct value combinations across hosts.
    Stack {
        relation: String,
        #[arg(long, value_delimiter = ',', required = true)]
        by: Vec<String>,
        #[arg(long, value_enum, default_value_t = StackOrderArg::Rarest)]
        order: StackOrderArg,
        #[command(flatten)]
        query: QueryArgs,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Find a value in every relation.
    Find(FindArgs),
    /// Count (or list) what happened around a row or a moment.
    Around(AroundArgs),
    /// Show one row in full.
    Row { id: i64 },
    /// Mark rows suspicious, clear marks, or list them.
    #[command(subcommand)]
    Mark(MarkCmd),
    /// Tag rows, remove tags, or list the tags.
    #[command(subcommand)]
    Tag(TagCmd),
    /// Write, delete or list notes on a row.
    #[command(subcommand)]
    Note(NoteCmd),
    /// Declare a column's type or a relation's primary time.
    #[command(subcommand)]
    Column(ColumnCmd),
    /// List the changes made to the case, newest first.
    History {
        /// Only this kind of target: source, relation, row_mark, row_tag, note, case or
        /// saved_view.
        #[arg(long)]
        kind: Option<String>,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Run one SELECT over the case.
    Query {
        /// The SQL, or @file to read it from a file.
        sql: String,
        /// Seconds the query may run before it is stopped, 1 to 600.
        #[arg(long, default_value_t = QUERY_SECONDS_DEFAULT,
              value_parser = clap::value_parser!(u64).range(1..=QUERY_SECONDS_MAX))]
        timeout: u64,
        #[command(flatten)]
        page: PageArgs,
    },
}

/// What narrows a relation: shared by every command that reads one.
#[derive(Args, Debug)]
pub struct QueryArgs {
    /// A filter document as JSON, or @file.
    #[arg(long)]
    pub filter: Option<String>,
    /// Search every column for this text.
    #[arg(long)]
    pub search: Option<String>,
    /// Read the search text as a regular expression.
    #[arg(long)]
    pub regex: bool,
    #[arg(long)]
    pub case_sensitive: bool,
    /// Restrict to a host; repeat for several.
    #[arg(long = "host")]
    pub hosts: Vec<String>,
    /// Sort key as column or column:desc; repeat for several.
    #[arg(long)]
    pub sort: Vec<String>,
    /// The columns to show, comma-separated.
    #[arg(long, value_delimiter = ',')]
    pub columns: Option<Vec<String>>,
}

#[derive(Args, Debug)]
pub struct FindArgs {
    pub value: String,
    #[arg(long, value_enum, default_value_t = FindModeArg::Exact)]
    pub mode: FindModeArg,
    /// Restrict to a host; repeat for several.
    #[arg(long = "host")]
    pub hosts: Vec<String>,
    /// Earliest UTC time.
    #[arg(long)]
    pub from: Option<String>,
    /// Latest UTC time.
    #[arg(long)]
    pub to: Option<String>,
    /// Return the matching rows, merged and in time order, not the summary.
    #[arg(long)]
    pub rows: bool,
    #[command(flatten)]
    pub page: RowsPageArgs,
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("anchor").required(true).args(["row", "host"])))]
pub struct AroundArgs {
    /// Centre on this row's host and time.
    #[arg(long, conflicts_with_all = ["host", "at"])]
    pub row: Option<i64>,
    #[arg(long, requires = "at")]
    pub host: Option<String>,
    /// A UTC time.
    #[arg(long, requires = "host")]
    pub at: Option<String>,
    /// Half-width of the window: an integer then s, m or h; at most 24h.
    #[arg(long)]
    pub window: String,
    /// Return the merged rows of the window, not the per-relation counts.
    #[arg(long)]
    pub rows: bool,
    #[command(flatten)]
    pub page: RowsPageArgs,
}

#[derive(Args, Debug)]
pub struct PageArgs {
    /// Rows to return, at least 1; the engine enforces the maximum.
    #[arg(long, default_value_t = DEFAULT_LIMIT, value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: u32,
    #[arg(long, default_value_t = 0)]
    pub offset: u64,
}

/// `--limit` and `--offset` of a command that pages only with `--rows`: given
/// without it they would change nothing, and a silent no-op hides a mistake.
#[derive(Args, Debug)]
pub struct RowsPageArgs {
    /// Rows to return, at least 1; needs --rows.
    #[arg(long, requires = "rows", value_parser = clap::value_parser!(u32).range(1..))]
    limit: Option<u32>,
    /// Rows to skip; needs --rows.
    #[arg(long, requires = "rows")]
    offset: Option<u64>,
}

impl RowsPageArgs {
    /// The page asked for, the defaults where a flag was left out.
    pub fn page(&self) -> PageArgs {
        PageArgs {
            limit: self.limit.unwrap_or(DEFAULT_LIMIT),
            offset: self.offset.unwrap_or(0),
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum StackOrderArg {
    Rarest,
    Commonest,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum FindModeArg {
    Exact,
    Contains,
}

#[derive(Subcommand, Debug)]
pub enum CaseCmd {
    /// Create a new case file.
    New {
        /// The new case file; --case or OXIDELENS_CASE when left out.
        path: Option<PathBuf>,
        #[arg(long)]
        name: String,
    },
    /// Show the case's name, format and creator, and its size.
    Info,
    /// Write a copy of the case without the space removed sources left
    /// free; the case itself is left as it is.
    Compact {
        /// The new case file to write; it must not exist.
        out: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
pub enum LoadCmd {
    /// Load a TriageSuite run output root.
    Run {
        out_root: PathBuf,
        /// Also load EvtxTriage's per-channel copies of the event log
        /// (Individual/<channel>), which repeat every event of its
        /// aggregate and are left out by default.
        #[arg(long)]
        per_channel: bool,
    },
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

/// The rows a batch command acts on.
#[derive(Args, Debug)]
pub struct RowsArg {
    /// Row ids, comma-separated, or @file holding them separated by commas,
    /// spaces or new lines.
    #[arg(long)]
    pub rows: String,
}

#[derive(Subcommand, Debug)]
pub enum MarkCmd {
    /// Mark rows suspicious.
    Set(RowsArg),
    /// Clear the mark on rows.
    Clear(RowsArg),
    /// List marked rows.
    List {
        /// Only marks set by this actor (analyst:<name> or agent:<name>).
        #[arg(long)]
        by: Option<String>,
        #[command(flatten)]
        page: PageArgs,
    },
}

#[derive(Subcommand, Debug)]
pub enum TagCmd {
    /// Add a tag to rows.
    Add {
        #[command(flatten)]
        rows: RowsArg,
        #[arg(long)]
        tag: String,
    },
    /// Remove a tag from rows.
    Remove {
        #[command(flatten)]
        rows: RowsArg,
        #[arg(long)]
        tag: String,
    },
    /// List the tags with the rows carrying each.
    List {
        #[command(flatten)]
        page: PageArgs,
    },
}

#[derive(Subcommand, Debug)]
pub enum NoteCmd {
    /// Write a note on a row.
    Add {
        #[arg(long)]
        row: i64,
        /// The note's text, or @file to read it from a file.
        #[arg(long)]
        text: String,
    },
    /// Delete a note.
    Delete { id: i64 },
    /// List the notes on a row.
    List {
        #[arg(long)]
        row: i64,
        #[command(flatten)]
        page: PageArgs,
    },
}

#[derive(Subcommand, Debug)]
pub enum ColumnCmd {
    /// Type a column (analyst only).
    Declare {
        relation: String,
        column: String,
        #[arg(long = "type", value_enum)]
        sql_type: SqlTypeArg,
        /// Required for a TIMESTAMP, and only for one: utc or unknown.
        #[arg(long, value_enum)]
        semantics: Option<SemanticsArg>,
    },
    /// Set the timestamp column a relation's timeline runs on, or clear it
    /// (analyst only).
    PrimaryTime {
        relation: String,
        #[arg(required_unless_present = "none", conflicts_with = "none")]
        column: Option<String>,
        /// Clear the primary time.
        #[arg(long)]
        none: bool,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum SqlTypeArg {
    #[value(name = "TIMESTAMP", alias = "timestamp")]
    Timestamp,
    #[value(name = "BIGINT", alias = "bigint")]
    BigInt,
    #[value(name = "UBIGINT", alias = "ubigint")]
    UBigInt,
    #[value(name = "BOOLEAN", alias = "boolean")]
    Boolean,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum SemanticsArg {
    Utc,
    Unknown,
}

impl Command {
    /// The name the envelope reports for this command.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Case(CaseCmd::New { .. }) => "case new",
            Command::Case(CaseCmd::Info) => "case info",
            Command::Case(CaseCmd::Compact { .. }) => "case compact",
            Command::Load(LoadCmd::Run { .. }) => "load run",
            Command::Load(LoadCmd::File { .. }) => "load file",
            Command::Sources => "sources",
            Command::Source(SourceCmd::Remove { .. }) => "source remove",
            Command::Capabilities => "capabilities",
            Command::Schema { .. } => "schema",
            Command::Rows { .. } => "rows",
            Command::Count { .. } => "count",
            Command::Groups { .. } => "groups",
            Command::Histogram { .. } => "histogram",
            Command::Stack { .. } => "stack",
            Command::Find(_) => "find",
            Command::Around(_) => "around",
            Command::Row { .. } => "row",
            Command::Query { .. } => "query",
            Command::Mark(MarkCmd::Set(_)) => "mark set",
            Command::Mark(MarkCmd::Clear(_)) => "mark clear",
            Command::Mark(MarkCmd::List { .. }) => "mark list",
            Command::Tag(TagCmd::Add { .. }) => "tag add",
            Command::Tag(TagCmd::Remove { .. }) => "tag remove",
            Command::Tag(TagCmd::List { .. }) => "tag list",
            Command::Note(NoteCmd::Add { .. }) => "note add",
            Command::Note(NoteCmd::Delete { .. }) => "note delete",
            Command::Note(NoteCmd::List { .. }) => "note list",
            Command::Column(ColumnCmd::Declare { .. }) => "column declare",
            Command::Column(ColumnCmd::PrimaryTime { .. }) => "column primary-time",
            Command::History { .. } => "history",
        }
    }
}
