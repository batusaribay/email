use std::collections::HashSet;
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate, TimeZone};
use imap::types::{Fetch, Flag, Mailbox, NameAttribute, Uid, ZeroCopy};
use mail_parser::{Address, MessageParser, MimeHeaders};
use native_tls::TlsConnector;

use crate::config::{Account, TlsMode};
use crate::mime::{RawAttachment, strip_html};
use crate::model::{Attachment, Folder, MessageSummary, ParsedMessage};
use crate::presets::Preset;

const SUMMARY_FETCH: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (FROM TO SUBJECT DATE MESSAGE-ID REFERENCES)])";
const FULL_FETCH: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[])";

#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Gmail search syntax on Gmail, raw IMAP SEARCH criteria elsewhere.
    pub query: Option<String>,
    pub from: Option<String>,
    pub subject: Option<String>,
    pub since: Option<String>,
    pub before: Option<String>,
    pub unread: bool,
    pub starred: bool,
    pub larger: Option<u64>,
    pub mailbox: String,
    pub limit: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub mailbox: String,
    pub messages: u32,
    pub unread: u32,
    pub folders: usize,
    pub uidplus: bool,
}

#[derive(Debug, Clone)]
pub struct ForwardedMessage {
    pub message: ParsedMessage,
    pub attachments: Vec<RawAttachment>,
}

pub async fn test(account: &Account, password: &str) -> Result<TestResult> {
    let account = account.clone();
    let password = password.to_string();
    run_imap(account, password, |session, account| {
        let mailbox_name = account.mailboxes.inbox.clone();
        let selected = session.select(&mailbox_name)?;
        let unread = session.uid_search("UNSEEN")?.len() as u32;
        let folders = session.folders()?.len();
        let uidplus = session.has_uidplus()?;
        Ok(TestResult {
            mailbox: mailbox_name,
            messages: selected.exists,
            unread,
            folders,
            uidplus,
        })
    })
    .await
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub mailbox: String,
    pub messages: Vec<MessageSummary>,
}

pub async fn search_messages(
    account: &Account,
    password: &str,
    options: SearchOptions,
) -> Result<SearchResult> {
    let account = account.clone();
    let password = password.to_string();
    run_imap(account, password, move |session, account| {
        let criteria = build_search(&options, account)?;
        let mailbox = if account.is_gmail()
            && (options.mailbox.is_empty()
                || options.mailbox.eq_ignore_ascii_case(&account.mailboxes.all))
        {
            resolve_special(session, account, SpecialFolder::All)
                .unwrap_or_else(|_| options.mailbox.clone())
        } else {
            options.mailbox.clone()
        };
        session.select(&mailbox)?;
        let mut uids: Vec<Uid> = session.uid_search(&criteria)?.into_iter().collect();
        uids.sort_unstable_by(|a, b| b.cmp(a));
        uids.truncate(options.limit as usize);
        let mut messages = fetch_summaries(session, &mailbox, &uids)?;
        messages.sort_unstable_by_key(|message| std::cmp::Reverse(message.uid));
        Ok(SearchResult { mailbox, messages })
    })
    .await
}

pub async fn fetch_message(
    account: &Account,
    password: &str,
    mailbox: &str,
    uid: u32,
    mark_seen: bool,
) -> Result<ParsedMessage> {
    let account = account.clone();
    let password = password.to_string();
    let mailbox = mailbox.to_string();
    run_imap(account, password, move |session, _account| {
        session.select(&mailbox)?;
        let fetches = fetch_one(session, uid, FULL_FETCH)?;
        let fetch = fetches
            .iter()
            .find(|fetch| fetch.uid == Some(uid))
            .ok_or_else(|| anyhow::anyhow!("message {uid} was not found in {mailbox}"))?;
        let message = parse_full(fetch, uid, &mailbox)?;
        if mark_seen {
            if let Err(error) = session.uid_store(&uid.to_string(), "+FLAGS.SILENT (\\Seen)") {
                eprintln!("warning: could not mark message {uid} as read: {error}");
            }
        }
        Ok(message)
    })
    .await
}

pub async fn fetch_raw(
    account: &Account,
    password: &str,
    mailbox: &str,
    uid: u32,
    mark_seen: bool,
) -> Result<Vec<u8>> {
    let account = account.clone();
    let password = password.to_string();
    let mailbox = mailbox.to_string();
    run_imap(account, password, move |session, _account| {
        session.select(&mailbox)?;
        let fetches = fetch_one(session, uid, FULL_FETCH)?;
        let fetch = fetches
            .iter()
            .find(|fetch| fetch.uid == Some(uid))
            .ok_or_else(|| anyhow::anyhow!("message {uid} was not found in {mailbox}"))?;
        let bytes = fetch
            .body()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow::anyhow!("message {uid} has no body"))?;
        if mark_seen {
            if let Err(error) = session.uid_store(&uid.to_string(), "+FLAGS.SILENT (\\Seen)") {
                eprintln!("warning: could not mark message {uid} as read: {error}");
            }
        }
        Ok(bytes)
    })
    .await
}

