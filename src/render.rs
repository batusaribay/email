use anyhow::Result;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::config::{Account, Config, Mailboxes};
use crate::imap::TestResult;
use crate::model::{Attachment, Folder, MessageSummary, ParsedMessage, SendResult};

pub fn print_page(mailbox: &str, messages: &[MessageSummary], format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "camelCase")]
            struct Page<'a> {
                mailbox: &'a str,
                messages: &'a [MessageSummary],
            }
            print_json(&Page { mailbox, messages })
        }
        OutputFormat::Plain => {
            for message in messages {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    message.uid,
                    cell(&message.mailbox),
                    if message.unread { "unread" } else { "read" },
                    if message.starred { "starred" } else { "" },
                    cell(&message.date),
                    cell(&message.from),
                    cell(&message.subject)
                );
            }
            Ok(())
        }
        OutputFormat::Table => {
            if messages.is_empty() {
                println!("No messages.");
            } else {
                let rows: Vec<Vec<String>> = messages
                    .iter()
                    .map(|message| {
                        vec![
                            message.uid.to_string(),
                            message.date.clone(),
                            sender(&message.from),
                            message.subject.clone(),
                        ]
                    })
                    .collect();
                print_table(&["UID", "DATE", "FROM", "SUBJECT"], &rows, &[0, 16, 28, 60]);
            }
            Ok(())
        }
    }
}

pub fn print_message(
    message: &ParsedMessage,
    format: OutputFormat,
    html: bool,
    headers_only: bool,
) -> Result<()> {
    match format {
        OutputFormat::Json => print_json(message),
        OutputFormat::Plain => {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                message.uid,
                cell(&message.mailbox),
                cell(&message.date),
                cell(&message.from),
                cell(&message.subject)
            );
            if !headers_only {
                println!("{}", sanitize_text(&message.body_text));
            }
            Ok(())
        }
        OutputFormat::Table => {
            print_message_text(message, html, headers_only);
            Ok(())
        }
    }
}

fn print_message_text(message: &ParsedMessage, html: bool, headers_only: bool) {
    if !message.from.is_empty() {
        println!("From:    {}", cell(&message.from));
    }
    if !message.to.is_empty() {
        println!("To:      {}", cell(&message.to));
    }
    if !message.cc.is_empty() {
        println!("Cc:      {}", cell(&message.cc));
    }
    if !message.bcc.is_empty() {
        println!("Bcc:     {}", cell(&message.bcc));
    }
    if !message.reply_to.is_empty() {
        println!("Reply-To: {}", cell(&message.reply_to));
    }
    println!("Date:    {}", cell(&message.date));
    println!("Subject: {}", cell(&message.subject));
    if !message.attachments.is_empty() {
        println!("Attachments:");
        for attachment in &message.attachments {
            println!(
                "  {}. {} ({}, {})",
                attachment.index,
                cell(&attachment.filename),
                size(attachment.size),
                cell(&attachment.mime_type)
            );
        }
        println!(
            "Download: email attachment {} <number> --mailbox '{}'",
            message.uid, message.mailbox
        );
    }
    println!();
    if headers_only {
        return;
    }
    let body = if html {
        message.body_html.as_deref().unwrap_or(&message.body_text)
    } else {
        &message.body_text
    };
    let body = sanitize_text(body);
    if body.trim().is_empty() {
        println!("(no body)");
    } else {
        print!("{body}");
        if !body.ends_with('\n') {
            println!();
        }
    }
}

pub fn print_attachments(message: &ParsedMessage, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "camelCase")]
            struct Output<'a> {
                uid: u32,
                mailbox: &'a str,
                attachments: &'a [Attachment],
            }
            print_json(&Output {
                uid: message.uid,
                mailbox: &message.mailbox,
                attachments: &message.attachments,
            })
        }
        OutputFormat::Plain => {
            for attachment in &message.attachments {
                println!(
                    "{}\t{}\t{}\t{}",
                    attachment.index, attachment.filename, attachment.size, attachment.mime_type
                );
            }
            Ok(())
        }
        OutputFormat::Table => {
            if message.attachments.is_empty() {
                println!("No attachments.");
            } else {
                let rows: Vec<Vec<String>> = message
                    .attachments
                    .iter()
                    .map(|attachment| {
                        vec![
                            attachment.index.to_string(),
                            size(attachment.size),
                            attachment.mime_type.clone(),
                            attachment.filename.clone(),
                        ]
                    })
                    .collect();
                print_table(&["#", "SIZE", "TYPE", "FILENAME"], &rows, &[3, 10, 30, 70]);
                println!();
                println!(
                    "Download with: email attachment {} <number> --mailbox '{}'",
                    message.uid, message.mailbox
                );
            }
            Ok(())
        }
    }
}

pub fn print_folders(folders: &[Folder], format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => print_json(folders),
        OutputFormat::Plain => {
            for folder in folders {
                println!(
                    "{}\t{}\t{}\t{}",
                    folder.name,
                    folder.delimiter.as_deref().unwrap_or(""),
                    folder.attributes.join(","),
                    folder.selectable
                );
            }
            Ok(())
        }
        OutputFormat::Table => {
            if folders.is_empty() {
                println!("No folders found.");
            } else {
                let rows: Vec<Vec<String>> = folders
                    .iter()
                    .map(|folder| {
                        vec![
                            folder.name.clone(),
                            folder.delimiter.as_deref().unwrap_or("-").to_string(),
                            folder.attributes.join(" "),
                        ]
                    })
                    .collect();
                print_table(&["NAME", "DELIMITER", "ATTRIBUTES"], &rows, &[48, 9, 40]);
            }
            Ok(())
        }
    }
}

