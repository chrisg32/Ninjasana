//! Non-TUI subcommands: `login` and `logout`.

use anyhow::{Result, bail};

use crate::asana::Client;
use crate::config::Config;
use crate::credentials;

/// Prompt for a Personal Access Token, validate it, and store it.
pub async fn login() -> Result<()> {
    println!("Create a Personal Access Token at: https://app.asana.com/0/my-apps");
    let token = rpassword::prompt_password("Paste your Asana Personal Access Token: ")
        .map(|t| t.trim().to_string())
        .unwrap_or_default();

    if token.is_empty() {
        bail!("no token entered");
    }

    print!("Validating… ");
    let user = Client::new(Config::with_token(token.clone()))
        .me()
        .await
        .map_err(|e| anyhow::anyhow!("could not validate token with Asana: {e:#}"))?;

    credentials::store_token(&token)?;
    println!("done.");
    println!("✓ Logged in as {} ({}).", user.name, user.gid);
    println!("  Token stored securely in your keychain.");
    Ok(())
}

/// Remove any stored token.
pub fn logout() -> Result<()> {
    credentials::delete_token()?;
    println!("✓ Logged out. Stored token removed.");
    Ok(())
}
