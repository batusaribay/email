use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::config::TlsMode;

/// An email client for IMAP and SMTP in the terminal.
///
/// Password authentication works with Gmail app passwords, Fastmail, iCloud,
/// Yahoo, self-hosted IMAP/SMTP, and other providers. Use `email account add`
/// once, then list, read, send, and manage folders from any terminal.
#[derive(Debug, Parser)]
#[command(
    name = "email",
    version,
    about = "An email client for IMAP and SMTP in the terminal",
    long_about = None,
    propagate_version = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Configuration directory (defaults to $XDG_CONFIG_HOME/email).
    #[arg(long, global = true, env = "EMAIL_CONFIG_DIR", value_name = "DIR")]
    pub config_dir: Option<PathBuf>,

    /// Account email to use (defaults to the configured default account).
    #[arg(long, global = true, env = "EMAIL_ACCOUNT", value_name = "EMAIL")]
    pub account: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Add, inspect, test, or remove mail accounts.
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },

    /// List messages in a mailbox.
    #[command(alias = "ls")]
    List(ListArgs),

    /// Search messages with Gmail syntax or IMAP criteria.
    #[command(alias = "find")]
    Search(SearchArgs),

    /// Print a message and its attachment list.
    #[command(alias = "show")]
    Read(ReadArgs),

    /// Send a new message.
    Send(SendArgs),

    /// Reply to a message.
    Reply(ReplyArgs),

    /// List attachments on a message.
    #[command(alias = "files")]
    Attachments(AttachmentsArgs),

    /// Download one attachment from a message.
    Attachment(AttachmentArgs),

    /// List, create, rename, or delete mail folders.
    #[command(alias = "folder")]
    Folders(FolderArgs),

    /// Move messages to another folder.
    Move(MoveArgs),

    /// Remove messages from the inbox.
    Archive(IdsArgs),

    /// Move messages to the trash folder.
    Trash(IdsArgs),

    /// Set read or starred flags on messages.
    Mark(MarkArgs),
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum AccountCommand {
    /// Add or update an account.
    Add(AddAccountArgs),

    /// List configured accounts.
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },

    /// Check IMAP and SMTP connectivity.
    Test {
        /// Account to test (defaults to --account or the default account).
        email: Option<String>,

        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },

    /// Change the default account.
    Default { email: String },

    /// Update the stored password for an account.
    Password {
        /// Account to update (defaults to --account or the default account).
        email: Option<String>,
    },

    /// Remove an account from the local configuration.
    Remove {
        /// Account to remove (defaults to --account or the default account).
        email: Option<String>,
    },
}

#[derive(Debug, Args, Clone)]
pub struct AddAccountArgs {
    /// Full email address.
    #[arg(short = 'e', long, value_name = "EMAIL")]
    pub email: String,

    /// Provider preset (defaults from the email domain).
    #[arg(
        long,
        value_name = "PROVIDER",
        value_parser = ["gmail", "outlook", "yahoo", "icloud", "fastmail", "custom"]
    )]
    pub preset: Option<String>,

    /// Display name used in the From header.
    #[arg(long, value_name = "NAME")]
    pub display_name: Option<String>,

    /// IMAP/SMTP login name (defaults to the email address).
    #[arg(long, value_name = "USER")]
    pub username: Option<String>,

    /// IMAP server hostname.
    #[arg(long, value_name = "HOST")]
    pub imap_host: Option<String>,

    /// IMAP server port.
    #[arg(long, value_name = "PORT")]
    pub imap_port: Option<u16>,

    /// IMAP encryption mode.
    #[arg(long, value_enum, value_name = "MODE")]
    pub imap_tls: Option<TlsMode>,

    /// SMTP server hostname.
    #[arg(long, value_name = "HOST")]
    pub smtp_host: Option<String>,

    /// SMTP server port.
    #[arg(long, value_name = "PORT")]
    pub smtp_port: Option<u16>,

    /// SMTP encryption mode.
    #[arg(long, value_enum, value_name = "MODE")]
    pub smtp_tls: Option<TlsMode>,

    /// Read the password from stdin instead of prompting.
    #[arg(long)]
    pub password_stdin: bool,

    /// Do not store the password in the OS keyring; prompt or use EMAIL_PASSWORD later.
    #[arg(long)]
    pub no_save_password: bool,

    /// Override the INBOX mailbox name.
    #[arg(long, value_name = "MAILBOX")]
    pub inbox_mailbox: Option<String>,

    /// Override the sent mailbox name.
    #[arg(long, value_name = "MAILBOX")]
    pub sent_mailbox: Option<String>,

    /// Override the drafts mailbox name.
    #[arg(long, value_name = "MAILBOX")]
    pub drafts_mailbox: Option<String>,

    /// Override the trash mailbox name.
    #[arg(long, value_name = "MAILBOX")]
    pub trash_mailbox: Option<String>,

    /// Override the junk/spam mailbox name.
    #[arg(long, value_name = "MAILBOX")]
    pub junk_mailbox: Option<String>,

    /// Override the archive mailbox name.
    #[arg(long, value_name = "MAILBOX")]
    pub archive_mailbox: Option<String>,

    /// Override the all-mail mailbox name.
    #[arg(long, value_name = "MAILBOX")]
    pub all_mailbox: Option<String>,
}

