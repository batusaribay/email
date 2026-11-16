use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use rand::RngCore;

#[derive(Debug, Clone)]
pub struct OutgoingMessage {
    pub from_name: String,
    pub from_email: String,
    pub to: String,
    pub cc: Option<String>,
    pub bcc: Option<String>,
    pub subject: String,
    pub body: String,
    pub html: bool,
    pub attachments: Vec<PathBuf>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
}

pub struct BuiltMessage {
    pub bytes: Vec<u8>,
    pub message_id: String,
}

pub fn build_message(message: &OutgoingMessage) -> Result<BuiltMessage> {
    let to = sanitize_header(&message.to);
    if to.trim().is_empty() {
        bail!("at least one recipient is required");
    }
    if message.from_email.trim().is_empty() {
        bail!("the account has no From address");
    }

    let from_email = sanitize_header(&message.from_email);
    let from_name = sanitize_header(&message.from_name);
    let from = if from_name.is_empty() {
        from_email.clone()
    } else {
        format!("{} <{from_email}>", encode_rfc2047(&from_name))
    };
    let message_id = make_message_id(&from_email);
    let subject = sanitize_header(&message.subject);

    let mut headers = String::new();
    headers.push_str("MIME-Version: 1.0\r\n");
    headers.push_str(&format!(
        "Date: {}\r\n",
        chrono::Local::now().format("%a, %d %b %Y %H:%M:%S %z")
    ));
    headers.push_str(&format!("From: {from}\r\n"));
    headers.push_str(&format!("To: {}\r\n", encode_address_header(&to)));
    if let Some(cc) = message
        .cc
        .as_deref()
        .map(sanitize_header)
        .filter(|value| !value.trim().is_empty())
    {
        headers.push_str(&format!("Cc: {}\r\n", encode_address_header(&cc)));
    }
    if !subject.is_empty() {
        headers.push_str(&format!("Subject: {}\r\n", encode_rfc2047(&subject)));
    }
    if let Some(value) = message
        .in_reply_to
        .as_deref()
        .map(sanitize_header)
        .filter(|value| !value.trim().is_empty())
    {
        headers.push_str(&format!("In-Reply-To: {value}\r\n"));
    }
    if let Some(value) = message
        .references
        .as_deref()
        .map(sanitize_header)
        .filter(|value| !value.trim().is_empty())
    {
        headers.push_str(&format!("References: {value}\r\n"));
    }
    headers.push_str(&format!("Message-ID: {message_id}\r\n"));
    headers.push_str(concat!(
        "X-Mailer: email/",
        env!("CARGO_PKG_VERSION"),
        "\r\n"
    ));
    // Bcc is deliberately omitted from the message headers. It is only used for
    // the SMTP envelope so other recipients never see the blind-copy addresses.

    let body = body_entity(&message.body, message.html);
    let mut output = Vec::new();
    if message.attachments.is_empty() {
        output.extend_from_slice(headers.as_bytes());
        output.extend_from_slice(body.as_bytes());
    } else {
        let boundary = random_boundary();
        headers.push_str(&format!(
            "Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n"
        ));
        output.extend_from_slice(headers.as_bytes());
        output.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        output.extend_from_slice(body.as_bytes());
        for path in &message.attachments {
            let attachment = attachment_entity(path)?;
            output.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            output.extend_from_slice(attachment.as_bytes());
        }
        output.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    }
    Ok(BuiltMessage {
        bytes: output,
        message_id,
    })
}

fn body_entity(body: &str, html: bool) -> String {
    let mime = if html { "text/html" } else { "text/plain" };
    format!(
        "Content-Type: {mime}; charset=\"UTF-8\"\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{}\r\n",
        base64_wrapped(body.as_bytes())
    )
}