pub async fn fetch_forward_messages(
    account: &Account,
    password: &str,
    mailbox: &str,
    uids: &[u32],
    mark_seen: bool,
) -> Result<Vec<ForwardedMessage>> {
    if uids.is_empty() {
        bail!("no message UIDs provided");
    }
    let account = account.clone();
    let password = password.to_string();
    let mailbox = mailbox.to_string();
    let uids = uids.to_vec();
    run_imap(account, password, move |session, _account| {
        session.select(&mailbox)?;
        let set = uid_set(&uids);
        let fetches = session.uid_fetch(&set, FULL_FETCH)?;
        let mut messages = Vec::with_capacity(uids.len());
        for uid in &uids {
            let fetch = fetches
                .iter()
                .find(|fetch| fetch.uid == Some(*uid))
                .ok_or_else(|| anyhow::anyhow!("message {uid} was not found in {mailbox}"))?;
            let raw = fetch
                .body()
                .ok_or_else(|| anyhow::anyhow!("message {uid} has no body"))?;
            let parsed = MessageParser::default()
                .parse(raw)
                .ok_or_else(|| anyhow::anyhow!("could not parse message {uid}"))?;
            let attachments = parsed
                .attachments()
                .enumerate()
                .map(|(index, part)| raw_attachment_from_part(part, index + 1))
                .collect();
            messages.push(ForwardedMessage {
                message: parse_full(fetch, *uid, &mailbox)?,
                attachments,
            });
        }
        if mark_seen {
            if let Err(error) = session.uid_store(&set, "+FLAGS.SILENT (\\Seen)") {
                eprintln!("warning: could not mark forwarded messages as read: {error}");
            }
        }
        Ok(messages)
    })
    .await
}

pub async fn download_attachment(
    account: &Account,
    password: &str,
    mailbox: &str,
    uid: u32,
    index: usize,
) -> Result<(Attachment, Vec<u8>)> {
    if index == 0 {
        bail!("attachment numbers start at 1");
    }
    let account = account.clone();
    let password = password.to_string();
    let mailbox = mailbox.to_string();
    run_imap(account, password, move |session, _account| {
        session.select(&mailbox)?;
        let fetches = fetch_one(session, uid, FULL_FETCH)?;
        let fetch = fetches
            .iter()
            .find(|fetch| fetch.uid == Some(uid))
            .ok_or_else(|| anyhow::anyhow!("message {uid} was not found"))?;
        let raw = fetch
            .body()
            .ok_or_else(|| anyhow::anyhow!("message {uid} has no body"))?;
        let parsed = MessageParser::default()
            .parse(raw)
            .ok_or_else(|| anyhow::anyhow!("could not parse message {uid}"))?;
        let position = (index - 1) as u32;
        let part = parsed
            .attachment(position)
            .ok_or_else(|| anyhow::anyhow!("message {uid} has no attachment {index}"))?;
        let attachment = attachment_from_part(part, index);
        Ok((attachment, part.contents().to_vec()))
    })
    .await
}

pub async fn folders(account: &Account, password: &str) -> Result<Vec<Folder>> {
    let account = account.clone();
    let password = password.to_string();
    run_imap(account, password, |session, _account| session.folders()).await
}

pub async fn create_folder(account: &Account, password: &str, name: &str) -> Result<()> {
    validate_mailbox_name(name)?;
    let account = account.clone();
    let password = password.to_string();
    let name = name.to_string();
    run_imap(account, password, move |session, _account| {
        session.create(&name)
    })
    .await
}

pub async fn delete_folder(account: &Account, password: &str, name: &str) -> Result<()> {
    validate_mailbox_name(name)?;
    let account = account.clone();
    let password = password.to_string();
    let name = name.to_string();
    run_imap(account, password, move |session, _account| {
        session.delete(&name)
    })
    .await
}

pub async fn rename_folder(account: &Account, password: &str, old: &str, new: &str) -> Result<()> {
    validate_mailbox_name(old)?;
    validate_mailbox_name(new)?;
    let account = account.clone();
    let password = password.to_string();
    let old = old.to_string();
    let new = new.to_string();
    run_imap(account, password, move |session, _account| {
        session.rename(&old, &new)
    })
    .await
}

pub async fn set_flags(
    account: &Account,
    password: &str,
    mailbox: &str,
    uids: &[u32],
    add: &[&'static str],
    remove: &[&'static str],
) -> Result<()> {
    if uids.is_empty() {
        bail!("no message UIDs provided");
    }
    let account = account.clone();
    let password = password.to_string();
    let mailbox = mailbox.to_string();
    let uids = uids.to_vec();
    let add: Vec<&'static str> = add.to_vec();
    let remove: Vec<&'static str> = remove.to_vec();
    run_imap(account, password, move |session, _account| {
        session.select(&mailbox)?;
        let set = uid_set(&uids);
        if !add.is_empty() {
            session.uid_store(&set, &format!("+FLAGS.SILENT ({})", add.join(" ")))?;
        }
        if !remove.is_empty() {
            session.uid_store(&set, &format!("-FLAGS.SILENT ({})", remove.join(" ")))?;
        }
        Ok(())
    })
    .await
}

pub async fn archive(account: &Account, password: &str, mailbox: &str, uids: &[u32]) -> Result<()> {
    if uids.is_empty() {
        bail!("no message UIDs provided");
    }
    let account = account.clone();
    let password = password.to_string();
    let mailbox = mailbox.to_string();
    let uids = uids.to_vec();
    run_imap(account, password, move |session, account| {
        session.select(&mailbox)?;
        if account.is_gmail() {
            let command = format!("UID STORE {} -X-GM-LABELS (\\Inbox)", uid_set(&uids));
            session.raw_command(&command)?;
            return Ok(());
        }
        let target = resolve_special(session, account, SpecialFolder::Archive)?;
        move_uids(session, &uids, &target)
    })
    .await
}

pub async fn trash(account: &Account, password: &str, mailbox: &str, uids: &[u32]) -> Result<()> {
    if uids.is_empty() {
        bail!("no message UIDs provided");
    }
    let account = account.clone();
    let password = password.to_string();
    let mailbox = mailbox.to_string();
    let uids = uids.to_vec();
    run_imap(account, password, move |session, account| {
        if account.is_gmail() && source_is_gmail_all_mail(session, account, &mailbox)? {
            bail!(
                "refusing to trash messages from Gmail's All Mail folder; \
                 select INBOX or a specific label as the source instead"
            );
        }
        session.select(&mailbox)?;
        let target = resolve_special(session, account, SpecialFolder::Trash)?;
        if target.eq_ignore_ascii_case(&mailbox) {
            bail!("message is already in the trash folder '{target}'");
        }
        move_uids(session, &uids, &target)
    })
    .await
}

pub async fn move_messages(
    account: &Account,
    password: &str,
    mailbox: &str,
    uids: &[u32],
    target: &str,
) -> Result<()> {
    if uids.is_empty() {
        bail!("no message UIDs provided");
    }
    validate_mailbox_name(target)?;
    let account = account.clone();
    let password = password.to_string();
    let mailbox = mailbox.to_string();
    let target = target.to_string();
    let uids = uids.to_vec();
    run_imap(account, password, move |session, account| {
        if mailbox.eq_ignore_ascii_case(&target) {
            bail!("source and destination mailboxes are the same");
        }
        if account.is_gmail() && source_is_gmail_all_mail(session, account, &mailbox)? {
            bail!(
                "refusing to move messages from Gmail's All Mail folder; \
                 select INBOX or a specific label as the source instead"
            );
        }
        session.select(&mailbox)?;
        move_uids(session, &uids, &target)
    })
    .await
}

async fn run_imap<T, F>(account: Account, password: String, operation: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&mut ImapSession, &Account) -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut session = ImapSession::connect(&account, &password)?;
        let result = operation(&mut session, &account);
        let _ = session.logout();
        result
    })
    .await
    .context("IMAP worker failed")?
}