#[derive(Debug, Args, Clone)]
pub struct ListArgs {
    /// Mailbox to list (defaults to INBOX).
    #[arg(short = 'm', long, value_name = "MAILBOX")]
    pub mailbox: Option<String>,

    /// Maximum number of messages to return (1-500).
    #[arg(
        short = 'n',
        long,
        default_value_t = 20,
        value_parser = clap::value_parser!(u32).range(1..=500),
        value_name = "N"
    )]
    pub limit: u32,

    /// Only unread messages.
    #[arg(short = 'u', long)]
    pub unread: bool,

    /// Only starred messages.
    #[arg(long)]
    pub starred: bool,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

#[derive(Debug, Args, Clone)]
pub struct SearchArgs {
    /// Gmail query on Gmail accounts, or raw IMAP SEARCH criteria elsewhere.
    #[arg(num_args = 0.., value_name = "QUERY")]
    pub query: Vec<String>,

    /// Match the From header.
    #[arg(long, value_name = "TEXT")]
    pub from: Option<String>,

    /// Match the Subject header.
    #[arg(long, value_name = "TEXT")]
    pub subject: Option<String>,

    /// Messages on or after this date (YYYY-MM-DD).
    #[arg(long, value_name = "DATE")]
    pub since: Option<String>,

    /// Messages before this date (YYYY-MM-DD).
    #[arg(long, value_name = "DATE")]
    pub before: Option<String>,

    /// Messages larger than this many bytes.
    #[arg(long, value_name = "BYTES")]
    pub larger: Option<u64>,

    /// Mailbox to search (defaults to INBOX, or All Mail for Gmail queries).
    #[arg(short = 'm', long, value_name = "MAILBOX")]
    pub mailbox: Option<String>,

    /// Maximum number of messages to return (1-500).
    #[arg(
        short = 'n',
        long,
        default_value_t = 20,
        value_parser = clap::value_parser!(u32).range(1..=500),
        value_name = "N"
    )]
    pub limit: u32,

    /// Only unread messages.
    #[arg(short = 'u', long)]
    pub unread: bool,

    /// Only starred messages.
    #[arg(long)]
    pub starred: bool,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

#[derive(Debug, Args, Clone)]
pub struct ReadArgs {
    /// Message UID.
    pub uid: u32,

    /// Mailbox containing the message (defaults to the last list/search mailbox, then INBOX).
    #[arg(short = 'm', long, value_name = "MAILBOX")]
    pub mailbox: Option<String>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = ReadFormat::Text)]
    pub format: ReadFormat,

    /// Print the HTML body instead of the plain-text body.
    #[arg(long)]
    pub html: bool,

    /// Print only the headers and attachment list (text mode).
    #[arg(long)]
    pub headers_only: bool,

    /// Do not mark the message as read after displaying it.
    #[arg(long)]
    pub no_mark_read: bool,
}

#[derive(Debug, Args, Clone)]
pub struct SendArgs {
    /// Recipient(s), comma separated.
    #[arg(short = 't', long, value_name = "ADDRESSES")]
    pub to: String,

    /// Cc recipient(s), comma separated.
    #[arg(long, value_name = "ADDRESSES")]
    pub cc: Option<String>,

    /// Bcc recipient(s), comma separated.
    #[arg(long, value_name = "ADDRESSES")]
    pub bcc: Option<String>,

    /// Subject line.
    #[arg(short = 's', long, value_name = "SUBJECT")]
    pub subject: Option<String>,

    /// Message body, inline.
    #[arg(short = 'b', long, value_name = "TEXT", conflicts_with = "body_file")]
    pub body: Option<String>,

    /// Read the message body from a file; use '-' for standard input.
    #[arg(long, value_name = "FILE", conflicts_with = "body")]
    pub body_file: Option<String>,

    /// Treat the body as HTML.
    #[arg(long)]
    pub html: bool,

    /// Attach a file (repeatable).
    #[arg(short = 'a', long, value_name = "FILE")]
    pub attach: Vec<PathBuf>,

