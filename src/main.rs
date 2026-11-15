mod cli;
mod config;
mod imap;
mod mime;
mod model;
mod presets;
mod render;
mod secrets;
mod smtp;

use std::collections::HashSet;
use std::env;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, FromArgMatches};
use tokio::io::AsyncReadExt;

use crate::cli::{
    AccountCommand, AddAccountArgs, AttachmentArgs, AttachmentsArgs, Cli, Command, FolderCommand,
    IdsArgs, ListArgs, MarkArgs, MoveArgs, OutputFormat, ReadArgs, ReadFormat, ReplyArgs,
    SearchArgs, SendArgs,
};
use crate::config::{
    Account, Config, Mailboxes, Server, config_path, expand_tilde, load_config, resolve_config_dir,
    save_config,
};
use crate::imap::SearchOptions;
use crate::mime::OutgoingMessage;
use crate::presets::Preset;

#[tokio::main]
async fn main() {
    sigpipe::reset();
    if let Err(error) = run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = parse_cli();
    let config_dir = resolve_config_dir(cli.config_dir.as_deref())?;
    let mut config = load_config(&config_dir)?;
    migrate_legacy_passwords(&config_dir, &mut config)?;
    let global_account = cli.account.clone();

    match cli.command {
        Command::Account { command } => {
            handle_account(&config_dir, &mut config, global_account.as_deref(), command).await
        }

        Command::Send(args) => {
            let account = config
                .find(global_account.as_deref())
                .context("could not select a mail account")?
                .clone();
            if args.dry_run {
                send_message(&account, None, args).await
            } else {
                let password = resolve_password(&config_dir, &mut config, &account)?;
                send_message(&account, Some(password.as_str()), args).await
            }
        }

        command => {
            let account = config
                .find(global_account.as_deref())
                .context("could not select a mail account")?
                .clone();
            let password = resolve_password(&config_dir, &mut config, &account)?;
            let inbox = account.mailboxes.inbox.clone();
            match command {
                Command::List(args) => {
                    let format = args.format;
                    let page = list_messages(&account, &password, args, inbox).await?;
                    remember_mailbox(&config_dir, &mut config, &account, &page.mailbox);
                    render::print_page(&page.mailbox, &page.messages, format)
                }
                Command::Search(args) => {
                    let format = args.format;
                    let page = search_messages(&account, &password, args, inbox).await?;
                    remember_mailbox(&config_dir, &mut config, &account, &page.mailbox);
                    render::print_page(&page.mailbox, &page.messages, format)
                }
                Command::Read(args) => read_message(&account, &password, args, inbox).await,
                Command::Reply(args) => reply_message(&account, &password, args, inbox).await,
                Command::Attachments(args) => attachments(&account, &password, args, inbox).await,
                Command::Attachment(args) => {
                    download_attachment(&account, &password, args, inbox).await
                }
                Command::Folders(args) => folders(&account, &password, args).await,
                Command::Move(args) => move_messages(&account, &password, args, inbox).await,
                Command::Archive(args) => archive(&account, &password, args, inbox).await,
                Command::Trash(args) => trash(&account, &password, args, inbox).await,
                Command::Mark(args) => mark(&account, &password, args, inbox).await,
                Command::Account { .. } | Command::Send(_) => unreachable!("handled above"),
            }
        }
    }
}

fn parse_cli() -> Cli {
    let command = Cli::command()
        .styles(clap::builder::Styles::plain())
        .color(clap::ColorChoice::Never);
    let matches = command.get_matches();
    Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit())
}

