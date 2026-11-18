use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use lettre::address::{Address, Envelope};
use lettre::message::Mailboxes;
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncTransport, Tokio1Executor};

use crate::config::{Account, TlsMode};
use crate::mime::{BuiltMessage, OutgoingMessage};
use crate::model::SendResult;
use crate::presets::Preset;

pub async fn send(
    account: &Account,
    password: &str,
    message: &OutgoingMessage,
    built: &BuiltMessage,
) -> Result<SendResult> {
    let from: Address = account
        .email
        .parse()
        .with_context(|| format!("invalid account email '{}'", account.email))?;
    let mut recipients = Vec::new();
    recipients.extend(parse_mailboxes(&message.to, "To")?);
    if let Some(cc) = message.cc.as_deref() {
        recipients.extend(parse_mailboxes(cc, "Cc")?);
    }
    if let Some(bcc) = message.bcc.as_deref() {
        recipients.extend(parse_mailboxes(bcc, "Bcc")?);
    }
    if recipients.is_empty() {
        bail!("at least one recipient is required");
    }
    let mut seen = HashSet::new();
    recipients.retain(|mailbox| seen.insert(mailbox.email.to_string().to_ascii_lowercase()));
    let envelope = Envelope::new(
        Some(from),
        recipients
            .iter()
            .map(|mailbox| mailbox.email.clone())
            .collect(),
    )
    .context("could not build the SMTP envelope")?;
    let transport = transport(account, password)?;
    transport
        .send_raw(&envelope, &built.bytes)
        .await
        .map_err(|error| smtp_error(error, account))?;
    Ok(SendResult {
        message_id: built.message_id.clone(),
        from: account.email.clone(),
        recipients: recipients
            .iter()
            .map(|mailbox| mailbox.email.to_string())
            .collect(),
    })
}

pub async fn test(account: &Account, password: &str) -> Result<bool> {
    let transport = transport(account, password)?;
    transport
        .test_connection()
        .await
        .map_err(|error| smtp_error(error, account))
}

fn transport(account: &Account, password: &str) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    if account.smtp.host.trim().is_empty() {
        bail!("account '{}' has no SMTP host", account.email);
    }
    if account.smtp.port == 0 {
        bail!("account '{}' has an invalid SMTP port", account.email);
    }
    let credentials = Credentials::new(account.username().to_string(), password.to_string());
    let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&account.smtp.host)
        .port(account.smtp.port)
        .credentials(credentials)
        .timeout(Some(Duration::from_secs(90)));

    builder = match account.smtp.tls {
        TlsMode::Tls => {
            let tls = TlsParameters::new(account.smtp.host.clone())
                .context("could not initialize SMTP TLS")?;
            builder.tls(Tls::Wrapper(tls))
        }
        TlsMode::Starttls => {
            let tls = TlsParameters::new(account.smtp.host.clone())
                .context("could not initialize SMTP TLS")?;
            builder.tls(Tls::Required(tls))
        }
        TlsMode::None => builder.tls(Tls::None),
    };
    Ok(builder.build::<Tokio1Executor>())
}

fn parse_mailboxes(value: &str, field: &str) -> Result<Vec<lettre::message::Mailbox>> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mailboxes: Mailboxes = value
        .parse()
        .with_context(|| format!("could not parse {field} addresses: {value}"))?;
    Ok(mailboxes.into_iter().collect())
}