enum ImapSession {
    Tls(imap::Session<native_tls::TlsStream<TcpStream>>),
    Plain(imap::Session<TcpStream>),
}

macro_rules! with_session {
    ($self:expr, $session:ident => $body:expr) => {
        match $self {
            ImapSession::Tls($session) => $body,
            ImapSession::Plain($session) => $body,
        }
    };
}

impl ImapSession {
    fn connect(account: &Account, password: &str) -> Result<Self> {
        if account.imap.host.trim().is_empty() {
            bail!("account '{}' has no IMAP host", account.email);
        }
        if account.imap.port == 0 {
            bail!("account '{}' has an invalid IMAP port", account.email);
        }
        let host = account.imap.host.as_str();
        let port = account.imap.port;
        let tls = TlsConnector::builder()
            .build()
            .context("could not initialize TLS")?;

        let session = match account.imap.tls {
            TlsMode::Tls => {
                let tcp = connect_tcp(host, port)?;
                let stream = tls
                    .connect(host, tcp)
                    .with_context(|| format!("TLS handshake failed for {host}:{port}"))?;
                let mut client = imap::Client::new(stream);
                client
                    .read_greeting()
                    .map_err(|error| imap_error("IMAP greeting", error))?;
                let session = client
                    .login(account.username(), password)
                    .map_err(|(error, _)| login_error(error, account))?;
                Self::Tls(session)
            }
            TlsMode::Starttls => {
                let client = imap::connect_starttls((host, port), host, &tls)
                    .with_context(|| format!("could not connect to IMAP {host}:{port}"))?;
                let session = client
                    .login(account.username(), password)
                    .map_err(|(error, _)| login_error(error, account))?;
                Self::Tls(session)
            }
            TlsMode::None => {
                let tcp = connect_tcp(host, port)?;
                let mut client = imap::Client::new(tcp);
                client
                    .read_greeting()
                    .map_err(|error| imap_error("IMAP greeting", error))?;
                let session = client
                    .login(account.username(), password)
                    .map_err(|(error, _)| login_error(error, account))?;
                Self::Plain(session)
            }
        };
        Ok(session)
    }

    fn select(&mut self, mailbox: &str) -> Result<Mailbox> {
        with_session!(self, session => session.select(mailbox))
            .map_err(|error| imap_error("SELECT", error))
    }

    fn uid_search(&mut self, query: &str) -> Result<HashSet<Uid>> {
        let result = with_session!(self, session => session.uid_search(query))
            .map_err(|error| imap_error("UID SEARCH", error))?;
        Ok(result.iter().copied().collect())
    }

    fn uid_fetch(&mut self, uids: &str, query: &str) -> Result<ZeroCopy<Vec<Fetch>>> {
        with_session!(self, session => session.uid_fetch(uids, query))
            .map_err(|error| imap_error("UID FETCH", error))
    }

    fn uid_store(&mut self, uids: &str, query: &str) -> Result<()> {
        with_session!(self, session => session.uid_store(uids, query))
            .map(|_| ())
            .map_err(|error| imap_error("UID STORE", error))
    }

    fn uid_copy(&mut self, uids: &str, mailbox: &str) -> Result<()> {
        with_session!(self, session => session.uid_copy(uids, mailbox))
            .map_err(|error| imap_error("UID COPY", error))
    }

    fn uid_expunge(&mut self, uids: &str) -> Result<()> {
        with_session!(self, session => session.uid_expunge(uids))
            .map(|_| ())
            .map_err(|error| imap_error("UID EXPUNGE", error))
    }