pub fn print_accounts(config: &Config, format: OutputFormat) -> Result<()> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct AccountView<'a> {
        email: &'a str,
        display_name: &'a str,
        preset: &'a str,
        imap: String,
        smtp: String,
        password_saved: bool,
        default: bool,
        mailboxes: &'a Mailboxes,
    }
    let default = config.default_account.as_deref();
    let views: Vec<AccountView<'_>> = config
        .accounts
        .iter()
        .map(|account| AccountView {
            email: &account.email,
            display_name: &account.display_name,
            preset: &account.preset,
            imap: account.imap.display(),
            smtp: account.smtp.display(),
            password_saved: account.password_saved,
            default: default.is_some_and(|email| email.eq_ignore_ascii_case(&account.email)),
            mailboxes: &account.mailboxes,
        })
        .collect();

    match format {
        OutputFormat::Json => print_json(&views),
        OutputFormat::Plain => {
            for view in &views {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    view.email,
                    view.preset,
                    view.imap,
                    view.smtp,
                    view.password_saved,
                    view.default
                );
            }
            Ok(())
        }
        OutputFormat::Table => {
            if views.is_empty() {
                println!(
                    "No accounts configured. Run `email account add --email you@example.com`."
                );
            } else {
                let rows: Vec<Vec<String>> = views
                    .iter()
                    .map(|view| {
                        vec![
                            view.email.to_string(),
                            view.preset.to_string(),
                            view.imap.clone(),
                            view.smtp.clone(),
                            if view.password_saved { "yes" } else { "no" }.to_string(),
                            if view.default { "*" } else { "" }.to_string(),
                        ]
                    })
                    .collect();
                print_table(
                    &["EMAIL", "PRESET", "IMAP", "SMTP", "SAVED", "DEFAULT"],
                    &rows,
                    &[32, 10, 30, 30, 5, 7],
                );
            }
            Ok(())
        }
    }
}

pub fn print_test(
    account: &Account,
    imap: &TestResult,
    smtp_ok: bool,
    format: OutputFormat,
) -> Result<()> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestOutput<'a> {
        account: &'a str,
        imap: &'a TestResult,
        smtp: bool,
    }
    match format {
        OutputFormat::Json => print_json(&TestOutput {
            account: &account.email,
            imap,
            smtp: smtp_ok,
        }),
        OutputFormat::Plain | OutputFormat::Table => {
            if format == OutputFormat::Plain {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    account.email, imap.mailbox, imap.messages, imap.unread, smtp_ok
                );
            } else {
                println!("Account: {}", account.email);
                println!(
                    "IMAP:    ok ({} messages, {} unread, {} folders)",
                    imap.messages, imap.unread, imap.folders
                );
                println!(
                    "SMTP:    {}",
                    if smtp_ok { "ok" } else { "connection failed" }
                );
            }
            Ok(())
        }
    }
}

pub fn print_send(result: &SendResult, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => print_json(result),
        OutputFormat::Plain => {
            println!(
                "{}\t{}\t{}",
                result.message_id,
                result.from,
                result.recipients.join(",")
            );
            Ok(())
        }
        OutputFormat::Table => {
            println!(
                "Sent {} to {} recipient{}.",
                result.message_id,
                result.recipients.len(),
                if result.recipients.len() == 1 {
                    ""
                } else {
                    "s"
                }
            );
            Ok(())
        }
    }
}

pub fn print_saved_file(path: &str, bytes: u64) {
    println!("Saved {path} ({})", size(bytes));
}

fn print_json<T: Serialize + ?Sized>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_table(headers: &[&str], rows: &[Vec<String>], max_widths: &[usize]) {
    if rows.is_empty() {
        return;
    }
    let columns = headers.len();
    let mut widths = vec![0_usize; columns];
    for (index, header) in headers.iter().enumerate() {
        widths[index] = display_width(header);
    }
    for row in rows {
        for (index, value) in row.iter().take(columns).enumerate() {
            widths[index] = widths[index].max(display_width(value));
        }
    }
    for (index, width) in widths.iter_mut().enumerate() {
        let cap = max_widths.get(index).copied().unwrap_or(0);
        if cap > 0 {
            *width = (*width).min(cap);
        }
    }

    let header_cells: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(index, header)| pad(header, widths[index]))
        .collect();
    println!("{}", header_cells.join("  "));
    let separator: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    println!("{}", separator.join("  "));
    for row in rows {
        let cells: Vec<String> = (0..columns)
            .map(|index| {
                pad(
                    row.get(index).map(String::as_str).unwrap_or(""),
                    widths[index],
                )
            })
            .collect();
        println!("{}", cells.join("  "));
    }
}

fn pad(value: &str, width: usize) -> String {
    let value = truncate(&cell(value), width);
    let count = display_width(&value);
    let mut output = value;
    if count < width {
        output.push_str(&" ".repeat(width - count));
    }
    output
}

fn truncate(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut output: String = value.chars().take(width.saturating_sub(1)).collect();
    output.push('…');
    output
}

fn display_width(value: &str) -> usize {
    value.chars().count()
}

fn sender(value: &str) -> String {
    let trimmed = value.trim();
    if let (Some(start), Some(end)) = (trimmed.rfind('<'), trimmed.rfind('>')) {
        if end > start {
            let name = trimmed[..start].trim().trim_matches('"').trim();
            if !name.is_empty() {
                return name.to_string();
            }
            return trimmed[start + 1..end].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn cell(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn sanitize_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect()
}

fn size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes / KB)
    } else {
        format!("{} B", bytes as u64)
    }
}