fn smtp_error(error: lettre::transport::smtp::Error, account: &Account) -> anyhow::Error {
    let text = error.to_string();
    let lower = text.to_ascii_lowercase();
    let mut message = format!("SMTP send failed for {}: {error}", account.email);
    if lower.contains("authentication") || lower.contains("credentials") || lower.contains("auth") {
        if account.is_gmail() {
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
    }
    anyhow::anyhow!(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_addresses() {
        let mailboxes = parse_mailboxes("Alice <a@example.com>, b@example.com", "To").unwrap();
        assert_eq!(mailboxes.len(), 2);
        assert_eq!(mailboxes[0].email.to_string(), "a@example.com");
        assert_eq!(mailboxes[1].email.to_string(), "b@example.com");
    }

    #[test]
    fn rejects_invalid_addresses() {
        assert!(parse_mailboxes("not an address", "To").is_err());
    }

    async fn start_mock_smtp() -> (u16, tokio::task::JoinHandle<(String, Vec<String>)>) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut reader = BufReader::new(read);
            write.write_all(b"220 mock SMTP ready\r\n").await.unwrap();
            let mut data = String::new();
            let mut recipients = Vec::new();
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    break;
                }
                let command = line.trim_end().to_string();
                let upper = command.to_ascii_uppercase();
                if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                    write
                        .write_all(
                            b"250-mock\r\n250-AUTH PLAIN LOGIN\r\n250-8BITMIME\r\n250 SIZE 1000000\r\n",
                        )
                        .await
                        .unwrap();
                } else if upper.starts_with("AUTH PLAIN") {
                    if command.split_whitespace().count() < 3 {
                        write.write_all(b"334 \r\n").await.unwrap();
                        line.clear();
                        reader.read_line(&mut line).await.unwrap();
                    }
                    write
                        .write_all(b"235 2.7.0 authenticated\r\n")
                        .await
                        .unwrap();
                } else if upper.starts_with("AUTH LOGIN") {
                    write.write_all(b"334 VXNlcm5hbWU6\r\n").await.unwrap();
                    line.clear();
                    reader.read_line(&mut line).await.unwrap();
                    write.write_all(b"334 UGFzc3dvcmQ6\r\n").await.unwrap();
                    line.clear();
                    reader.read_line(&mut line).await.unwrap();
                    write
                        .write_all(b"235 2.7.0 authenticated\r\n")
                        .await
                        .unwrap();
                } else if upper.starts_with("MAIL FROM") {
                    write.write_all(b"250 2.1.0 OK\r\n").await.unwrap();
                } else if upper.starts_with("RCPT TO") {
                    recipients.push(command.clone());
                    write.write_all(b"250 2.1.5 OK\r\n").await.unwrap();
                } else if upper == "DATA" {
                    write
                        .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                        .await
                        .unwrap();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                            break;
                        }
                        if line == ".\r\n" {
                            break;
                        }
                        data.push_str(&line);
                    }
                    write.write_all(b"250 2.0.0 queued\r\n").await.unwrap();
                } else if upper == "QUIT" {
                    write.write_all(b"221 2.0.0 bye\r\n").await.unwrap();
                    break;
                } else {
                    write.write_all(b"250 OK\r\n").await.unwrap();
                }
                write.flush().await.unwrap();
            }
            (data, recipients)
        });
        (port, handle)
    }

    #[tokio::test]
    async fn sends_message_to_mock_smtp() {
        use crate::config::Server;
        use crate::mime::{OutgoingMessage, build_message};

        let (port, server) = start_mock_smtp().await;
        let account = Account {
            email: "alice@example.com".to_string(),
            display_name: "Alice".to_string(),
            username: "alice@example.com".to_string(),
            password: None,
            password_saved: false,
            last_mailbox: None,
            preset: "custom".to_string(),
            imap: Server::new("127.0.0.1", 1, TlsMode::None),
            smtp: Server::new("127.0.0.1", port, TlsMode::None),
            mailboxes: crate::config::Mailboxes::default(),
        };
        let outgoing = OutgoingMessage {
            from_name: "Alice".to_string(),
            from_email: "alice@example.com".to_string(),
            to: "Bob <bob@example.com>".to_string(),
            cc: Some("carol@example.com".to_string()),
            bcc: Some("hidden@example.com".to_string()),
            subject: "Test".to_string(),
            body: "hello".to_string(),
            html: false,
            attachments: Vec::new(),
            raw_attachments: Vec::new(),
            in_reply_to: None,
            references: None,
        };
        let built = build_message(&outgoing).unwrap();
        let result = send(&account, "secret", &outgoing, &built).await.unwrap();
        let (data, recipients) = server.await.unwrap();
        assert_eq!(result.recipients.len(), 3);
        assert!(data.contains("To: Bob <bob@example.com>\r\n"));
        assert!(data.contains("Subject: Test\r\n"));
        assert!(!data.contains("Bcc:"));
        assert!(
            recipients
                .iter()
                .any(|rcpt| rcpt.contains("hidden@example.com"))
        );
    }
}
