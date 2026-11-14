use crate::config::{Mailboxes, Server, TlsMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Gmail,
    Outlook,
    Yahoo,
    Icloud,
    Fastmail,
    Custom,
}

pub struct PresetDefaults {
    pub imap: Server,
    pub smtp: Server,
    pub mailboxes: Mailboxes,
    pub note: &'static str,
}

impl Preset {
    pub fn name(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::Outlook => "outlook",
            Self::Yahoo => "yahoo",
            Self::Icloud => "icloud",
            Self::Fastmail => "fastmail",
            Self::Custom => "custom",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "gmail" | "google" => Some(Self::Gmail),
            "outlook" | "microsoft" | "office365" | "hotmail" => Some(Self::Outlook),
            "yahoo" => Some(Self::Yahoo),
            "icloud" | "apple" | "me" => Some(Self::Icloud),
            "fastmail" => Some(Self::Fastmail),
            "custom" | "other" => Some(Self::Custom),
            _ => None,
        }
    }

    pub fn detect(email: &str) -> Self {
        let domain = email
            .rsplit_once('@')
            .map(|(_, domain)| domain.to_ascii_lowercase())
            .unwrap_or_default();
        match domain.as_str() {
            "gmail.com" | "googlemail.com" => Self::Gmail,
            "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => Self::Outlook,
            "yahoo.com" | "ymail.com" | "rocketmail.com" => Self::Yahoo,
            "icloud.com" | "me.com" | "mac.com" => Self::Icloud,
            "fastmail.com" | "fastmail.fm" => Self::Fastmail,
            _ => Self::Custom,
        }
    }

    pub fn defaults(self) -> PresetDefaults {
        match self {
            Self::Gmail => PresetDefaults {
                imap: Server::new("imap.gmail.com", 993, TlsMode::Tls),
                smtp: Server::new("smtp.gmail.com", 465, TlsMode::Tls),
                mailboxes: Mailboxes {
                    inbox: "INBOX".to_string(),
                    sent: "[Gmail]/Sent Mail".to_string(),
                    drafts: "[Gmail]/Drafts".to_string(),
                    trash: "[Gmail]/Trash".to_string(),
                    junk: "[Gmail]/Spam".to_string(),
                    archive: String::new(),
                    all: "[Gmail]/All Mail".to_string(),
                },
                note: "Gmail requires 2-Step Verification and a 16-character app password. \
                       Normal Google passwords are rejected. Create one at \
                       https://myaccount.google.com/apppasswords",
            },
            Self::Outlook => PresetDefaults {
                imap: Server::new("outlook.office365.com", 993, TlsMode::Tls),
                smtp: Server::new("smtp-mail.outlook.com", 587, TlsMode::Starttls),
                mailboxes: Mailboxes {
                    inbox: "INBOX".to_string(),
                    sent: "Sent Items".to_string(),
                    drafts: "Drafts".to_string(),
                    trash: "Deleted Items".to_string(),
                    junk: "Junk Email".to_string(),
                    archive: "Archive".to_string(),
                    all: String::new(),
                },
                note: "Microsoft accounts commonly require OAuth instead of password authentication. \
                       App passwords may be unavailable.",
            },
            Self::Yahoo => PresetDefaults {
                imap: Server::new("imap.mail.yahoo.com", 993, TlsMode::Tls),
                smtp: Server::new("smtp.mail.yahoo.com", 465, TlsMode::Tls),
                mailboxes: Mailboxes {
                    inbox: "INBOX".to_string(),
                    sent: "Sent".to_string(),
                    drafts: "Draft".to_string(),
                    trash: "Trash".to_string(),
                    junk: "Bulk Mail".to_string(),
                    archive: "Archive".to_string(),
                    all: String::new(),
                },
                note: "Yahoo requires an app password generated in Account Security.",
            },
            Self::Icloud => PresetDefaults {
                imap: Server::new("imap.mail.me.com", 993, TlsMode::Tls),
                smtp: Server::new("smtp.mail.me.com", 587, TlsMode::Starttls),
                mailboxes: Mailboxes {
                    inbox: "INBOX".to_string(),
                    sent: "Sent Messages".to_string(),
                    drafts: "Drafts".to_string(),
                    trash: "Deleted Messages".to_string(),
                    junk: "Junk".to_string(),
                    archive: "Archive".to_string(),
                    all: String::new(),
                },
                note: "iCloud requires an app-specific password from account.apple.com.",
            },
            Self::Fastmail => PresetDefaults {
                imap: Server::new("imap.fastmail.com", 993, TlsMode::Tls),
                smtp: Server::new("smtp.fastmail.com", 465, TlsMode::Tls),
                mailboxes: Mailboxes {
                    inbox: "INBOX".to_string(),
                    sent: "Sent".to_string(),
                    drafts: "Drafts".to_string(),
                    trash: "Trash".to_string(),
                    junk: "Junk Mail".to_string(),
                    archive: "Archive".to_string(),
                    all: String::new(),
                },
                note: "Fastmail requires an app password from Settings > Privacy & Security.",
            },
            Self::Custom => PresetDefaults {
                imap: Server::new("", 993, TlsMode::Tls),
                smtp: Server::new("", 587, TlsMode::Starttls),
                mailboxes: Mailboxes::default(),
                note: "Review the IMAP/SMTP ports and TLS modes for your server.",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_providers() {
        assert_eq!(Preset::detect("a@gmail.com"), Preset::Gmail);
        assert_eq!(Preset::detect("a@outlook.com"), Preset::Outlook);
        assert_eq!(Preset::detect("a@icloud.com"), Preset::Icloud);
        assert_eq!(Preset::detect("a@example.org"), Preset::Custom);
    }

    #[test]
    fn parses_names_case_insensitively() {
        assert_eq!(Preset::parse("GMAIL"), Some(Preset::Gmail));
        assert_eq!(Preset::parse("fastmail"), Some(Preset::Fastmail));
        assert_eq!(Preset::parse("nope"), None);
    }
}
