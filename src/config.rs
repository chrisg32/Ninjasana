//! Runtime configuration. The Asana token is resolved from, in order:
//!   1. the `ASANA_ACCESS_TOKEN` environment variable (handy for dev),
//!   2. the OS keychain (written by `ninjasana login`).

const DEFAULT_BASE_URL: &str = "https://app.asana.com/api/1.0";

/// Everything needed to talk to the Asana API.
#[derive(Clone)]
pub struct Config {
    pub token: String,
    pub base_url: String,
}

impl Config {
    /// Resolve config from env then keychain. Returns `None` when no token is
    /// available, which puts the app into offline "demo" mode.
    pub fn load() -> Option<Self> {
        let token = std::env::var("ASANA_ACCESS_TOKEN")
            .ok()
            .filter(|t| !t.trim().is_empty())
            .or_else(crate::credentials::load_token)?;
        Some(Self {
            token,
            base_url: base_url(),
        })
    }

    /// Build config from an explicit token (used during `login` before the
    /// token is persisted).
    pub fn with_token(token: String) -> Self {
        Self {
            token,
            base_url: base_url(),
        }
    }
}

fn base_url() -> String {
    std::env::var("ASANA_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}