    fn folders(&mut self) -> Result<Vec<Folder>> {
        let names = with_session!(self, session => session.list(None, Some("*")))
            .map_err(|error| imap_error("LIST", error))?;
        let mut folders: Vec<Folder> = names
            .iter()
            .map(|name| {
                let attributes: Vec<String> =
                    name.attributes().iter().map(attribute_name).collect();
                Folder {
                    name: name.name().to_string(),
                    delimiter: name.delimiter().map(ToOwned::to_owned),
                    selectable: !attributes
                        .iter()
                        .any(|attribute| attribute.eq_ignore_ascii_case("\\Noselect")),
                    attributes,
                }
            })
            .collect();
        folders.sort_by_key(|folder| folder.name.to_lowercase());
        Ok(folders)
    }

    fn create(&mut self, name: &str) -> Result<()> {
        with_session!(self, session => session.create(name))
            .map_err(|error| imap_error("CREATE", error))
    }

    fn delete(&mut self, name: &str) -> Result<()> {
        with_session!(self, session => session.delete(name))
            .map_err(|error| imap_error("DELETE", error))
    }

    fn rename(&mut self, old: &str, new: &str) -> Result<()> {
        with_session!(self, session => session.rename(old, new))
            .map_err(|error| imap_error("RENAME", error))
    }

    fn raw_command(&mut self, command: &str) -> Result<()> {
        validate_imap_query(command)?;
        with_session!(self, session => session.run_command_and_check_ok(command))
            .map_err(|error| imap_error("IMAP command", error))
    }

    fn has_uidplus(&mut self) -> Result<bool> {
        let capabilities = with_session!(self, session => session.capabilities())
            .map_err(|error| imap_error("CAPABILITY", error))?;
        Ok(capabilities.has_str("UIDPLUS"))
    }

    fn logout(&mut self) -> Result<()> {
        with_session!(self, session => session.logout())
            .map_err(|error| imap_error("LOGOUT", error))
    }
}

fn connect_tcp(host: &str, port: u16) -> Result<TcpStream> {
    let stream = TcpStream::connect((host, port))
        .with_context(|| format!("could not connect to {host}:{port}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .context("could not set IMAP read timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(60)))
        .context("could not set IMAP write timeout")?;
    Ok(stream)
}

fn fetch_summaries(
    session: &mut ImapSession,
    mailbox: &str,
    uids: &[Uid],
) -> Result<Vec<MessageSummary>> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    let parser = MessageParser::default();
    let mut messages = Vec::with_capacity(uids.len());
    for chunk in uids.chunks(50) {
        let set = uid_set(chunk);
        let fetches = session.uid_fetch(&set, SUMMARY_FETCH)?;
        for fetch in fetches.iter() {
            if let Some(message) = summary_from_fetch(fetch, mailbox, &parser) {
                messages.push(message);
            }
        }
    }
    Ok(messages)
}

fn summary_from_fetch(
    fetch: &Fetch,
    mailbox: &str,
    parser: &MessageParser,
) -> Option<MessageSummary> {
    let uid = fetch.uid?;
    let header = fetch.header().unwrap_or_default();
    let parsed = parser.parse(header);
    let from = parsed
        .as_ref()
        .and_then(|message| message.from())
        .map(format_address)
        .unwrap_or_default();
    let to = parsed
        .as_ref()
        .and_then(|message| message.to())
        .map(format_address)
        .unwrap_or_default();
    let subject = parsed
        .as_ref()
        .and_then(|message| message.subject())
        .unwrap_or_default()
        .to_string();
    let internal = fetch.internal_date();
    let timestamp = internal.map(|date| date.timestamp()).or_else(|| {
        parsed
            .as_ref()
            .and_then(|message| message.date())
            .map(|date| date.to_timestamp())
    });
    let date = timestamp.map(format_local_datetime).unwrap_or_default();
    let flags = flags_from_fetch(fetch);
    let unread = !fetch.flags().iter().any(|flag| matches!(flag, Flag::Seen));
    let starred = fetch
        .flags()
        .iter()
        .any(|flag| matches!(flag, Flag::Flagged));
    Some(MessageSummary {
        uid,
        mailbox: mailbox.to_string(),
        from,
        to,
        subject,
        date,
        timestamp,
        size: fetch.size.unwrap_or(0),
        flags,
        unread,
        starred,
    })
}

fn fetch_one(session: &mut ImapSession, uid: u32, query: &str) -> Result<ZeroCopy<Vec<Fetch>>> {
    session.uid_fetch(&uid.to_string(), query)
}

fn parse_full(fetch: &Fetch, uid: u32, mailbox: &str) -> Result<ParsedMessage> {
    let raw = fetch
        .body()
        .ok_or_else(|| anyhow::anyhow!("message {uid} has no body"))?;
    let parsed = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| anyhow::anyhow!("could not parse message {uid}"))?;
    let flags = flags_from_fetch(fetch);
    let unread = !fetch.flags().iter().any(|flag| matches!(flag, Flag::Seen));
    let starred = fetch
        .flags()
        .iter()
        .any(|flag| matches!(flag, Flag::Flagged));
    let parsed_timestamp = parsed.date().map(|date| date.to_timestamp());
    let timestamp = fetch
        .internal_date()
        .map(|date| date.timestamp())
        .or(parsed_timestamp);
    let attachments = parsed
        .attachments()
        .enumerate()
        .map(|(index, part)| attachment_from_part(part, index + 1))
        .collect();
    let references = normalize_message_ids(&header_text(parsed.references()));
    let in_reply_to = normalize_message_ids(&header_text(parsed.in_reply_to()));
    let (body_text, body_html) = extract_body(&parsed);
    Ok(ParsedMessage {
        uid,
        mailbox: mailbox.to_string(),
        from: parsed.from().map(format_address).unwrap_or_default(),
        to: parsed.to().map(format_address).unwrap_or_default(),
        cc: parsed.cc().map(format_address).unwrap_or_default(),
        bcc: parsed.bcc().map(format_address).unwrap_or_default(),
        reply_to: parsed.reply_to().map(format_address).unwrap_or_default(),
        subject: parsed.subject().unwrap_or_default().to_string(),
        date: timestamp.map(format_local_datetime).unwrap_or_default(),
        timestamp,
        message_id: parsed.message_id().map(normalize_message_id),
        in_reply_to: (!in_reply_to.is_empty()).then_some(in_reply_to),
        references: (!references.is_empty()).then_some(references),
        body_text,
        body_html,
        attachments,
        flags,
        unread,
        starred,
        size: fetch.size.unwrap_or(0),
    })
}