fn attachment_entity(path: &Path) -> Result<String> {
    let bytes =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment")
        .replace(['\r', '\n'], " ");
    let mime = mime_type_for(path);
    Ok(format!(
        "Content-Type: {mime}; {}\r\n\
         Content-Transfer-Encoding: base64\r\n\
         Content-Disposition: attachment; {}\r\n\r\n{}",
        filename_param(&filename, "name"),
        filename_param(&filename, "filename"),
        base64_wrapped(&bytes)
    ))
}

fn encode_address_header(value: &str) -> String {
    let mut output = Vec::new();
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
                output.push(encode_mailbox(current.trim()));
                current.clear();
            }
            _ => current.push(character),
        }
    }
    if !current.trim().is_empty() {
        output.push(encode_mailbox(current.trim()));
    }
    output.join(", ")
}

fn encode_mailbox(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    let Some(start) = value.rfind('<') else {
        return value.to_string();
    };
    let Some(end) = value.rfind('>') else {
        return value.to_string();
    };
    if end <= start {
        return value.to_string();
    }
    let name = value[..start].trim().trim_matches('"').trim();
    if name.is_empty() {
        return value.to_string();
    }
    format!(
        "{} <{}>",
        encode_rfc2047(name),
        value[start + 1..end].trim()
    )
}

fn filename_param(filename: &str, parameter: &str) -> String {
    if filename.is_ascii()
        && !filename
            .chars()
            .any(|character| matches!(character, '"' | '\\' | '\r' | '\n'))
    {
        format!("{parameter}=\"{filename}\"")
    } else {
        format!(
            "{parameter}*=UTF-8''{}",
            utf8_percent_encode(filename, NON_ALPHANUMERIC)
        )
    }
}

fn mime_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("txt") | Some("log") => "text/plain",
        Some("rs") | Some("py") | Some("js") | Some("ts") | Some("tsx") | Some("jsx")
        | Some("css") | Some("sh") | Some("c") | Some("h") | Some("cpp") | Some("hpp")
        | Some("go") | Some("java") | Some("rb") | Some("php") | Some("sql") | Some("ini")
        | Some("conf") => "text/plain",
        Some("md") => "text/markdown",
        Some("html") | Some("htm") => "text/html",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("toml") => "application/toml",
        Some("yaml") | Some("yml") => "application/yaml",
        Some("xml") => "application/xml",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("gz") | Some("tgz") => "application/gzip",
        Some("tar") => "application/x-tar",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("mp3") => "audio/mpeg",
        Some("mp4") => "video/mp4",
        Some("doc") => "application/msword",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xls") => "application/vnd.ms-excel",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("ppt") => "application/vnd.ms-powerpoint",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

fn base64_wrapped(bytes: &[u8]) -> String {
    let encoded = STANDARD.encode(bytes);
    if encoded.is_empty() {
        return String::new();
    }
    let mut output = String::with_capacity(encoded.len() + encoded.len() / 76 * 2 + 2);
    for chunk in encoded.as_bytes().chunks(76) {
        output.push_str(std::str::from_utf8(chunk).expect("base64 is ASCII"));
        output.push_str("\r\n");
    }
    output
}