fn migrate_legacy_passwords(config_dir: &Path, config: &mut Config) -> Result<()> {
    let mut migrated = 0_usize;
    let mut failed = 0_usize;
    for account in &mut config.accounts {
        let Some(legacy) = account.password.take() else {
            continue;
        };
        match secrets::get_password(&account.email) {
            Ok(Some(_)) => {
                account.password_saved = true;
                migrated += 1;
            }
            Ok(None) => match secrets::store_password(&account.email, &legacy) {
                Ok(()) => {
                    account.password_saved = true;
                    migrated += 1;
                }
                Err(error) => {
                    eprintln!(
                        "warning: could not move the password for {} into the OS keyring: {error:#}",
                        account.email
                    );
                    account.password = Some(legacy);
                    failed += 1;
                }
            },
            Err(error) => {
                eprintln!(
                    "warning: could not read the OS keyring for {}: {error:#}",
                    account.email
                );
                account.password = Some(legacy);
                failed += 1;
            }
        }
    }
    if migrated > 0 && failed == 0 {
        save_config(config_dir, config)?;
        eprintln!("Moved {migrated} password(s) from config.json to the OS keyring.");
    }
    Ok(())
}

async fn handle_account(
    config_dir: &Path,
    config: &mut Config,
    global_account: Option<&str>,
    command: AccountCommand,
) -> Result<()> {
    match command {
        AccountCommand::Add(args) => add_account(config_dir, config, args),
        AccountCommand::List { format } => render::print_accounts(config, format),
        AccountCommand::Test { email, format } => {
            let account = select_account(config, email.as_deref().or(global_account))?.clone();
            let password = resolve_password(config_dir, &mut *config, &account)?;
            let imap_result = imap::test(&account, &password).await?;
            let smtp_result = smtp::test(&account, &password).await;
            match smtp_result {
                Ok(ok) => render::print_test(&account, &imap_result, ok, format),
                Err(error) => {
                    render::print_test(&account, &imap_result, false, format)?;
                    Err(error)
                }
            }
        }
        AccountCommand::Default { email } => {
            config.set_default(&email)?;
            save_config(config_dir, config)?;
            println!("Default account is now {email}.");
            Ok(())
        }
        AccountCommand::Password { email } => {
            let selected_account =
                select_account(config, email.as_deref().or(global_account))?.clone();
            let password = clean_password(
                &prompt_password(&selected_account.email)?,
                selected_account.is_gmail(),
            );
            secrets::store_password(&selected_account.email, &password)?;
            let selected = selected_account.email;
            if let Some(account) = config.find_mut(&selected) {
                account.password = None;
                account.password_saved = true;
            }
            save_config(config_dir, config)?;
            println!("Stored the password for {selected} in the OS keyring.");
            Ok(())
        }
        AccountCommand::Remove { email } => {
            let selected = select_account(config, email.as_deref().or(global_account))?
                .email
                .clone();
            let removed = config.remove(&selected);
            if removed.is_some() {
                if let Err(error) = secrets::delete_password(&selected) {
                    eprintln!("warning: could not remove the keyring entry: {error:#}");
                }
            }
            save_config(config_dir, config)?;
            if removed.is_some() {
                println!("Removed {selected}.");
            } else {
                println!("Account {selected} was not configured.");
            }
            Ok(())
        }
    }
}

