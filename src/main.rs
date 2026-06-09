//! Ninjasana — a mouse-native terminal UI for Asana.
//!
//! Inspired by herdr (https://herdr.dev). Built on Ratatui + crossterm so it
//! behaves like a well-mannered terminal program that can be embedded in a
//! herdr PTY pane.

mod app;
mod asana;
mod config;
mod event;
mod tui;
mod ui;

use anyhow::Result;

use crate::app::App;

#[tokio::main]
async fn main() -> Result<()> {
    // Load a local .env (e.g. ASANA_ACCESS_TOKEN) if present; ignore if absent.
    let _ = dotenvy::dotenv();

    let mut terminal = tui::init()?;
    let result = App::new().run(&mut terminal).await;
    tui::restore()?;
    result
}
