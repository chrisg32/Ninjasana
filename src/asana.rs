//! A thin async Asana REST client built on `reqwest` + `serde`.
//!
//! Asana ships no official Rust SDK, so we call the REST API directly. Every
//! Asana response wraps its payload in a top-level `{ "data": ... }` envelope,
//! which `DataEnvelope<T>` models generically.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::Config;

/// A result handed back to the UI thread from an async Asana call.
#[derive(Debug, Clone)]
pub enum AsanaUpdate {
    /// Successfully authenticated; carries the current user's display name.
    Me(String),
    /// Something went wrong; carries a human-readable message.
    Error(String),
}

/// Generic `{ "data": T }` envelope used by every Asana endpoint.
#[derive(Debug, Deserialize)]
struct DataEnvelope<T> {
    data: T,
}

/// The authenticated Asana user (`/users/me`).
#[derive(Debug, Clone, Deserialize)]
pub struct User {
    // Retained from the API payload for when views start keying off the user.
    #[allow(dead_code)]
    pub gid: String,
    pub name: String,
}

pub struct Client {
    http: reqwest::Client,
    config: Config,
}

impl Client {
    pub fn new(config: Config) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
        }
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.config.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    /// Fetch the authenticated user. Doubles as a connectivity/auth check.
    pub async fn me(&self) -> Result<User> {
        let envelope: DataEnvelope<User> = self
            .http
            .get(self.url("users/me"))
            .bearer_auth(&self.config.token)
            .send()
            .await
            .context("sending request to Asana")?
            .error_for_status()
            .context("Asana returned an error status")?
            .json()
            .await
            .context("decoding the Asana response")?;
        Ok(envelope.data)
    }
}