fn add_account(config_dir: &Path, config: &mut Config, args: AddAccountArgs) -> Result<()> {
    let email = args.email.trim().to_string();
    if !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        bail!("'{email}' is not a valid email address");
    }
    let preset = match args.preset.as_deref() {
        Some(name) => {
            Preset::parse(name).ok_or_else(|| anyhow::anyhow!("unknown preset '{name}'"))?
        }
        None => Preset::detect(&email),
    };
    let defaults = preset.defaults();

    let imap_tls = args.imap_tls.unwrap_or(defaults.imap.tls);
    let smtp_tls = args.smtp_tls.unwrap_or(defaults.smtp.tls);
    let imap_port = args.imap_port.unwrap_or_else(|| {
        if imap_tls == defaults.imap.tls {
            defaults.imap.port
        } else {
            imap_tls.default_imap_port()
        }
    });
    let smtp_port = args.smtp_port.unwrap_or_else(|| {
        if smtp_tls == defaults.smtp.tls {
            defaults.smtp.port
        } else {
            smtp_tls.default_smtp_port()
        }
    });
    let imap_host = args.imap_host.unwrap_or_else(|| defaults.imap.host.clone());
    let smtp_host = args.smtp_host.unwrap_or_else(|| defaults.smtp.host.clone());
    if imap_host.trim().is_empty() || smtp_host.trim().is_empty() {
        bail!("custom accounts need --imap-host and --smtp-host (or choose a known --preset)");
    }

    let mut mailboxes: Mailboxes = defaults.mailboxes;
    if let Some(value) = args.inbox_mailbox {
        mailboxes.inbox = value;
    }
    if let Some(value) = args.sent_mailbox {
        mailboxes.sent = value;
    }
    if let Some(value) = args.drafts_mailbox {
        mailboxes.drafts = value;
    }
    if let Some(value) = args.trash_mailbox {
        mailboxes.trash = value;
    }
    if let Some(value) = args.junk_mailbox {
        mailboxes.junk = value;
    }
    if let Some(value) = args.archive_mailbox {
        mailboxes.archive = value;
    }
    if let Some(value) = args.all_mailbox {
        mailboxes.all = value;
    }

    let password = if args.no_save_password {
        None
    } else if args.password_stdin {
        let mut value = String::new();
        std::io::stdin()
            .read_line(&mut value)
            .context("could not read password from stdin")?;
        Some(clean_password(&value, preset == Preset::Gmail))
    } else if let Ok(value) = env::var("MAIL_CLIENT_PASSWORD") {
        if value.is_empty() {
            Some(clean_password(
                &prompt_password(&email)?,
                preset == Preset::Gmail,
            ))
        } else {
            Some(clean_password(&value, preset == Preset::Gmail))
        }
    } else {
        Some(clean_password(
            &prompt_password(&email)?,
            preset == Preset::Gmail,
        ))
    };
    if password.as_deref().is_some_and(str::is_empty) {
        bail!("password must not be empty");
    }

    let mut password_saved = false;
    if !args.no_save_password {
        if let Some(password) = password.as_deref() {
            secrets::store_password(&email, password)?;
            password_saved = true;
        }
    }

    let account = Account {
        email: email.clone(),
        display_name: args.display_name.unwrap_or_default(),
        username: args.username.unwrap_or_else(|| email.clone()),
        password: None,
        password_saved,
        last_mailbox: None,
        preset: preset.name().to_string(),
        imap: Server::new(imap_host.trim(), imap_port, imap_tls),
        smtp: Server::new(smtp_host.trim(), smtp_port, smtp_tls),
        mailboxes,
    };

    let was_default = config.default_account.is_none();
    config.upsert(account.clone());
    if was_default {
        config.default_account = Some(email.clone());
    }
    save_config(config_dir, config)?;

    println!("Account: {email} ({})", preset.name());
    println!(
        "IMAP:    {}",
        config.find(Some(email.as_str()))?.imap.display()
    );
    println!(
        "SMTP:    {}",
        config.find(Some(email.as_str()))?.smtp.display()
    );
    if args.no_save_password {
        println!(
            "Password: not saved; set MAIL_CLIENT_PASSWORD or run `mail-client account password {email}`."
        );
    } else {
        println!("Password: stored in the OS keyring.");
    }
    println!("Config:  {}", config_path(config_dir).display());
    if !imap_tls.is_secure() || !smtp_tls.is_secure() {
        eprintln!(
            "Warning: one or both connections are unencrypted. Use TLS unless this is a local relay."
        );
    }
    eprintln!("{}", defaults.note);
    println!("Next: mail-client account test {email}");
    Ok(())
}

fn select_account<'a>(config: &'a Config, selector: Option<&str>) -> Result<&'a Account> {
    config.find(selector)
}