fn extract_body(parsed: &mail_parser::Message<'_>) -> (String, Option<String>) {
    let html = parsed
        .html_part(0)
        .filter(|part| part.is_text_html())
        .and_then(|_| parsed.body_html(0))
        .map(|html| html.into_owned());
    let text_is_html = parsed.text_part(0).is_some_and(|part| part.is_text_html());
    let text = if text_is_html {
        String::new()
    } else {
        parsed
            .body_text(0)
            .map(|text| text.into_owned())
            .unwrap_or_default()
    };
    let text = if text.trim().is_empty() {
        html.as_deref().map(strip_html).unwrap_or_default()
    } else {
        text
    };
    (text, html)
}

fn attachment_from_part(part: &mail_parser::MessagePart<'_>, index: usize) -> Attachment {
    Attachment {
        index,
        filename: attachment_filename(part, index),
        mime_type: attachment_mime_type(part),
        size: part.len() as u64,
    }
}

fn raw_attachment_from_part(part: &mail_parser::MessagePart<'_>, index: usize) -> RawAttachment {
    RawAttachment {
        filename: attachment_filename(part, index),
        mime_type: attachment_mime_type(part),
        bytes: part.contents().to_vec(),
    }
}

fn attachment_filename(part: &mail_parser::MessagePart<'_>, index: usize) -> String {
    part.attachment_name()
        .map(ToOwned::to_owned)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("attachment-{index}"))
}

fn attachment_mime_type(part: &mail_parser::MessagePart<'_>) -> String {
    part.content_type()
        .map(|content_type| {
            let subtype = content_type.c_subtype.as_deref().unwrap_or("octet-stream");
            format!("{}/{subtype}", content_type.c_type)
        })
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

fn flags_from_fetch(fetch: &Fetch) -> Vec<String> {
    fetch.flags().iter().map(flag_name).collect()
}

fn flag_name(flag: &Flag<'_>) -> String {
    match flag {
        Flag::Seen => "seen",
        Flag::Answered => "answered",
        Flag::Flagged => "flagged",
        Flag::Deleted => "deleted",
        Flag::Draft => "draft",
        Flag::Recent => "recent",
        Flag::MayCreate => "*",
        Flag::Custom(name) => name.as_ref(),
    }
    .to_string()
}

fn format_address(address: &Address<'_>) -> String {
    address
        .iter()
        .map(
            |addr| match (addr.name.as_deref(), addr.address.as_deref()) {
                (Some(name), Some(email)) if !name.trim().is_empty() => {
                    format!("{} <{}>", name.trim(), email)
                }
                (_, Some(email)) => email.to_string(),
                (Some(name), None) => name.to_string(),
                (None, None) => String::new(),
            },
        )
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_local_datetime(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|date| date.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn header_text(value: &mail_parser::HeaderValue<'_>) -> String {
    if let Some(list) = value.as_text_list() {
        return list.join(" ");
    }
    value.as_text().unwrap_or_default().to_string()
}

fn normalize_message_id(value: &str) -> String {
    let value = value.trim();
    if value.starts_with('<') && value.ends_with('>') {
        value.to_string()
    } else {
        format!("<{value}>")
    }
}

fn normalize_message_ids(value: &str) -> String {
    value
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(normalize_message_id)
        .collect::<Vec<_>>()
        .join(" ")
}

fn attribute_name(attribute: &NameAttribute<'_>) -> String {
    match attribute {
        NameAttribute::NoInferiors => "\\Noinferiors".to_string(),
        NameAttribute::NoSelect => "\\Noselect".to_string(),
        NameAttribute::Marked => "\\Marked".to_string(),
        NameAttribute::Unmarked => "\\Unmarked".to_string(),
        NameAttribute::Custom(value) => value.to_string(),
    }
}

fn build_search(options: &SearchOptions, account: &Account) -> Result<String> {
    let mut criteria = Vec::new();
    if let Some(query) = options
        .query
        .as_deref()
        .filter(|query| !query.trim().is_empty())
    {
        validate_imap_query(query)?;
        if account.is_gmail() {
            criteria.push(format!("X-GM-RAW \"{}\"", escape_quoted(query.trim())));
        } else {
            criteria.push(query.trim().to_string());
        }
    }
    if let Some(from) = options
        .from
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        validate_imap_query(from)?;
        criteria.push(format!("FROM \"{}\"", escape_quoted(from.trim())));
    }
    if let Some(subject) = options
        .subject
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        validate_imap_query(subject)?;
        criteria.push(format!("SUBJECT \"{}\"", escape_quoted(subject.trim())));
    }
    if let Some(since) = options
        .since
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        criteria.push(format!("SINCE {}", parse_imap_date(since)?));
    }
    if let Some(before) = options
        .before
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        criteria.push(format!("BEFORE {}", parse_imap_date(before)?));
    }
    if options.unread {
        criteria.push("UNSEEN".to_string());
    }
    if options.starred {
        criteria.push("FLAGGED".to_string());
    }
    if let Some(larger) = options.larger {
        criteria.push(format!("LARGER {larger}"));
    }
    if criteria.is_empty() {
        criteria.push("ALL".to_string());
    }
    Ok(criteria.join(" "))
}

