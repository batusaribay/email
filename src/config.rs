use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const APP_DIR_NAME: &str = "email";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    /// Implicit TLS from the first byte (usually port 465/993).
    Tls,
    /// Plain connection upgraded with STARTTLS (usually port 587/143).
    Starttls,
    /// No encryption. Only safe for a local relay.
    None,
}

impl TlsMode {
    pub fn default_imap_port(self) -> u16 {
        match self {
            Self::Tls => 993,
            Self::Starttls | Self::None => 143,
        }
    }

    pub fn default_smtp_port(self) -> u16 {
        match self {
            Self::Tls => 465,
            Self::Starttls => 587,
            Self::None => 25,
        }
    }

    pub fn is_secure(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
}

impl Server {
    pub fn new(host: impl Into<String>, port: u16, tls: TlsMode) -> Self {
        Self {
            host: host.into(),
            port,
            tls,
        }
    }

    pub fn display(&self) -> String {
        let tls = match self.tls {
            TlsMode::Tls => "tls",
            TlsMode::Starttls => "starttls",
            TlsMode::None => "plain",
        };
        format!("{}:{} ({tls})", self.host, self.port)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mailboxes {
    #[serde(default = "inbox_name")]
    pub inbox: String,
    #[serde(default)]
    pub sent: String,
    #[serde(default)]
    pub drafts: String,
    #[serde(default)]
    pub trash: String,
    #[serde(default)]
    pub junk: String,
    #[serde(default)]
    pub archive: String,
    #[serde(default)]
    pub all: String,
}

fn inbox_name() -> String {
    "INBOX".to_string()
}

impl Default for Mailboxes {
    fn default() -> Self {
        Self {
            inbox: inbox_name(),
            sent: String::new(),
            drafts: String::new(),
            trash: String::new(),
            junk: String::new(),
            archive: String::new(),
            all: String::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Account {
    pub email: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub username: String,
    /// Legacy plaintext password from older versions. Never serialized again.
    #[serde(default, skip_serializing)]
    pub password: Option<String>,
    #[serde(default)]
    pub password_saved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_mailbox: Option<String>,
    #[serde(default)]
    pub preset: String,
    pub imap: Server,
    pub smtp: Server,
    #[serde(default)]
    pub mailboxes: Mailboxes,
}

impl Account {
    pub fn username(&self) -> &str {
        if self.username.trim().is_empty() {
            &self.email
        } else {
            &self.username
        }
    }

    pub fn preset_enum(&self) -> Option<crate::presets::Preset> {
        crate::presets::Preset::parse(&self.preset)
    }

    pub fn is_gmail(&self) -> bool {
        self.preset_enum() == Some(crate::presets::Preset::Gmail)
            || self.imap.host.eq_ignore_ascii_case("imap.gmail.com")
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_account: Option<String>,
    #[serde(default)]
    pub accounts: Vec<Account>,
}

impl Config {
    pub fn find(&self, selector: Option<&str>) -> Result<&Account> {
        let selected = if let Some(selector) = selector {
            self.accounts
                .iter()
                .find(|account| account.email.eq_ignore_ascii_case(selector))
                .ok_or_else(|| {
                    anyhow::anyhow!("account '{selector}' not found (see `email account list`)")
                })?
        } else if let Some(default) = self.default_account.as_deref() {
            self.accounts
                .iter()
                .find(|account| account.email.eq_ignore_ascii_case(default))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "default account '{default}' is not configured; run `email account list`"
                    )
                })?
        } else {
            match self.accounts.as_slice() {
                [account] => account,
                [] => bail!("no mail accounts configured. Run `email account add`."),
                _ => bail!(
                    "multiple accounts are configured; pass --account EMAIL or set one with \
                     `email account default EMAIL`"
                ),
            }
        };
        Ok(selected)
    }

    pub fn find_mut(&mut self, email: &str) -> Option<&mut Account> {
        self.accounts
            .iter_mut()
            .find(|account| account.email.eq_ignore_ascii_case(email))
    }

    pub fn upsert(&mut self, account: Account) {
        if let Some(existing) = self.find_mut(&account.email) {
            *existing = account;
        } else {
            self.accounts.push(account);
        }
    }

    pub fn remove(&mut self, email: &str) -> Option<Account> {
        let index = self
            .accounts
            .iter()
            .position(|account| account.email.eq_ignore_ascii_case(email))?;
        let removed = self.accounts.remove(index);
        if self
            .default_account
            .as_deref()
            .is_some_and(|default| default.eq_ignore_ascii_case(&removed.email))
        {
            self.default_account = self.accounts.first().map(|account| account.email.clone());
        }
        Some(removed)
    }

    pub fn set_default(&mut self, email: &str) -> Result<()> {
        let account = self
            .accounts
            .iter()
            .find(|account| account.email.eq_ignore_ascii_case(email))
            .ok_or_else(|| anyhow::anyhow!("account '{email}' not found"))?;
        self.default_account = Some(account.email.clone());
        Ok(())
    }
}

pub fn resolve_config_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(expand_tilde(path));
    }
    if let Some(path) = env::var_os("EMAIL_CONFIG_DIR") {
        return Ok(expand_tilde(&PathBuf::from(path)));
    }
    if let Some(path) = env::var_os("XDG_CONFIG_HOME")
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path).join(APP_DIR_NAME));
    }
    if let Some(home) = home_dir() {
        return Ok(home.join(".config").join(APP_DIR_NAME));
    }
    bail!("could not determine a configuration directory; pass --config-dir DIR")
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