fn resolve_password(config_dir: &Path, config: &mut Config, account: &Account) -> Result<String> {
    if let Ok(value) = env::var("MAIL_CLIENT_PASSWORD") {
        if !value.is_empty() {
            return Ok(clean_password(&value, account.is_gmail()));
        }
    }
    match secrets::get_password(&account.email) {
        Ok(Some(password)) if !password.is_empty() => {
            return Ok(clean_password(&password, account.is_gmail()));
        }
        Ok(_) => {}
        Err(error) => {
            if account.password.is_none() {
                return Err(error);
            }
            eprintln!(
                "warning: OS keyring unavailable; using the legacy password for this session"
            );
        }
    }
    if let Some(password) = account.password.as_deref() {
        if !password.is_empty() {
            return Ok(clean_password(password, account.is_gmail()));
        }
    }
    if std::io::stdin().is_terminal() {
        let password = clean_password(&prompt_password(&account.email)?, account.is_gmail());
        match secrets::store_password(&account.email, &password) {
            Ok(()) => {
                if let Some(saved) = config.find_mut(&account.email) {
                    saved.password_saved = true;
                }
                if let Err(error) = save_config(config_dir, config) {
                    eprintln!(
                        "warning: could not update the config after saving the password: {error:#}"
                    );
                } else {
                    eprintln!("Saved password in the OS keyring.");
                }
            }
            Err(error) => {
                eprintln!("warning: could not store the password in the OS keyring: {error:#}");
            }
        }
        Ok(password)
    } else {
        let mut message = format!(
            "no password available for {}. Set MAIL_CLIENT_PASSWORD or store one with \
             `mail-client account password {}`.",
            account.email, account.email
        );
        if account.is_gmail() {
            message.push_str(
                "\nGmail requires a 16-character app password from \
                 https://myaccount.google.com/apppasswords",
            );
        }
        bail!("{message}")
    }
}

fn prompt_password(email: &str) -> Result<String> {
    if !std::io::stdin().is_terminal() {
        bail!(
            "cannot prompt for a password without a terminal; use MAIL_CLIENT_PASSWORD \
             or --password-stdin"
        );
    }
    let password = rpassword::prompt_password(format!("Password for {email} (input hidden): "))
        .context("could not read password")?;
    if password.is_empty() {
        bail!("password must not be empty");
    }
    Ok(password)
}

fn clean_password(value: &str, gmail: bool) -> String {
    let value = value.trim_end_matches(['\r', '\n']);
    if gmail {
        value
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect()
    } else {
        value.to_string()
    }
}

fn remember_mailbox(config_dir: &Path, config: &mut Config, account: &Account, mailbox: &str) {
    let changed = if let Some(saved) = config.find_mut(&account.email) {
        if saved.last_mailbox.as_deref() == Some(mailbox) {
            false
        } else {
            saved.last_mailbox = Some(mailbox.to_string());
            true
        }
    } else {
        false
    };
    if changed {
        if let Err(error) = save_config(config_dir, config) {
            eprintln!("warning: could not remember the current mailbox: {error:#}");
        }
    }
}

async fn list_messages(
    account: &Account,
    password: &str,
    args: ListArgs,
    inbox: String,
) -> Result<imap::SearchResult> {
    let mailbox = args.mailbox.unwrap_or(inbox);
    imap::search_messages(
        account,
        password,
        SearchOptions {
            mailbox,
            limit: args.limit,
            unread: args.unread,
            starred: args.starred,
            ..SearchOptions::default()
        },
    )
    .await
}

async fn search_messages(
    account: &Account,
    password: &str,
    args: SearchArgs,
    inbox: String,
) -> Result<imap::SearchResult> {
    let query = args.query.join(" ");
    let query = (!query.trim().is_empty()).then_some(query);
    let mailbox = match args.mailbox {
        Some(mailbox) => mailbox,
        None if account.is_gmail() => {
            if account.mailboxes.all.trim().is_empty() {
                inbox
            } else {
                account.mailboxes.all.clone()
            }
        }
        None => inbox,
    };
    imap::search_messages(
        account,
        password,
        SearchOptions {
            query,
            from: args.from,
            subject: args.subject,
            since: args.since,
            before: args.before,
            unread: args.unread,
            starred: args.starred,
            larger: args.larger,
            mailbox,
            limit: args.limit,
        },
    )
    .await
}