fn parse_imap_date(value: &str) -> Result<String> {
    let value = value.trim();
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(date.format("%d-%b-%Y").to_string());
    }
    let bytes = value.as_bytes();
    let looks_like_imap = (9..=11).contains(&bytes.len())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        && value.contains('-');
    if looks_like_imap {
        return Ok(value.to_string());
    }
    bail!("invalid date '{value}': use YYYY-MM-DD or DD-Mon-YYYY")
}

fn uid_set(uids: &[Uid]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn escape_quoted(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn validate_imap_query(value: &str) -> Result<()> {
    if value.contains('\r') || value.contains('\n') {
        bail!("IMAP query must be a single line");
    }
    Ok(())
}

fn source_is_gmail_all_mail(
    session: &mut ImapSession,
    account: &Account,
    mailbox: &str,
) -> Result<bool> {
    Ok(resolve_special(session, account, SpecialFolder::All)?.eq_ignore_ascii_case(mailbox.trim()))
}

fn validate_mailbox_name(value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("mailbox name must not be empty");
    }
    validate_imap_query(value)
}

fn move_uids(session: &mut ImapSession, uids: &[Uid], target: &str) -> Result<()> {
    if !session.has_uidplus()? {
        bail!("this IMAP server does not support UIDPLUS, so safe move/expunge is unavailable");
    }
    let set = uid_set(uids);
    session.uid_copy(&set, target)?;
    session.uid_store(&set, "+FLAGS.SILENT (\\Deleted)")?;
    session.uid_expunge(&set)?;
    Ok(())
}

#[derive(Clone, Copy)]
enum SpecialFolder {
    Trash,
    Archive,
    All,
}

impl SpecialFolder {
    fn configured(self, account: &Account) -> &str {
        match self {
            Self::Trash => &account.mailboxes.trash,
            Self::Archive => &account.mailboxes.archive,
            Self::All => &account.mailboxes.all,
        }
    }

    fn special_use(self) -> &'static [&'static str] {
        match self {
            Self::Trash => &["\\Trash"],
            Self::Archive => &["\\Archive"],
            Self::All => &["\\All"],
        }
    }

    fn fallback_names(self) -> &'static [&'static str] {
        match self {
            Self::Trash => &["Trash", "Deleted Items", "Deleted Messages", "Bin"],
            Self::Archive => &["Archive", "Archives"],
            Self::All => &["All Mail", "All"],
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Trash => "trash",
            Self::Archive => "archive",
            Self::All => "all-mail",
        }
    }
}

fn resolve_special(
    session: &mut ImapSession,
    account: &Account,
    special: SpecialFolder,
) -> Result<String> {
    let folders = session.folders()?;
    // Special-use flags are language-independent, unlike names such as
    // "[Gmail]/All Mail" which are localized by Gmail.
    for folder in &folders {
        if folder.attributes.iter().any(|attribute| {
            special
                .special_use()
                .iter()
                .any(|wanted| attribute.eq_ignore_ascii_case(wanted))
        }) {
            return Ok(folder.name.clone());
        }
    }
    let configured = special.configured(account).trim();
    if !configured.is_empty() {
        return Ok(configured.to_string());
    }
    for fallback in special.fallback_names() {
        if let Some(folder) = folders
            .iter()
            .find(|folder| folder.name.eq_ignore_ascii_case(fallback))
        {
            return Ok(folder.name.clone());
        }
    }
    bail!(
        "could not find the {} mailbox; configure it with --{}-mailbox when adding the account",
        special.label().replace('-', " "),
        special.label()
    )
}

fn login_error(error: imap::Error, account: &Account) -> anyhow::Error {
    let mut message = format!("IMAP login failed for {}: {error}", account.email);
    let text = error.to_string().to_ascii_lowercase();
    if account.is_gmail()
        || text.contains("application-specific password")
        || text.contains("app password")
    {
        message.push_str(
            "\nGmail requires a 16-character app password, not your normal password. \
             Enable 2-Step Verification and create one at \
             https://myaccount.google.com/apppasswords",
        );
    } else if account.preset_enum() == Some(Preset::Yahoo) {
        message.push_str("\nYahoo requires an app password from Account Security.");
    } else if account.preset_enum() == Some(Preset::Icloud) {
        message.push_str("\niCloud requires an app-specific password from account.apple.com.");
    } else if account.preset_enum() == Some(Preset::Outlook) {
        message.push_str(
            "\nMicrosoft accounts often require OAuth; password authentication may be disabled.",
        );
    }
    anyhow::anyhow!(message)
}

