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
    /// Work the case's timeline entries: list, show, write, review, attach evidence.
    #[command(subcommand)]
    Entry(EntryCmd),
    /// List the rows attached to an entry as evidence.
    #[command(subcommand)]
    Evidence(EvidenceCmd),
    /// Work the case's host records.
    #[command(subcommand)]
    Host(HostCmd),
    /// Work the case's account records, and read an account's logons.
    #[command(subcommand)]
    Account(AccountCmd),
    /// Work the case's IOC records, and read an IOC's sightings.
    #[command(subcommand)]
    Ioc(IocCmd),
    /// Link an entry to a host, account or IOC record, or remove the link.
    #[command(subcommand)]
    Link(LinkCmd),
    /// The accounts and IOCs a row names, for the analyst to record.
    Candidates {
        /// The row to read.
        #[arg(long)]
        row: i64,
    },
    /// Write the timeline, a workbook or a report to a file.
    #[command(subcommand)]
    Export(ExportCmd),
    /// List the changes made to the case, newest first.
    History {
        /// Only this kind of target: source, relation, row_mark, row_tag, note, case,
        /// saved_view, entry, evidence, host, account, ioc or link.
        #[arg(long)]
        kind: Option<String>,
        /// Only this target's changes; needs --kind. An entry, host, account or IOC by
        /// its id (12 or E-12); an evidence link as <entry>:<row>.
        #[arg(long, requires = "kind")]
        id: Option<String>,
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
            Command::Entry(c) => match c {
                EntryCmd::List { .. } => "entry list",
                EntryCmd::Show { .. } => "entry show",
                EntryCmd::Add { .. } => "entry add",
                EntryCmd::Edit { .. } => "entry edit",
                EntryCmd::Status { .. } => "entry status",
                EntryCmd::Delete { .. } => "entry delete",
                EntryCmd::Promote(_) => "entry promote",
                EntryCmd::Attach { .. } => "entry attach",
                EntryCmd::Detach { .. } => "entry detach",
            },
            Command::Evidence(EvidenceCmd::List { .. }) => "evidence list",
            Command::Host(c) => match c {
                HostCmd::List { .. } => "host list",
                HostCmd::Show { .. } => "host show",
                HostCmd::Add { .. } => "host add",
                HostCmd::Edit { .. } => "host edit",
                HostCmd::Delete { .. } => "host delete",
                HostCmd::Merge { .. } => "host merge",
                HostCmd::Status { .. } => "host status",
            },
            Command::Account(c) => match c {
                AccountCmd::List { .. } => "account list",
                AccountCmd::Show { .. } => "account show",
                AccountCmd::Add { .. } => "account add",
                AccountCmd::Edit { .. } => "account edit",
                AccountCmd::Delete { .. } => "account delete",
                AccountCmd::Merge { .. } => "account merge",
                AccountCmd::Status { .. } => "account status",
                AccountCmd::Logons { .. } => "account logons",
            },
            Command::Ioc(c) => match c {
                IocCmd::List { .. } => "ioc list",
                IocCmd::Show { .. } => "ioc show",
                IocCmd::Add { .. } => "ioc add",
                IocCmd::Edit { .. } => "ioc edit",
                IocCmd::Delete { .. } => "ioc delete",
                IocCmd::Merge { .. } => "ioc merge",
                IocCmd::Status { .. } => "ioc status",
                IocCmd::Sightings { .. } => "ioc sightings",
            },
            Command::Link(LinkCmd::Add(_)) => "link add",
            Command::Link(LinkCmd::Remove(_)) => "link remove",
            Command::Candidates { .. } => "candidates",
            Command::Export(c) => match c {
                ExportCmd::Timeline { .. } => "export timeline",
                ExportCmd::Workbook { .. } => "export workbook",
                ExportCmd::Report { .. } => "export report",
            },
        }
    }
}

/// `12` or `<letter>-12` (an id as the case shows it: `E-12`) as 12.
fn shown_id(text: &str, prefix: char) -> Result<i64, String> {
    let digits = match text.split_once('-') {
        Some((p, rest)) if p.eq_ignore_ascii_case(&prefix.to_string()) => rest,
        _ => text,
    };
    let shown = || text.chars().take(40).collect::<String>();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "an id is a number, or {prefix}- and a number; not {:?}",
            shown()
        ));
    }
    digits
        .parse::<i64>()
        .map_err(|_| format!("{:?} is too large for an id", shown()))
}

/// The history target id for `text` as typed: an entry, host, account or IOC
/// by its shown id (`E-12`) is its number, anything else is left as it is.
pub fn target_id(kind: &str, text: &str) -> String {
    let prefix = match kind {
        "entry" => 'E',
        "host" => 'H',
        "account" => 'A',
        "ioc" => 'I',
        _ => return text.to_string(),
    };
    shown_id(text, prefix).map_or_else(|_| text.to_string(), |n| n.to_string())
}

/// The id of a record of `kind` as typed: `12`, or `H-12` for a host.
pub fn record_id_of(kind: oxl_engine::RecordKind, text: &str) -> oxl_engine::Result<i64> {
    let prefix = kind.prefix().chars().next().unwrap_or('?');
    shown_id(text, prefix).map_err(oxl_engine::EngineError::Invalid)
}

