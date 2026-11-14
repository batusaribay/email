use anyhow::{Context, Result, bail};
use keyring::{Entry, Error as KeyringError};

const SERVICE: &str = "mail-client";

fn entry(email: &str) -> Result<Entry> {
    let user = email.trim().to_ascii_lowercase();
    if user.is_empty() {
        bail!("cannot use an empty account email for the keyring");
    }
    Entry::new(SERVICE, &user).with_context(|| {
        "could not access the OS keyring; unlock your login keyring and try again".to_string()
    })
}

pub fn store_password(email: &str, password: &str) -> Result<()> {
    entry(email)?
        .set_password(password)
        .with_context(|| format!("could not store the password for {email} in the OS keyring"))
}

pub fn get_password(email: &str) -> Result<Option<String>> {
    match entry(email)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(error).with_context(|| {
            format!("could not read the password for {email} from the OS keyring")
        }),
    }
}

pub fn delete_password(email: &str) -> Result<bool> {
    match entry(email)?.delete_credential() {
        Ok(()) => Ok(true),
        Err(KeyringError::NoEntry) => Ok(false),
        Err(error) => Err(error).with_context(|| {
            format!("could not delete the password for {email} from the OS keyring")
        }),
    }
}
