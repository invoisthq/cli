//! The command line: every command, flag and help text.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::output::Format;

const AFTER_HELP: &str = "\
Run `invoist` on its own in a terminal to open the full-screen app.

Sign in once with `invoist login`. Scripts and CI can set INVOIST_API_KEY
instead. Docs: https://invoi.st/docs/cli";

#[derive(Debug, Parser)]
#[command(
    name = "invoist",
    version = crate::version_line(),
    about = "Read invoices, work through checks and see what's due, from your terminal.",
    after_help = AFTER_HELP,
    propagate_version = true,
    max_term_width = 100
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Args)]
pub struct Global {
    /// Use a saved profile other than the default.
    #[arg(
        long,
        global = true,
        help_heading = "Global options",
        env = "INVOIST_PROFILE",
        value_name = "NAME"
    )]
    pub profile: Option<String>,

    /// The API's address, for a test server (also INVOIST_BASE_URL).
    #[arg(
        long,
        global = true,
        help_heading = "Global options",
        value_name = "URL",
        hide_short_help = true
    )]
    pub base_url: Option<String>,

    /// Print JSON instead of a table. Same as `--output json`.
    #[arg(long, global = true, help_heading = "Global options")]
    pub json: bool,

    /// How to print results.
    #[arg(
        long,
        short = 'o',
        global = true,
        help_heading = "Global options",
        value_enum,
        value_name = "FORMAT",
        conflicts_with = "json"
    )]
    pub output: Option<Format>,

    /// Print only the results, no progress or notes.
    #[arg(long, short, global = true, help_heading = "Global options")]
    pub quiet: bool,

    /// Never use colour (also NO_COLOR).
    #[arg(long, global = true, help_heading = "Global options")]
    pub no_color: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Sign in. Opens your browser to approve the CLI in Invoist.
    #[command(
        after_help = "Examples:\n  invoist login\n  invoist login --with-key < key.txt\n  invoist login --profile test --with-key"
    )]
    Login(LoginArgs),
    /// Sign out, and forget the key or sign-in for this profile.
    Logout,
    /// Show who you're signed in as, and whether it still works.
    Status,
    /// Manage saved profiles, such as a test key and a live sign-in.
    #[command(subcommand)]
    Profile(ProfileCmd),

    /// Read invoices: upload files and see what Invoist read.
    #[command(
        after_help = "Examples:\n  invoist extract invoice.pdf\n  invoist extract scans/ --json\n  cat invoice.pdf | invoist extract - --name invoice.pdf"
    )]
    Extract(ExtractArgs),
    /// List, show and download the invoices Invoist has read.
    #[command(subcommand, visible_alias = "docs")]
    Documents(DocumentsCmd),
    /// Save every invoice from a period as CSV or JSON.
    #[command(
        after_help = "Examples:\n  invoist export --since 2026-10-01 --until 2026-10-31 > october.csv\n  invoist export --details --output json > invoices.json"
    )]
    Export(ExportArgs),
    /// The invoices set aside for a person to check.
    #[command(subcommand)]
    Review(ReviewCmd),
    /// Your workspace's categories.
    Categories,
    /// What you've used this billing period.
    Usage,
    /// The overview numbers for your workspace.
    Stats,
    /// Endpoints that Invoist calls when something happens.
    #[command(subcommand)]
    Webhooks(WebhooksCmd),

    /// A household's bills. On its own, shows what's left to pay.
    #[command(
        after_help = "Examples:\n  invoist bills\n  invoist bills list --unpaid\n  invoist bills paid 4f9c2a1e"
    )]
    Bills(BillsArgs),
    /// Emails waiting for your household to approve their sender.
    #[command(name = "held-emails")]
    HeldEmails,

    /// Open the full-screen terminal app.
    Ui,
    /// Print shell completions.
    #[command(
        after_help = "Examples:\n  invoist completions zsh > ~/.zfunc/_invoist\n  invoist completions bash > /etc/bash_completion.d/invoist"
    )]
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Write the man pages.
    Man {
        /// The folder to write them to.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Paste an API key instead of signing in with the browser. Reads the key
    /// from standard input when it isn't a terminal.
    #[arg(long)]
    pub with_key: bool,
    /// Print the sign-in link instead of opening a browser, for example over SSH.
    #[arg(long, conflicts_with = "with_key")]
    pub no_browser: bool,
}

#[derive(Debug, Subcommand)]
pub enum ProfileCmd {
    /// List saved profiles.
    List,
    /// Make a profile the default.
    Use { name: String },
}

#[derive(Debug, Args)]
pub struct ExtractArgs {
    /// Files or folders to read. Folders are searched for PDF, PNG and JPEG
    /// files. Use `-` to read one file from standard input.
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<PathBuf>,
    /// The file name to use for standard input.
    #[arg(long, value_name = "NAME", default_value = "upload.pdf")]
    pub name: String,
    /// Upload and return straight away, without waiting for the result.
    #[arg(long)]
    pub no_wait: bool,
    /// How long to wait for each result, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 120)]
    pub timeout: u64,
    /// A key for one logical upload: resending it never charges twice.
    /// Only with a single file.
    #[arg(long, value_name = "KEY")]
    pub idempotency_key: Option<String>,
    /// Exit with code 5 when an invoice was set aside for a person to check.
    #[arg(long)]
    pub fail_on_review: bool,
    /// How many files to upload at once.
    #[arg(long, value_name = "N", default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=16))]
    pub concurrency: u8,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum StatusFilter {
    /// Uploaded, not started yet.
    Waiting,
    /// Being read now.
    Reading,
    /// Read: the fields are ready.
    Read,
    /// Couldn't be read.
    Failed,
}

impl StatusFilter {
    pub fn to_sdk(self) -> invoist::models::DocumentStatus {
        use invoist::models::DocumentStatus as S;
        match self {
            StatusFilter::Waiting => S::Pending,
            StatusFilter::Reading => S::Processing,
            StatusFilter::Read => S::Done,
            StatusFilter::Failed => S::Failed,
        }
    }
}

#[derive(Debug, Args)]
pub struct Paging {
    /// How many to show.
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=200))]
    pub limit: u32,
    /// Fetch every page, not just the first.
    #[arg(long, conflicts_with = "limit")]
    pub all: bool,
}