async fn read_message(
    account: &Account,
    password: &str,
    args: ReadArgs,
    inbox: String,
) -> Result<()> {
    let mailbox = args
        .mailbox
        .or_else(|| account.last_mailbox.clone())
        .unwrap_or(inbox);
    if args.format == ReadFormat::Raw {
        let bytes =
            imap::fetch_raw(account, password, &mailbox, args.uid, !args.no_mark_read).await?;
        return write_stdout(&bytes);
    }
    let message =
        imap::fetch_message(account, password, &mailbox, args.uid, !args.no_mark_read).await?;
    render::print_message(
        &message,
        output_for_read(args.format),
        args.html,
        args.headers_only,
    )
}

async fn send_message(account: &Account, password: Option<&str>, args: SendArgs) -> Result<()> {
    let body = resolve_body(args.body.as_deref(), args.body_file.as_deref()).await?;
    let attachments = expand_paths(args.attach);
    let message = OutgoingMessage {
        from_name: account.display_name.clone(),
        from_email: account.email.clone(),
        to: args.to,
        cc: args.cc,
        bcc: args.bcc,
        subject: args.subject.unwrap_or_default(),
        body,
        html: args.html,
        attachments,
        in_reply_to: None,
        references: None,
    };
    let built = mime::build_message(&message)?;
    if args.dry_run {
        return write_stdout(&built.bytes);
    }
    let password =
        password.ok_or_else(|| anyhow::anyhow!("a password is required to send mail"))?;
    let result = smtp::send(account, password, &message, &built).await?;
    render::print_send(&result, args.format)
}

async fn reply_message(
    account: &Account,
    password: &str,
    args: ReplyArgs,
    inbox: String,
) -> Result<()> {
    let mailbox = args
        .mailbox
        .or_else(|| account.last_mailbox.clone())
        .unwrap_or(inbox);
    let original = imap::fetch_message(account, password, &mailbox, args.uid, true).await?;
    let recipient = if original.reply_to.trim().is_empty() {
        original.from.trim()
    } else {
        original.reply_to.trim()
    };
    if recipient.is_empty() {
        bail!("message {} has no From or Reply-To header", args.uid);
    }

    let mut cc = None;
    if args.all {
        let mut seen = HashSet::new();
        seen.insert(extract_email(recipient));
        seen.insert(account.email.to_ascii_lowercase());
        let mut parts = Vec::new();
        for address in split_addresses(&original.to)
            .into_iter()
            .chain(split_addresses(&original.cc))
        {
            let email = extract_email(&address);
            if email.is_empty() || seen.contains(&email) {
                continue;
            }
            seen.insert(email);
            parts.push(address);
        }
        if !parts.is_empty() {
            cc = Some(parts.join(", "));
        }
    }

    let subject = args.subject.unwrap_or_else(|| {
        if original
            .subject
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("re:")
        {
            original.subject.clone()
        } else {
            format!("Re: {}", original.subject)
        }
    });
    let references = match (
        original.message_id.as_deref(),
        original.references.as_deref(),
    ) {
        (Some(message_id), Some(references)) => Some(format!("{references} {message_id}")),
        (Some(message_id), None) => Some(message_id.to_string()),
        _ => None,
    };
    let in_reply_to = original.message_id.clone();
    let body = resolve_body(args.body.as_deref(), args.body_file.as_deref()).await?;
    let attachments = expand_paths(args.attach);
    let outgoing = OutgoingMessage {
        from_name: account.display_name.clone(),
        from_email: account.email.clone(),
        to: recipient.to_string(),
        cc,
        bcc: None,
        subject,
        body,
        html: args.html,
        attachments,
        in_reply_to,
        references,
    };
    let built = mime::build_message(&outgoing)?;
    if args.dry_run {
        return write_stdout(&built.bytes);
    }
    let result = smtp::send(account, password, &outgoing, &built).await?;
    render::print_send(&result, args.format)
}

