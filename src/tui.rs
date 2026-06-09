//! Terminal lifecycle: raw mode, alternate screen, and — crucially for a
//! mouse-native tool — mouse capture. A panic hook restores the terminal so a
//! crash never leaves the user's shell in a broken state.

use std::io::{self, Stdout};

use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Enter raw mode, switch to the alternate screen, and enable mouse capture.
pub fn init() -> Result<Tui> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    set_panic_hook();
    let backend = CrosstermBackend::new(io::stdout());
    Ok(Terminal::new(backend)?)
}

/// Undo everything `init` did. Safe to call more than once.
pub fn restore() -> Result<()> {
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    disable_raw_mode()?;
    Ok(())
}

/// Make sure a panic doesn't leave the terminal in raw/alt-screen/mouse mode.
fn set_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore();
        original(info);
    }));
}
