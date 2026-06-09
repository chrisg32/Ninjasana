//! Command-line surface.
//!
//!   ninjasana                 Open the full three-pane Asana view.
//!   ninjasana <task_url>      Open just the detail pane for one task.
//!   ninjasana login           Store an Asana Personal Access Token.
//!   ninjasana logout          Remove the stored token.

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "ninjasana",
    version,
    about = "A mouse-native terminal UI for Asana",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// An Asana task URL (or bare task id) to open directly in detail view.
    #[arg(value_name = "TASK_URL")]
    pub task_url: Option<String>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Authenticate with Asana and store a Personal Access Token.
    Login,
    /// Remove the stored Asana token.
    Logout,
}

/// Extract a task gid from an Asana task URL (or accept a bare numeric id).
///
/// Handles the common shapes:
///   https://app.asana.com/0/<project>/<task>
///   https://app.asana.com/1/<workspace>/project/<p>/task/<task>
pub fn parse_task_gid(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Ok(trimmed.to_string());
    }

    let path = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    let is_gid = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());

    // Prefer the segment right after a "task" marker (newer URL form).
    if let Some(pos) = segments.iter().position(|s| *s == "task")
        && let Some(gid) = segments.get(pos + 1)
        && is_gid(gid)
    {
        return Ok((*gid).to_string());
    }

    // Otherwise the last all-numeric segment (older `/0/<project>/<task>`).
    if let Some(gid) = segments.iter().rev().find(|s| is_gid(s)) {
        return Ok((*gid).to_string());
    }

    bail!("could not find a task id in: {input}");
}

#[cfg(test)]
mod tests {
    use super::parse_task_gid;

    #[test]
    fn bare_id() {
        assert_eq!(parse_task_gid("1201234567890").unwrap(), "1201234567890");
    }

    #[test]
    fn classic_url() {
        let url = "https://app.asana.com/0/1199999999/1201234567890";
        assert_eq!(parse_task_gid(url).unwrap(), "1201234567890");
    }

    #[test]
    fn task_marker_url() {
        let url = "https://app.asana.com/1/55/project/77/task/1201234567890?focus=true";
        assert_eq!(parse_task_gid(url).unwrap(), "1201234567890");
    }

    #[test]
    fn no_id() {
        assert!(parse_task_gid("https://example.com/nope").is_err());
    }
}