fn entry_id(text: &str) -> Result<i64, String> {
    shown_id(text, 'E')
}
fn host_id(text: &str) -> Result<i64, String> {
    shown_id(text, 'H')
}
fn account_id(text: &str) -> Result<i64, String> {
    shown_id(text, 'A')
}
fn ioc_id(text: &str) -> Result<i64, String> {
    shown_id(text, 'I')
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum EntryStatusArg {
    Draft,
    Unreviewed,
    Reviewed,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum RecordStatusArg {
    Unreviewed,
    Reviewed,
}

#[derive(Subcommand, Debug)]
pub enum EntryCmd {
    /// List the live entries in the timeline's order (by time, undated last).
    List {
        /// Only entries with this status; repeat for several.
        #[arg(long = "status", value_enum)]
        status: Vec<EntryStatusArg>,
        /// Only entries linked to the host record of this name.
        #[arg(long)]
        host: Option<String>,
        /// Only entries with this ATT&CK tactic.
        #[arg(long)]
        tactic: Option<String>,
        /// Only entries whose title or description contains this text.
        #[arg(long)]
        text: Option<String>,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Show one entry and a page of its evidence.
    Show {
        #[arg(value_parser = entry_id)]
        id: i64,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Write a new entry.
    Add {
        #[arg(long)]
        title: String,
        /// The description, or @file to read it from a file.
        #[arg(long)]
        description: Option<String>,
        /// A UTC time, YYYY-MM-DDTHH:MM:SS[.f]Z; none leaves the entry undated.
        #[arg(long)]
        time: Option<String>,
        /// One of the fourteen ATT&CK Enterprise tactics.
        #[arg(long)]
        tactic: Option<String>,
        /// ATT&CK technique ids, comma-separated (T1059 or T1059.001).
        #[arg(long = "technique", value_delimiter = ',')]
        techniques: Vec<String>,
    },
    /// Change the fields given; the others stay as they are.
    Edit {
        #[arg(value_parser = entry_id)]
        id: i64,
        #[arg(long)]
        title: Option<String>,
        /// The description, or @file to read it from a file.
        #[arg(long)]
        description: Option<String>,
        /// A UTC time; an empty value clears it.
        #[arg(long)]
        time: Option<String>,
        /// A tactic; an empty value clears it.
        #[arg(long)]
        tactic: Option<String>,
        /// Technique ids, comma-separated; they replace the entry's.
        #[arg(long = "technique", value_delimiter = ',')]
        techniques: Vec<String>,
        /// Remove every technique id.
        #[arg(long, conflicts_with = "techniques")]
        clear_techniques: bool,
    },
    /// Set an entry's status (analyst only).
    Status {
        #[arg(value_parser = entry_id)]
        id: i64,
        #[arg(value_enum)]
        status: EntryStatusArg,
    },
    /// Delete an entry; its history stays.
    Delete {
        #[arg(value_parser = entry_id)]
        id: i64,
    },
    /// Make an entry from each row, as M does in the app.
    Promote(RowsArg),
    /// Attach rows to an entry as evidence.
    Attach {
        #[arg(value_parser = entry_id)]
        id: i64,
        #[command(flatten)]
        rows: RowsArg,
    },
    /// Detach a row from an entry.
    Detach {
        #[arg(value_parser = entry_id)]
        id: i64,
        #[arg(long)]
        row: i64,
    },
}

#[derive(Subcommand, Debug)]
pub enum EvidenceCmd {
    /// The rows attached to an entry, each compared with the case now.
    List {
        #[arg(long, value_parser = entry_id)]
        entry: i64,
        #[command(flatten)]
        page: PageArgs,
    },
}

#[derive(Subcommand, Debug)]
pub enum HostCmd {
    /// List the live host records.
    List {
        #[command(flatten)]
        page: PageArgs,
    },
    /// Show one host record and a page of the entries linked to it.
    Show {
        #[arg(value_parser = host_id)]
        id: i64,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Write a new host record.
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        role: Option<String>,
        /// The note, or @file to read it from a file.
        #[arg(long)]
        note: Option<String>,
    },
    /// Change the fields given; the others stay as they are.
    Edit {
        #[arg(value_parser = host_id)]
        id: i64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        role: Option<String>,
        /// The note, or @file to read it from a file.
        #[arg(long)]
        note: Option<String>,
    },
    /// Delete a host record; its links to entries are removed.
    Delete {
        #[arg(value_parser = host_id)]
        id: i64,
    },
    /// Merge one host record into another; the first is deleted.
    Merge {
        #[arg(long, value_parser = host_id)]
        from: i64,
        #[arg(long, value_parser = host_id)]
        into: i64,
    },
    /// Set a host record's status (analyst only).
    Status {
        #[arg(value_parser = host_id)]
        id: i64,
        #[arg(value_enum)]
        status: RecordStatusArg,
    },
}

#[derive(Subcommand, Debug)]
pub enum AccountCmd {
    /// List the live account records.
    List {
        #[command(flatten)]
        page: PageArgs,
    },
    /// Show one account record and a page of the entries linked to it.
    Show {
        #[arg(value_parser = account_id)]
        id: i64,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Write a new account record.
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        sid: Option<String>,
        /// The description, or @file to read it from a file.
        #[arg(long)]
        description: Option<String>,
        /// standard, local_admin, domain_admin, service or unknown (the default).
        #[arg(long)]
        privilege: Option<String>,
        /// The note, or @file to read it from a file.
        #[arg(long)]
        note: Option<String>,
    },
    /// Change the fields given; the others stay as they are. An empty
    /// --domain or --sid clears it.
    Edit {
        #[arg(value_parser = account_id)]
        id: i64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        sid: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        privilege: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Delete an account record; its links to entries are removed.
    Delete {
        #[arg(value_parser = account_id)]
        id: i64,
    },
    /// Merge one account record into another; the first is deleted.
    Merge {
        #[arg(long, value_parser = account_id)]
        from: i64,
        #[arg(long, value_parser = account_id)]
        into: i64,
    },
    /// Set an account record's status (analyst only).
    Status {
        #[arg(value_parser = account_id)]
        id: i64,
        #[arg(value_enum)]
        status: RecordStatusArg,
    },
    /// Count the account's Security 4624 and 4625 logons by host and logon type.
    Logons {
        #[arg(long, value_parser = account_id)]
        id: i64,
    },
}

#[derive(Subcommand, Debug)]
pub enum IocCmd {
    /// List the live IOC records.
    List {
        /// Only this type: ip, domain, hostname, url, email, md5, sha1, sha256, file_name,
        /// file_path, registry_key, service, scheduled_task or other.
        #[arg(long = "type")]
        ioc_type: Option<String>,
        /// Only IOCs of this tool.
        #[arg(long)]
        tool: Option<String>,
        /// Only IOCs whose value, tool or note contains this text.
        #[arg(long)]
        text: Option<String>,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Show one IOC record and a page of the entries linked to it.
    Show {
        #[arg(value_parser = ioc_id)]
        id: i64,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Write a new IOC record.
    Add {
        #[arg(long = "type")]
        ioc_type: String,
        #[arg(long)]
        value: String,
        #[arg(long)]
        tool: Option<String>,
        /// ATT&CK technique ids, comma-separated.
        #[arg(long = "attack-id", value_delimiter = ',')]
        attack_ids: Vec<String>,
        /// The note, or @file to read it from a file.
        #[arg(long)]
        note: Option<String>,
    },
    /// Change the fields given; the others stay as they are. An empty --tool clears it.
    Edit {
        #[arg(value_parser = ioc_id)]
        id: i64,
        #[arg(long = "type")]
        ioc_type: Option<String>,
        #[arg(long)]
        value: Option<String>,
        #[arg(long)]
        tool: Option<String>,
        /// ATT&CK technique ids, comma-separated; they replace the IOC's.
        #[arg(long = "attack-id", value_delimiter = ',')]
        attack_ids: Vec<String>,
        /// Remove every ATT&CK id.
        #[arg(long, conflicts_with = "attack_ids")]
        clear_attack_ids: bool,
        #[arg(long)]
        note: Option<String>,
    },
    /// Delete an IOC record; its links to entries are removed.
    Delete {
        #[arg(value_parser = ioc_id)]
        id: i64,
    },
    /// Merge one IOC record into another; the first is deleted.
    Merge {
        #[arg(long, value_parser = ioc_id)]
        from: i64,
        #[arg(long, value_parser = ioc_id)]
        into: i64,
    },
    /// Set an IOC record's status (analyst only).
    Status {
        #[arg(value_parser = ioc_id)]
        id: i64,
        #[arg(value_enum)]
        status: RecordStatusArg,
    },
    /// Where the IOC's value occurs in the loaded data (Find, exact).
    Sightings {
        #[arg(long, value_parser = ioc_id)]
        id: i64,
    },
}

/// Which link `link add` or `link remove` acts on.
#[derive(Args, Debug)]
pub struct LinkArgs {
    #[arg(long, value_parser = entry_id)]
    pub entry: i64,
    /// host, account or ioc.
    #[arg(long)]
    pub kind: String,
    /// The record's id, as the case shows it (12 or H-12, A-12, I-12).
    #[arg(long)]
    pub id: String,
}

#[derive(Subcommand, Debug)]
pub enum LinkCmd {
    /// Link an entry to a record.
    Add(LinkArgs),
    /// Remove the link from an entry to a record.
    Remove(LinkArgs),
}

#[derive(Subcommand, Debug)]
pub enum ExportCmd {
    /// The live entries as CSV, in the timeline's order.
    Timeline {
        /// The file to write; it is made new, never over a folder or the case.
        path: PathBuf,
    },
    /// The timeline, hosts, accounts and IOCs as an XLSX workbook, every cell a string.
    Workbook { path: PathBuf },
    /// The timeline, hosts, accounts and IOCs as a Markdown report with evidence text escaped.
    Report { path: PathBuf },
}
