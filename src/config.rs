//! Runtime configuration, sourced from the environment (or a local `.env`).

/// Everything needed to talk to the Asana API.
pub struct Config {
    pub token: String,
    pub base_url: String,
}

impl Config {
    /// Build config from the environment. Returns `None` when no access token
    /// is set, which puts the app into offline "demo" mode.
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("ASANA_ACCESS_TOKEN").ok()?;
        if token.trim().is_empty() {
            return None;
        }
        Some(Self {
            token,
            base_url: std::env::var("ASANA_BASE_URL")
                .unwrap_or_else(|_| "https://app.asana.com/api/1.0".to_string()),
        })
    }
}