async fn attachments(
    account: &Account,
    password: &str,
    args: AttachmentsArgs,
    inbox: String,
) -> Result<()> {
    let mailbox = args
        .mailbox
        .or_else(|| account.last_mailbox.clone())
        .unwrap_or(inbox);
    let message = imap::fetch_message(account, password, &mailbox, args.uid, false).await?;
    render::print_attachments(&message, args.format)
}

async fn download_attachment(
    account: &Account,
    password: &str,
    args: AttachmentArgs,
    inbox: String,
) -> Result<()> {
    let mailbox = args
        .mailbox
        .or_else(|| account.last_mailbox.clone())
        .unwrap_or(inbox);
    let (attachment, bytes) =
        imap::download_attachment(account, password, &mailbox, args.uid, args.attachment).await?;
    if args.output.as_deref() == Some(Path::new("-")) {
        return write_stdout(&bytes);
    }
    let path = args
        .output
        .map(|path| expand_tilde(&path))
        .unwrap_or_else(|| PathBuf::from(safe_filename(&attachment.filename)));
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    if path.exists() && !args.force {
        bail!(
            "{} already exists; pass --force to overwrite it",
            path.display()
        );
    }
    if path.is_dir() {
        bail!("{} is a directory", path.display());
    }
    tokio::fs::write(&path, &bytes)
        .await
        .with_context(|| format!("could not write {}", path.display()))?;
    render::print_saved_file(&path.display().to_string(), bytes.len() as u64);
    Ok(())
}

async fn folders(account: &Account, password: &str, args: cli::FolderArgs) -> Result<()> {
    match args.command {
        None => {
            let folders = imap::folders(account, password).await?;
            render::print_folders(&folders, args.format)
        }
        Some(FolderCommand::List { format }) => {
            let folders = imap::folders(account, password).await?;
            render::print_folders(&folders, format)
        }
        Some(FolderCommand::Create { name }) => {
            imap::create_folder(account, password, &name).await?;
            println!("Created folder '{name}'.");
            Ok(())
        }
        Some(FolderCommand::Delete { name }) => {
            imap::delete_folder(account, password, &name).await?;
            println!("Deleted folder '{name}'.");
            Ok(())
        }
        Some(FolderCommand::Rename { old, new }) => {
            imap::rename_folder(account, password, &old, &new).await?;
            println!("Renamed folder '{old}' to '{new}'.");
            Ok(())
        }
    }
}

async fn move_messages(
    account: &Account,
    password: &str,
    args: MoveArgs,
    inbox: String,
) -> Result<()> {
    let mailbox = args.mailbox.unwrap_or(inbox);
    imap::move_messages(account, password, &mailbox, &args.uids, &args.target).await?;
    println!(
        "Moved {} message{} to '{}'.",
        args.uids.len(),
        plural(args.uids.len()),
        args.target
    );
    Ok(())
}

async fn archive(account: &Account, password: &str, args: IdsArgs, inbox: String) -> Result<()> {
    let mailbox = args.mailbox.unwrap_or(inbox);
    imap::archive(account, password, &mailbox, &args.uids).await?;
    println!(
        "Archived {} message{}.",
        args.uids.len(),
        plural(args.uids.len())
    );
    Ok(())
}

async fn trash(account: &Account, password: &str, args: IdsArgs, inbox: String) -> Result<()> {
    let mailbox = args.mailbox.unwrap_or(inbox);
    imap::trash(account, password, &mailbox, &args.uids).await?;
    println!(
        "Moved {} message{} to the trash.",
        args.uids.len(),
        plural(args.uids.len())
    );
    Ok(())
}

