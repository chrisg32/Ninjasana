//! Application state, the event loop, and mouse hit-testing.
//!
//! Mouse-native UX hinges on [`ZoneMap`]: during every render the UI registers
//! the screen rectangle of each clickable thing. When a click arrives we look
//! up which zone (if any) contains the cursor. Because Ratatui is immediate
//! mode, the rectangles we hit-test against are the very same ones we just laid
//! out — one coordinate system, no second source of truth.

use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{
    Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::asana::{self, AsanaUpdate};
use crate::config::Config;
use crate::event::{Event, EventBus};
use crate::tui::Tui;
use crate::ui;

/// The top-level views selectable from the sidebar.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    MyTasks,
    Projects,
    Search,
}

impl View {
    pub const ALL: [View; 3] = [View::MyTasks, View::Projects, View::Search];

    pub fn title(self) -> &'static str {
        match self {
            View::MyTasks => "My Tasks",
            View::Projects => "Projects",
            View::Search => "Search",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            View::MyTasks => "✓",
            View::Projects => "▣",
            View::Search => "⌕",
        }
    }
}

/// A clickable region of the screen. The variants carry whatever the click
/// handler needs to act (which view, which task row, …).
#[derive(Clone, PartialEq)]
pub enum Zone {
    Sidebar(View),
    TaskRow(usize),
    Quit,
}

/// The set of clickable regions for the current frame. Rebuilt every render.
#[derive(Default)]
pub struct ZoneMap {
    items: Vec<(Zone, Rect)>,
}

impl ZoneMap {
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Register a clickable region. Later registrations sit "on top", so push
    /// background regions first and overlays last.
    pub fn push(&mut self, zone: Zone, rect: Rect) {
        self.items.push((zone, rect));
    }

    /// Topmost zone containing the cursor, if any.
    pub fn hit(&self, column: u16, row: u16) -> Option<Zone> {
        let pos = Position {
            x: column,
            y: row,
        };
        self.items
            .iter()
            .rev()
            .find(|(_, rect)| rect.contains(pos))
            .map(|(zone, _)| zone.clone())
    }
}

/// A single task row (demo data until a real Asana fetch fills it in).
pub struct Task {
    pub name: String,
    pub completed: bool,
}

pub struct App {
    pub running: bool,
    pub view: View,
    pub tasks: Vec<Task>,
    pub selected: Option<usize>,
    pub scroll: usize,
    pub status: String,
    pub user: Option<String>,
    pub zones: ZoneMap,
}

impl App {
    pub fn new() -> Self {
        Self {
            running: true,
            view: View::MyTasks,
            tasks: demo_tasks(),
            selected: None,
            scroll: 0,
            status: "Welcome to Ninjasana — click a view, click a task, or press q to quit.".into(),
            user: None,
            zones: ZoneMap::default(),
        }
    }

    pub async fn run(&mut self, terminal: &mut Tui) -> Result<()> {
        let mut bus = EventBus::new(Duration::from_millis(250));

        // If a token is configured, verify the connection in the background so
        // the UI stays responsive. Otherwise run in offline demo mode.
        match Config::from_env() {
            Some(config) => {
                let tx = bus.tx.clone();
                tokio::spawn(async move {
                    let update = match asana::Client::new(config).me().await {
                        Ok(user) => AsanaUpdate::Me(user.name),
                        Err(err) => AsanaUpdate::Error(format!("{err:#}")),
                    };
                    let _ = tx.send(Event::Asana(update));
                });
            }
            None => {
                self.status =
                    "No ASANA_ACCESS_TOKEN set — running in demo mode. Press q to quit.".into();
            }
        }

        while self.running {
            terminal.draw(|frame| ui::render(frame, self))?;
            match bus.next().await {
                Some(Event::Tick) => {}
                Some(Event::Crossterm(event)) => self.handle_crossterm(event),
                Some(Event::Asana(update)) => self.handle_asana(update),
                None => break,
            }
        }
        Ok(())
    }

    fn handle_crossterm(&mut self, event: CrosstermEvent) {
        match event {
            CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
            CrosstermEvent::Mouse(mouse) => self.handle_mouse(mouse),
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.running = false,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.running = false
            }
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_prev(),
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(zone) = self.zones.hit(mouse.column, mouse.row) {
                    self.activate(zone);
                }
            }
            MouseEventKind::ScrollDown => {
                self.scroll = self.scroll.saturating_add(1);
            }
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn activate(&mut self, zone: Zone) {
        match zone {
            Zone::Sidebar(view) => {
                self.view = view;
                self.status = format!("Switched to {}.", view.title());
            }
            Zone::TaskRow(index) => {
                if let Some(task) = self.tasks.get(index) {
                    self.selected = Some(index);
                    self.status = format!("Selected: {}", task.name);
                }
            }
            Zone::Quit => self.running = false,
        }
    }

    fn handle_asana(&mut self, update: AsanaUpdate) {
        match update {
            AsanaUpdate::Me(name) => {
                self.status = format!("Connected to Asana as {name}.");
                self.user = Some(name);
            }
            AsanaUpdate::Error(message) => {
                self.status = format!("Asana error: {message}");
            }
        }
    }

    fn select_next(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let next = match self.selected {
            Some(i) => (i + 1).min(self.tasks.len() - 1),
            None => 0,
        };
        self.selected = Some(next);
        self.status = format!("Selected: {}", self.tasks[next].name);
    }

    fn select_prev(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let prev = match self.selected {
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        self.selected = Some(prev);
        self.status = format!("Selected: {}", self.tasks[prev].name);
    }
}

fn demo_tasks() -> Vec<Task> {
    [
        ("Wire up Asana OAuth / PAT auth", false),
        ("Render real My Tasks from the API", false),
        ("Drag-to-reorder task rows", false),
        ("Right-click context menu on a task", false),
        ("Project sidebar with workspaces", false),
        ("Scaffold the mouse zone system", true),
        ("Pick the language (Rust + Ratatui)", true),
        ("Create the private GitHub repo", true),
    ]
    .into_iter()
    .map(|(name, completed)| Task {
        name: name.to_string(),
        completed,
    })
    .collect()
}