    /// Print the generated MIME message instead of sending it.
    #[arg(long)]
    pub dry_run: bool,

    /// Output format for the result.
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

#[derive(Debug, Args, Clone)]
pub struct ReplyArgs {
    /// Message UID to reply to.
    pub uid: u32,

    /// Mailbox containing the message (defaults to the last list/search mailbox, then INBOX).
    #[arg(short = 'm', long, value_name = "MAILBOX")]
    pub mailbox: Option<String>,

    /// Reply to all original To and Cc recipients.
    #[arg(long)]
    pub all: bool,

    /// Override the generated subject.
    #[arg(short = 's', long, value_name = "SUBJECT")]
    pub subject: Option<String>,

    /// Message body, inline.
    #[arg(short = 'b', long, value_name = "TEXT", conflicts_with = "body_file")]
    pub body: Option<String>,

    /// Read the message body from a file; use '-' for standard input.
    #[arg(long, value_name = "FILE", conflicts_with = "body")]
    pub body_file: Option<String>,

    /// Treat the body as HTML.
    #[arg(long)]
    pub html: bool,

    /// Attach a file (repeatable).
    #[arg(short = 'a', long, value_name = "FILE")]
    pub attach: Vec<PathBuf>,

    /// Print the generated MIME message instead of sending it.
    #[arg(long)]
    pub dry_run: bool,

    /// Output format for the result.
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

#[derive(Debug, Args, Clone)]
pub struct AttachmentsArgs {
    /// Message UID.
    pub uid: u32,

    /// Mailbox containing the message (defaults to the last list/search mailbox, then INBOX).
    #[arg(short = 'm', long, value_name = "MAILBOX")]
    pub mailbox: Option<String>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

#[derive(Debug, Args, Clone)]
pub struct AttachmentArgs {
    /// Message UID.
    pub uid: u32,

    /// Attachment number from `email attachments` (1-based).
    pub attachment: usize,

    /// Mailbox containing the message (defaults to the last list/search mailbox, then INBOX).
    #[arg(short = 'm', long, value_name = "MAILBOX")]
    pub mailbox: Option<String>,

    /// Output path (defaults to the attachment filename; '-' writes to stdout).
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Overwrite an existing output file.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args, Clone)]
pub struct FolderArgs {
    #[command(subcommand)]
    pub command: Option<FolderCommand>,

    /// Output format for the folder list.
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

#[derive(Debug, Clone, Subcommand)]
pub enum FolderCommand {
    /// List folders.
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },

    /// Create a folder (Gmail: creates a label).
    Create { name: String },

    /// Delete a folder.
    Delete { name: String },

    /// Rename a folder.
    Rename { old: String, new: String },
}

#[derive(Debug, Args, Clone)]
pub struct MoveArgs {
    /// Message UIDs (space or comma separated).
    #[arg(required = true, num_args = 1.., value_delimiter = ',', value_name = "UID")]
    pub uids: Vec<u32>,

    /// Destination mailbox.
    #[arg(short = 't', long = "to", value_name = "MAILBOX")]
    pub target: String,

    /// Source mailbox (defaults to INBOX).
    #[arg(short = 'm', long, value_name = "MAILBOX")]
    pub mailbox: Option<String>,
}

#[derive(Debug, Args, Clone)]
pub struct IdsArgs {
    /// Message UIDs (space or comma separated).
    #[arg(required = true, num_args = 1.., value_delimiter = ',', value_name = "UID")]
    pub uids: Vec<u32>,

    /// Source mailbox (defaults to INBOX).
    #[arg(short = 'm', long, value_name = "MAILBOX")]
    pub mailbox: Option<String>,
}

#[derive(Debug, Args, Clone)]
#[command(group(
    clap::ArgGroup::new("state")
        .required(true)
        .multiple(false)
        .args(["read", "unread", "star", "unstar"])
))]
pub struct MarkArgs {
    /// Message UIDs (space or comma separated).
    #[arg(required = true, num_args = 1.., value_delimiter = ',', value_name = "UID")]
    pub uids: Vec<u32>,

    /// Source mailbox (defaults to the last list/search mailbox, then INBOX).
    #[arg(short = 'm', long, value_name = "MAILBOX")]
    pub mailbox: Option<String>,

    /// Mark as read.
    #[arg(long)]
    pub read: bool,

    /// Mark as unread.
    #[arg(long)]
    pub unread: bool,

    /// Add a star.
    #[arg(long)]
    pub star: bool,

    /// Remove a star.
    #[arg(long)]
    pub unstar: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
    Plain,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum ReadFormat {
    #[default]
    Text,
    Json,
    Raw,
}
