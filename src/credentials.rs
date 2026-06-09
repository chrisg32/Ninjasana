//! Secure storage for the Asana Personal Access Token, backed by the OS
//! keychain via the `keyring` crate. The platform-native backend is selected
//! at build time (see Cargo.toml): macOS Keychain, Windows Credential Manager,
//! or the Linux Secret Service.

use anyhow::{Context, Result};
use keyring::Entry;

const SERVICE: &str = "ninjasana";
const ACCOUNT: &str = "asana-pat";

fn entry() -> Result<Entry> {
    Entry::new(SERVICE, ACCOUNT).context("opening keychain entry")
}

/// Persist the token in the keychain.
pub fn store_token(token: &str) -> Result<()> {
    entry()?
        .set_password(token)
        .context("writing token to the keychain")
}

/// Load the token from the keychain, if present.
pub fn load_token() -> Option<String> {
    entry().ok()?.get_password().ok()
}

/// Remove the stored token. Succeeds even if nothing was stored.
pub fn delete_token() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err).context("removing token from the keychain"),
    }
}
