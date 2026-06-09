//! Ninjasana — a mouse-native terminal UI for Asana.
//!
//! Inspired by herdr (https://herdr.dev). Built on Ratatui + crossterm so it
//! behaves like a well-mannered terminal program that can be embedded in a
//! herdr PTY pane.

mod app;
mod asana;
mod cli;
mod commands;
mod config;
mod credentials;
mod event;
mod settings;
mod tui;
mod ui;

use anyhow::Result;
use clap::Parser;

use crate::app::{App, AppMode};
use crate::asana::Client;
use crate::cli::{Cli, Command, parse_task_gid};
use crate::config::Config;
use crate::settings::Settings;

#[tokio::main]
async fn main() -> Result<()> {
    // Load a local .env (e.g. ASANA_ACCESS_TOKEN) if present; ignore if absent.
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    // Non-TUI subcommands return before we touch the terminal.
    match cli.command {
        Some(Command::Login) => return commands::login().await,
        Some(Command::Logout) => return commands::logout(),
        None => {}
    }

    // Decide which view to open.
    let mode = match &cli.task_url {
        Some(url) => AppMode::TaskDetail(parse_task_gid(url)?),
        None => AppMode::Full,
    };

    // A token (env or keychain) enables live data; absence means demo mode.
    let client = Config::load().map(Client::new);
    let columns = Settings::load().columns;

    let mut terminal = tui::init()?;
    let result = App::new(mode, client, columns).run(&mut terminal).await;
    tui::restore()?;
    result
}