async fn mark(account: &Account, password: &str, args: MarkArgs, inbox: String) -> Result<()> {
    let mailbox = args
        .mailbox
        .or_else(|| account.last_mailbox.clone())
        .unwrap_or(inbox);
    let (add, remove, description): (Vec<&'static str>, Vec<&'static str>, &str) = if args.read {
        (Vec::new(), vec!["\\Seen"], "read")
    } else if args.unread {
        (vec!["\\Seen"], Vec::new(), "unread")
    } else if args.star {
        (vec!["\\Flagged"], Vec::new(), "starred")
    } else {
        (Vec::new(), vec!["\\Flagged"], "unstarred")
    };
    imap::set_flags(account, password, &mailbox, &args.uids, &add, &remove).await?;
    println!(
        "Marked {} message{} as {description}.",
        args.uids.len(),
        plural(args.uids.len())
    );
    Ok(())
}

fn output_for_read(format: ReadFormat) -> OutputFormat {
    match format {
        ReadFormat::Text => OutputFormat::Table,
        ReadFormat::Json => OutputFormat::Json,
        ReadFormat::Raw => OutputFormat::Table,
    }
}

async fn resolve_body(body: Option<&str>, body_file: Option<&str>) -> Result<String> {
    if let Some(body) = body {
        return Ok(body.to_string());
    }
    if let Some(file) = body_file {
        if file == "-" {
            return read_stdin().await;
        }
        let path = expand_tilde(Path::new(file));
        return tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("could not read body file {}", path.display()));
    }
    if !std::io::stdin().is_terminal() {
        let body = read_stdin().await?;
        if body.is_empty() {
            bail!("no message body received on standard input");
        }
        return Ok(body);
    }
    bail!(
        "a message body is required; pass --body TEXT or --body-file FILE \
         (use --body-file - or pipe text to stdin)"
    )
}

async fn read_stdin() -> Result<String> {
    let mut body = String::new();
    tokio::io::stdin()
        .read_to_string(&mut body)
        .await
        .context("could not read standard input")?;
    Ok(body)
}

fn expand_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.into_iter().map(|path| expand_tilde(&path)).collect()
}

fn safe_filename(filename: &str) -> String {
    let name = Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment")
        .chars()
        .map(|character| {
            if character == '/' || character == '\\' || character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    if name.is_empty() || name == "." || name == ".." {
        "attachment".to_string()
    } else {
        name
    }
}

fn split_addresses(value: &str) -> Vec<String> {
    let mut addresses = Vec::new();
    let mut current = String::new();
    let mut angle_depth = 0_u32;
    let mut quote = false;
    for character in value.chars() {
        match character {
            '"' => {
                quote = !quote;
                current.push(character);
            }
            '<' if !quote => {
                angle_depth += 1;
                current.push(character);
            }
            '>' if !quote => {
                angle_depth = angle_depth.saturating_sub(1);
                current.push(character);
            }
            ',' if !quote && angle_depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    addresses.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(character),
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        addresses.push(trimmed.to_string());
    }
    addresses
}

fn extract_email(value: &str) -> String {
    if let (Some(start), Some(end)) = (value.rfind('<'), value.rfind('>')) {
        if end > start {
            return value[start + 1..end].trim().to_ascii_lowercase();
        }
    }
    value.trim().trim_matches('"').to_ascii_lowercase()
}

fn write_stdout(bytes: &[u8]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(bytes)?;
    stdout.flush()?;
    Ok(())
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_addresses_with_quotes_and_angles() {
        let values = split_addresses("\"Doe, Jane\" <jane@example.com>, bob@example.com");
        assert_eq!(values.len(), 2);
        assert_eq!(extract_email(&values[0]), "jane@example.com");
    }

    #[test]
    fn sanitizes_attachment_names() {
        assert_eq!(safe_filename("../../etc/passwd"), "passwd");
        assert_eq!(safe_filename(".."), "attachment");
    }

    #[test]
    fn trims_gmail_app_password_spaces() {
        assert_eq!(
            clean_password("abcd efgh ijkl mnop\n", true),
            "abcdefghijklmnop"
        );
        assert_eq!(clean_password(" pass phrase \n", false), " pass phrase ");
    }
}