#[derive(Debug, Subcommand)]
pub enum DocumentsCmd {
    /// List invoices, newest first.
    #[command(visible_alias = "ls")]
    List {
        /// Only invoices at this stage.
        #[arg(long, value_enum)]
        status: Option<StatusFilter>,
        /// Only invoices set aside for a person to check.
        #[arg(long)]
        flagged: bool,
        /// Search file names, suppliers, invoice numbers and references.
        #[arg(long, short = 's', value_name = "TEXT")]
        search: Option<String>,
        /// A category id, or `none` for invoices without one.
        #[arg(long, value_name = "ID")]
        category: Option<String>,
        #[command(flatten)]
        paging: Paging,
    },
    /// Everything Invoist read from one invoice.
    #[command(after_help = "The id can be the short one shown in lists, such as 4f9c2a1e.")]
    Get { id: String },
    /// Save the original file.
    Download {
        id: String,
        /// Where to save it. Defaults to the original file name; `-` writes to standard output.
        #[arg(long = "out", value_name = "PATH")]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Only invoices added on or after this day (YYYY-MM-DD).
    #[arg(long, value_name = "DATE")]
    pub since: Option<String>,
    /// Only invoices added on or before this day (YYYY-MM-DD).
    #[arg(long, value_name = "DATE")]
    pub until: Option<String>,
    /// Only invoices at this stage.
    #[arg(long, value_enum)]
    pub status: Option<StatusFilter>,
    /// Also fetch each invoice's dates, subtotal and tax (one request per invoice).
    #[arg(long)]
    pub details: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReviewFilter {
    Pending,
    Resolved,
    Dismissed,
    All,
}

#[derive(Debug, Subcommand)]
pub enum ReviewCmd {
    /// List invoices set aside for a person, and why.
    #[command(visible_alias = "ls")]
    List {
        #[arg(long, value_enum, default_value = "pending")]
        status: ReviewFilter,
    },
    /// Mark an item as checked.
    Resolve {
        id: String,
        /// It wasn't a real problem.
        #[arg(long)]
        dismiss: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum WebhooksCmd {
    /// List endpoints, and the events you can send to them.
    #[command(visible_alias = "ls")]
    List,
    /// Start sending events to a URL. Prints its signing secret once.
    Create {
        url: String,
        /// An event to send; repeat for more. Every event when left out.
        #[arg(long = "event", value_name = "EVENT")]
        events: Vec<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Stop sending events to an endpoint.
    Delete {
        id: String,
        /// Don't ask to confirm.
        #[arg(long, short)]
        yes: bool,
    },
    /// Check a saved webhook body against its signature, locally.
    #[command(
        after_help = "Example:\n  INVOIST_WEBHOOK_SECRET=whsec_... invoist webhooks verify body.json --signature 't=...,v1=...'"
    )]
    Verify {
        /// The raw request body; `-` for standard input.
        #[arg(default_value = "-")]
        body: PathBuf,
        /// The X-Invoist-Signature header.
        #[arg(long, value_name = "HEADER")]
        signature: String,
        /// The endpoint's signing secret.
        #[arg(long, env = "INVOIST_WEBHOOK_SECRET", hide_env_values = true)]
        secret: String,
    },
}

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct BillsArgs {
    #[command(subcommand)]
    pub command: Option<BillsCmd>,
    /// How many days ahead count as due soon.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..=366))]
    pub days: u32,
}

#[derive(Debug, Subcommand)]
pub enum BillsCmd {
    /// List bills by month, sender, or paid or not.
    #[command(visible_alias = "ls")]
    List {
        /// Only this month (YYYY-MM).
        #[arg(long, value_name = "MONTH")]
        month: Option<String>,
        /// Only bills still to pay.
        #[arg(long, conflicts_with = "paid")]
        unpaid: bool,
        /// Only bills marked paid.
        #[arg(long)]
        paid: bool,
        #[arg(long, value_name = "NAME")]
        sender: Option<String>,
        #[arg(long, short = 's', value_name = "TEXT")]
        search: Option<String>,
        #[command(flatten)]
        paging: Paging,
    },
    /// Everything about one bill: who to pay, how much, by when.
    Get { id: String },
    /// Save the bill's file.
    Download {
        id: String,
        #[arg(long = "out", value_name = "PATH")]
        out: Option<PathBuf>,
    },
    /// Mark a bill as paid.
    Paid {
        id: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// Mark a bill as not paid again.
    Unpaid {
        id: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// Totals by month and sender.
    Stats {
        #[arg(long, value_name = "MONTH")]
        month: Option<String>,
        #[arg(long, value_name = "NAME")]
        sender: Option<String>,
    },
}

impl Global {
    pub fn format(&self) -> Format {
        if self.json {
            Format::Json
        } else {
            self.output.unwrap_or(Format::Table)
        }
    }
}