pub fn expand_tilde(path: &Path) -> PathBuf {
    let Some(raw) = path.to_str() else {
        return path.to_path_buf();
    };
    if raw == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    } else if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    path.to_path_buf()
}

pub fn config_path(dir: &Path) -> PathBuf {
    dir.join("config.toml")
}

fn legacy_config_path(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

pub fn load_config(dir: &Path) -> Result<Config> {
    let path = config_path(dir);
    match fs::read_to_string(&path) {
        Ok(text) => {
            toml::from_str(&text).with_context(|| format!("could not parse {}", path.display()))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let legacy = legacy_config_path(dir);
            match fs::read(&legacy) {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .with_context(|| format!("could not parse {}", legacy.display())),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
                Err(err) => {
                    Err(err).with_context(|| format!("could not read {}", legacy.display()))
                }
            }
        }
        Err(err) => Err(err).with_context(|| format!("could not read {}", path.display())),
    }
}

pub fn save_config(dir: &Path, config: &Config) -> Result<()> {
    if config
        .accounts
        .iter()
        .any(|account| account.password.is_some())
    {
        bail!(
            "refusing to save a plaintext password. Unlock the OS keyring and run the \
             command again, or set EMAIL_PASSWORD."
        );
    }
    let text = toml::to_string_pretty(config)?;
    write_private_atomic(&config_path(dir), text.as_bytes())
}

pub fn migrate_legacy_config(dir: &Path, config: &Config) -> Result<bool> {
    let legacy = legacy_config_path(dir);
    if !legacy.is_file() {
        return Ok(false);
    }
    save_config(dir, config)?;
    fs::remove_file(&legacy)
        .with_context(|| format!("could not remove migrated {}", legacy.display()))?;
    Ok(true)
}

fn ensure_config_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)
        .with_context(|| format!("could not create config directory {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("could not restrict permissions on {}", dir.display()))?;
    }
    Ok(())
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    ensure_config_dir(parent)?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("email");
    let tmp = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));

    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.create(true).write(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("could not create {}", tmp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("could not write {}", tmp.display()))?;
        file.flush()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    })();
    if let Err(err) = result {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }

    #[cfg(windows)]
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp, path).with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::Preset;

    fn account(email: &str) -> Account {
        let defaults = Preset::detect(email).defaults();
        Account {
            email: email.to_string(),
            display_name: String::new(),
            username: email.to_string(),
            password: None,
            password_saved: false,
            last_mailbox: None,
            preset: Preset::detect(email).name().to_string(),
            imap: defaults.imap,
            smtp: defaults.smtp,
            mailboxes: defaults.mailboxes,
        }
    }

    #[test]
    fn selects_single_and_default_accounts() {
        let mut config = Config::default();
        config.upsert(account("a@example.com"));
        assert_eq!(config.find(None).unwrap().email, "a@example.com");
        config.upsert(account("b@example.com"));
        assert!(config.find(None).is_err());
        config.set_default("b@example.com").unwrap();
        assert_eq!(config.find(None).unwrap().email, "b@example.com");
    }

    #[test]
    fn never_serializes_a_legacy_password() {
        let mut account = account("a@example.com");
        account.password = Some("super-secret".to_string());
        account.password_saved = true;
        let json = serde_json::to_string(&account).unwrap();
        assert!(!json.contains("super-secret"));
        assert!(json.contains("\"password_saved\":true"));
    }

    #[test]
    fn migrates_legacy_json_to_toml() {
        let dir = std::env::temp_dir().join(format!(
            "email-config-test-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut config = Config::default();
        config.upsert(account("a@example.com"));
        let legacy = legacy_config_path(&dir);
        fs::write(&legacy, serde_json::to_vec_pretty(&config).unwrap()).unwrap();

        let loaded = load_config(&dir).unwrap();
        assert_eq!(loaded.accounts.len(), 1);
        assert!(migrate_legacy_config(&dir, &loaded).unwrap());
        assert!(!legacy.exists());
        let text = fs::read_to_string(config_path(&dir)).unwrap();
        assert!(text.contains("[[accounts]]"));
        assert_eq!(
            load_config(&dir).unwrap().accounts[0].email,
            "a@example.com"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn removes_default_and_picks_another() {
        let mut config = Config::default();
        config.upsert(account("a@example.com"));
        config.upsert(account("b@example.com"));
        config.set_default("a@example.com").unwrap();
        config.remove("a@example.com");
        assert_eq!(config.default_account.as_deref(), Some("b@example.com"));
    }
}