fn random_boundary() -> String {
    let mut bytes = [0_u8; 18];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("email-{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn make_message_id(from: &str) -> String {
    let domain = from
        .rsplit_once('@')
        .map(|(_, domain)| domain)
        .filter(|domain| !domain.is_empty())
        .unwrap_or("localhost");
    let mut random = [0_u8; 12];
    rand::thread_rng().fill_bytes(&mut random);
    format!(
        "<{}.{}@{}>",
        chrono::Utc::now().timestamp_millis(),
        URL_SAFE_NO_PAD.encode(random),
        domain
    )
}

fn sanitize_header(value: &str) -> String {
    value
        .replace("\r\n", " ")
        .chars()
        .map(|character| {
            if character == '\t' || character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// Encode non-ASCII header text as RFC 2047 base64 words.
pub fn encode_rfc2047(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    for (index, character) in value.char_indices() {
        let end = index + character.len_utf8();
        if end - start > 45 && index > start {
            chunks.push(&value[start..index]);
            start = index;
        }
    }
    if start < value.len() {
        chunks.push(&value[start..]);
    }
    chunks
        .into_iter()
        .map(|chunk| format!("=?UTF-8?B?{}?=", STANDARD.encode(chunk.as_bytes())))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convert HTML to readable plain text without pulling in a browser engine.
pub fn strip_html(html: &str) -> String {
    let html = remove_block(&html.to_ascii_lowercase(), html, "head");
    let html = remove_block(&html.to_ascii_lowercase(), &html, "script");
    let html = remove_block(&html.to_ascii_lowercase(), &html, "style");

    let bytes = html.as_bytes();
    let mut output = String::with_capacity(html.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'<' {
            if html[index..].starts_with("<!--") {
                if let Some(end) = html[index + 4..].find("-->") {
                    index += 4 + end + 3;
                    continue;
                }
            }
            let Some(end) = find_tag_end(&html, index + 1) else {
                output.push('<');
                index += 1;
                continue;
            };
            let tag = &html[index + 1..end];
            let tag = tag.trim_start_matches('/').trim_start();
            let name: String = tag
                .chars()
                .take_while(|character| character.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            match name.as_str() {
                "br" => output.push('\n'),
                "p" | "div" | "tr" | "section" | "article" | "header" | "footer" | "blockquote" => {
                    output.push('\n')
                }
                "li" => output.push_str("\n- "),
                "td" | "th" => output.push(' '),
                "hr" => output.push_str("\n---\n"),
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => output.push('\n'),
                _ => {}
            }
            index = end + 1;
        } else if bytes[index] == b'&' {
            if let Some(end) = html[index..].find(';') {
                if end <= 12 {
                    let entity = &html[index + 1..index + end];
                    if let Some(character) = decode_entity(entity) {
                        output.push(character);
                        index += end + 1;
                        continue;
                    }
                }
            }
            output.push('&');
            index += 1;
        } else {
            let character = html[index..]
                .chars()
                .next()
                .expect("index always points at a character boundary");
            output.push(character);
            index += character.len_utf8();
        }
    }
    normalize_text(&output)
}

fn remove_block(lower: &str, original: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut output = String::with_capacity(original.len());
    let mut cursor = 0;
    while let Some(start) = find_tag(lower, cursor, &open) {
        output.push_str(&original[cursor..start]);
        if let Some(relative_end) = lower[start..].find(&close) {
            cursor = start + relative_end + close.len();
        } else {
            cursor = original.len();
            break;
        }
    }
    output.push_str(&original[cursor..]);
    output
}

fn find_tag(lower: &str, from: usize, pattern: &str) -> Option<usize> {
    let mut cursor = from;
    while let Some(relative) = lower[cursor..].find(pattern) {
        let start = cursor + relative;
        let after = start + pattern.len();
        let next = lower[after..].chars().next();
        if next.is_none_or(|character| {
            character == '>' || character == '/' || character.is_ascii_whitespace()
        }) {
            return Some(start);
        }
        cursor = after;
    }
    None
}

fn find_tag_end(html: &str, start: usize) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (offset, character) in html[start..].char_indices() {
        match character {
            '"' | '\'' if quote.is_none() => quote = Some(character),
            current if quote == Some(current) => quote = None,
            '>' if quote.is_none() => return Some(start + offset),
            _ => {}
        }
    }
    None
}

fn decode_entity(entity: &str) -> Option<char> {
    if let Some(number) = entity.strip_prefix('#') {
        let value = if let Some(hex) = number
            .strip_prefix('x')
            .or_else(|| number.strip_prefix('X'))
        {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            number.parse::<u32>().ok()?
        };
        return char::from_u32(value);
    }
    Some(match entity.to_ascii_lowercase().as_str() {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "rsquo" => '’',
        "lsquo" => '‘',
        "ldquo" => '“',
        "rdquo" => '”',
        "bull" => '•',
        "middot" => '·',
        _ => return None,
    })
}

fn normalize_text(text: &str) -> String {
    let mut lines = Vec::new();
    for raw_line in text.lines() {
        let mut line = String::new();
        let mut last_space = false;
        for character in raw_line.trim_end().chars() {
            if character == ' ' || character == '\t' {
                if !last_space {
                    line.push(' ');
                    last_space = true;
                }
            } else {
                line.push(character);
                last_space = false;
            }
        }
        if line.is_empty() && lines.last().is_some_and(|last: &String| last.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    let first_non_empty = lines
        .iter()
        .position(|line| !line.is_empty())
        .unwrap_or(lines.len());
    lines.drain(..first_non_empty);
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(attachments: Vec<PathBuf>) -> OutgoingMessage {
        OutgoingMessage {
            from_name: "Alice Örnek".to_string(),
            from_email: "alice@example.com".to_string(),
            to: "bob@example.com".to_string(),
            cc: Some("carol@example.com".to_string()),
            bcc: Some("hidden@example.com".to_string()),
            subject: "Merhaba dünya".to_string(),
            body: "Hello\nworld".to_string(),
            html: false,
            attachments,
            in_reply_to: Some("<abc@example.com>".to_string()),
            references: Some("<abc@example.com>".to_string()),
        }
    }

    #[test]
    fn builds_message_without_leaking_bcc() {
        let built = build_message(&sample(Vec::new())).unwrap();
        let raw = String::from_utf8(built.bytes).unwrap();
        assert!(raw.contains("From: =?UTF-8?B?"));
        assert!(raw.contains("To: bob@example.com\r\n"));
        assert!(raw.contains("Cc: carol@example.com\r\n"));
        assert!(!raw.contains("Bcc:"));
        assert!(raw.contains("Subject: =?UTF-8?B?"));
        assert!(raw.contains("In-Reply-To: <abc@example.com>\r\n"));
        assert!(raw.contains(&format!("Message-ID: {}", built.message_id)));
        assert!(raw.contains("Content-Type: text/plain; charset=\"UTF-8\""));
        assert!(raw.ends_with("\r\n"));
    }

    #[test]
    fn builds_attachment_message() {
        let path =
            std::env::temp_dir().join(format!("email-test-{}-note.toml", std::process::id()));
        std::fs::write(&path, "[test]\nvalue = 1\n").unwrap();
        let built = build_message(&sample(vec![path.clone()])).unwrap();
        let raw = String::from_utf8(built.bytes).unwrap();
        let _ = std::fs::remove_file(path);
        assert!(raw.contains("Content-Type: multipart/mixed; boundary=\""));
        assert!(raw.contains("Content-Type: application/toml; name=\"email-test-"));
        assert!(!raw.contains("name=name="));
        assert!(raw.contains("Content-Disposition: attachment; filename=\"email-test-"));
    }

    #[test]
    fn encodes_non_ascii_display_names() {
        let mut message = sample(Vec::new());
        message.to = "\"Şükrü Kaya\" <s@example.com>, b@example.com".to_string();
        let raw = String::from_utf8(build_message(&message).unwrap().bytes).unwrap();
        assert!(raw.contains("To: =?UTF-8?B?"));
        assert!(raw.contains("<s@example.com>, b@example.com"));
    }

    #[test]
    fn sanitizes_header_injection() {
        let mut message = sample(Vec::new());
        message.to = "bob@example.com\r\nBcc: evil@example.com".to_string();
        let raw = String::from_utf8(build_message(&message).unwrap().bytes).unwrap();
        assert!(!raw.contains("\r\nBcc: evil@example.com\r\n"));
        assert!(raw.contains("To: bob@example.com Bcc: evil@example.com\r\n"));
    }

    #[test]
    fn decodes_own_encoded_words() {
        let encoded = encode_rfc2047("İyi günler — toplantı");
        assert!(encoded.starts_with("=?UTF-8?B?"));
    }
}