fn imap_error(operation: &str, error: imap::Error) -> anyhow::Error {
    anyhow::anyhow!("{operation} failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mailboxes;
    use crate::presets::Preset;

    fn account(email: &str) -> Account {
        let preset = Preset::detect(email);
        let defaults = preset.defaults();
        Account {
            email: email.to_string(),
            display_name: String::new(),
            username: email.to_string(),
            password: None,
            password_saved: false,
            last_mailbox: None,
            preset: preset.name().to_string(),
            imap: defaults.imap,
            smtp: defaults.smtp,
            mailboxes: defaults.mailboxes,
        }
    }

    #[test]
    fn builds_gmail_and_generic_searches() {
        let gmail = account("a@gmail.com");
        let query = build_search(
            &SearchOptions {
                query: Some("from:alice has:attachment".to_string()),
                unread: true,
                ..SearchOptions::default()
            },
            &gmail,
        )
        .unwrap();
        assert!(query.contains("X-GM-RAW \"from:alice has:attachment\""));
        assert!(query.contains("UNSEEN"));

        let generic = account("a@example.com");
        let query = build_search(
            &SearchOptions {
                from: Some("alice@example.com".to_string()),
                subject: Some("invoice".to_string()),
                since: Some("2026-01-31".to_string()),
                unread: true,
                ..SearchOptions::default()
            },
            &generic,
        )
        .unwrap();
        assert_eq!(
            query,
            "FROM \"alice@example.com\" SUBJECT \"invoice\" SINCE 31-Jan-2026 UNSEEN"
        );
    }

    #[test]
    fn parses_dates_and_uid_sets() {
        assert_eq!(parse_imap_date("2026-11-14").unwrap(), "14-Nov-2026");
        assert_eq!(parse_imap_date("14-Nov-2026").unwrap(), "14-Nov-2026");
        assert!(parse_imap_date("14/11/2026").is_err());
        assert_eq!(uid_set(&[3, 1, 2]), "3,1,2");
    }

    #[test]
    fn formats_addresses() {
        let message = MessageParser::default()
            .parse(b"From: Alice Example <alice@example.com>\r\nTo: bob@example.com\r\n\r\n")
            .unwrap();
        assert_eq!(
            format_address(message.from().unwrap()),
            "Alice Example <alice@example.com>"
        );
        assert_eq!(format_address(message.to().unwrap()), "bob@example.com");
    }

    #[test]
    fn extracts_plain_and_html_bodies() {
        let plain = MessageParser::default()
            .parse(b"Content-Type: text/plain\r\n\r\nhello")
            .unwrap();
        let (text, html) = extract_body(&plain);
        assert_eq!(text, "hello");
        assert!(html.is_none());

        let html_only = MessageParser::default()
            .parse(b"Content-Type: text/html\r\n\r\n<p>Hello <b>world</b></p>")
            .unwrap();
        let (text, html) = extract_body(&html_only);
        assert!(text.contains("Hello"));
        assert!(html.unwrap().contains("<b>world</b>"));

        let google = MessageParser::default()
            .parse(
                b"Content-Type: text/html\r\n\r\n<!--[if mso]><v:roundrect>secret</v:roundrect><![endif]--><p>Body</p>",
            )
            .unwrap();
        let (text, _) = extract_body(&google);
        assert!(text.contains("Body"));
        assert!(!text.contains("if mso"));
        assert!(!text.contains("secret"));
    }

    #[test]
    fn parses_a_full_message_with_attachment() {
        let raw = concat!(
            "From: Alice <alice@example.com>\r\n",
            "To: bob@example.com\r\n",
            "Subject: Test\r\n",
            "Date: Fri, 14 Nov 2026 10:00:00 +0300\r\n",
            "Message-ID: <abc@example.com>\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=\"x\"\r\n",
            "\r\n",
            "--x\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n",
            "\r\n",
            "hello\r\n",
            "--x\r\n",
            "Content-Type: text/plain; name=\"note.txt\"\r\n",
            "Content-Disposition: attachment; filename=\"note.txt\"\r\n",
            "\r\n",
            "attachment body\r\n",
            "--x--\r\n"
        );
        let parsed = MessageParser::default().parse(raw.as_bytes()).unwrap();
        assert_eq!(parsed.subject(), Some("Test"));
        assert_eq!(parsed.body_text(0).unwrap(), "hello");
        let part = parsed.attachment(0).unwrap();
        assert_eq!(part.attachment_name(), Some("note.txt"));
        assert_eq!(part.contents(), b"attachment body");
    }

    #[test]
    fn decodes_base64_attachments() {
        let raw = concat!(
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=\"x\"\r\n",
            "\r\n",
            "--x\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "body\r\n",
            "--x\r\n",
            "Content-Type: application/octet-stream; name=\"file.bin\"\r\n",
            "Content-Transfer-Encoding: base64\r\n",
            "Content-Disposition: attachment; filename=\"file.bin\"\r\n",
            "\r\n",
            "AAECAwQ=\r\n",
            "--x--\r\n"
        );
        let parsed = MessageParser::default().parse(raw.as_bytes()).unwrap();
        let part = parsed.attachment(0).unwrap();
        assert_eq!(part.attachment_name(), Some("file.bin"));
        assert_eq!(part.contents(), &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn default_mailboxes_are_sane() {
        let mailboxes = Mailboxes::default();
        assert_eq!(mailboxes.inbox, "INBOX");
    }

    fn mock_account(port: u16) -> Account {
        let mut account = account("me@example.com");
        account.preset = "custom".to_string();
        account.imap = crate::config::Server::new("127.0.0.1", port, TlsMode::None);
        account.smtp = crate::config::Server::new("127.0.0.1", 2525, TlsMode::None);
        account
    }

    fn start_mock_imap(full_message: bool) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            write!(socket, "* OK mock IMAP ready\r\n").unwrap();
            socket.flush().unwrap();
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let command = line.trim_end().to_string();
                let tag = command
                    .split_whitespace()
                    .next()
                    .unwrap_or("a1")
                    .to_string();
                let upper = command.to_ascii_uppercase();
                if upper.contains("LOGIN") {
                    write!(socket, "{tag} OK logged in\r\n").unwrap();
                } else if upper.contains("SELECT") {
                    write!(
                        socket,
                        "* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)\r\n\
                         * 1 EXISTS\r\n\
                         * 0 RECENT\r\n\
                         * OK [UIDVALIDITY 1] valid\r\n\
                         * OK [UIDNEXT 43] next\r\n\
                         * OK [PERMANENTFLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft \\*)] permanent\r\n\
                         {tag} OK [READ-WRITE] selected\r\n"
                    )
                    .unwrap();
                } else if upper.contains("LIST") {
                    write!(
                        socket,
                        "* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n\
                         * LIST (\\HasNoChildren \\Sent) \"/\" \"Sent\"\r\n\
                         * LIST (\\HasNoChildren \\Drafts) \"/\" \"Drafts\"\r\n\
                         * LIST (\\HasNoChildren \\Trash) \"/\" \"Trash\"\r\n\
                         * LIST (\\HasNoChildren \\Archive) \"/\" \"Archive\"\r\n\
                         * LIST (\\HasNoChildren \\All) \"/\" \"[Gmail]/Tüm Postalar\"\r\n\
                         {tag} OK list done\r\n"
                    )
                    .unwrap();
                } else if upper.contains("UID SEARCH") {
                    write!(socket, "* SEARCH 42\r\n{tag} OK search done\r\n").unwrap();
                } else if upper.contains("UID FETCH") {
                    let body = if full_message {
                        concat!(
                            "From: Alice <alice@example.com>\r\n",
                            "To: bob@example.com\r\n",
                            "Subject: Test\r\n",
                            "Date: Fri, 14 Nov 2026 10:00:00 +0300\r\n",
                            "Message-ID: <abc@example.com>\r\n",
                            "MIME-Version: 1.0\r\n",
                            "Content-Type: multipart/mixed; boundary=\"x\"\r\n",
                            "\r\n",
                            "--x\r\n",
                            "Content-Type: text/plain; charset=utf-8\r\n",
                            "\r\n",
                            "hello\r\n",
                            "--x\r\n",
                            "Content-Type: text/plain; name=\"note.txt\"\r\n",
                            "Content-Disposition: attachment; filename=\"note.txt\"\r\n",
                            "\r\n",
                            "attachment body\r\n",
                            "--x--\r\n"
                        )
                    } else {
                        concat!(
                            "From: Alice <alice@example.com>\r\n",
                            "To: bob@example.com\r\n",
                            "Subject: Test\r\n",
                            "Date: Fri, 14 Nov 2026 10:00:00 +0300\r\n",
                            "Message-ID: <abc@example.com>\r\n",
                            "\r\n"
                        )
                    };
                    let section = if full_message {
                        "BODY[]"
                    } else {
                        "BODY[HEADER.FIELDS (FROM TO SUBJECT DATE MESSAGE-ID REFERENCES)]"
                    };
                    write!(
                        socket,
                        "* 1 FETCH (UID 42 FLAGS (\\Seen) INTERNALDATE \"14-Nov-2026 10:00:00 +0300\" RFC822.SIZE 500 {section} {{{}}}\r\n",
                        body.len()
                    )
                    .unwrap();
                    socket.write_all(body.as_bytes()).unwrap();
                    write!(socket, ")\r\n{tag} OK fetch done\r\n").unwrap();
                } else if upper.contains("LOGOUT") {
                    write!(socket, "* BYE closing\r\n{tag} OK logout\r\n").unwrap();
                    socket.flush().unwrap();
                    break;
                } else {
                    write!(socket, "{tag} OK done\r\n").unwrap();
                }
                socket.flush().unwrap();
            }
        });
        (port, handle)
    }

    #[tokio::test]
    async fn lists_messages_against_a_mock_imap_server() {
        let (port, server) = start_mock_imap(false);
        let account = mock_account(port);
        let result = search_messages(
            &account,
            "secret",
            SearchOptions {
                mailbox: "INBOX".to_string(),
                limit: 10,
                ..SearchOptions::default()
            },
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(result.mailbox, "INBOX");
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].uid, 42);
        assert_eq!(result.messages[0].subject, "Test");
        assert_eq!(result.messages[0].from, "Alice <alice@example.com>");
        assert!(!result.messages[0].unread);
    }

    #[tokio::test]
    async fn collects_forwarded_messages_and_attachments() {
        let (port, server) = start_mock_imap(true);
        let account = mock_account(port);
        let messages = fetch_forward_messages(&account, "secret", "INBOX", &[42], false)
            .await
            .unwrap();
        server.join().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message.subject, "Test");
        assert_eq!(messages[0].attachments.len(), 1);
        assert_eq!(messages[0].attachments[0].filename, "note.txt");
        assert_eq!(messages[0].attachments[0].bytes, b"attachment body");
    }

    #[tokio::test]
    async fn resolves_localized_gmail_all_mail() {
        let (port, server) = start_mock_imap(false);
        let mut account = mock_account(port);
        account.preset = "gmail".to_string();
        account.mailboxes.all = "[Gmail]/All Mail".to_string();
        let result = search_messages(
            &account,
            "secret",
            SearchOptions {
                query: Some("is:unread".to_string()),
                mailbox: "[Gmail]/All Mail".to_string(),
                limit: 10,
                ..SearchOptions::default()
            },
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(result.mailbox, "[Gmail]/Tüm Postalar");
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].mailbox, "[Gmail]/Tüm Postalar");
    }

    #[tokio::test]
    async fn reads_message_and_attachment_against_a_mock_imap_server() {
        let (port, server) = start_mock_imap(true);
        let account = mock_account(port);
        let message = fetch_message(&account, "secret", "INBOX", 42, false)
            .await
            .unwrap();
        server.join().unwrap();
        assert_eq!(message.subject, "Test");
        assert_eq!(message.message_id.as_deref(), Some("<abc@example.com>"));
        assert_eq!(message.body_text.trim(), "hello");
        assert_eq!(message.attachments.len(), 1);
        assert_eq!(message.attachments[0].filename, "note.txt");
    }
}
