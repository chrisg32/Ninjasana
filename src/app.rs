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
use tokio::sync::mpsc::UnboundedSender;

use crate::asana::{AsanaUpdate, Client, Named, Task, TaskDetail, TaskListKey};
use crate::event::{Event, EventBus};
use crate::tui::Tui;
use crate::ui;

/// Which front door the app was launched through.
pub enum AppMode {
    /// `ninjasana` — the full three-pane view.
    Full,
    /// `ninjasana <task_url>` — only the detail pane for one task.
    TaskDetail(String),
}

/// A selectable entry in the left navigation pane.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    MyTasks,
    Project(usize),
}

/// A clickable region of the screen. Variants carry whatever the click handler
/// needs to act on.
#[derive(Clone, PartialEq)]
pub enum Zone {
    Nav(Nav),
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

pub struct App {
    pub mode: AppMode,
    pub running: bool,

    client: Option<Client>,
    tx: UnboundedSender<Event>,

    // Identity / workspace, learned at bootstrap.
    workspace: Option<String>,
    user_gid: Option<String>,
    pub user_name: Option<String>,

    // Left pane.
    pub projects: Vec<crate::asana::Project>,
    pub nav: Nav,

    // Middle pane.
    pub tasks: Vec<Task>,
    pub task_scroll: usize,
    pub selected_task: Option<usize>,

    // Right pane.
    pub detail: Option<TaskDetail>,
    pub detail_loading: bool,

    pub status: String,
    pub zones: ZoneMap,
}

impl App {
    pub fn new(mode: AppMode, client: Option<Client>) -> Self {
        // A dummy sender; replaced with the real one in `run`.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let offline = client.is_none();
        let mut app = Self {
            mode,
            running: true,
            client,
            tx,
            workspace: None,
            user_gid: None,
            user_name: None,
            projects: Vec::new(),
            nav: Nav::MyTasks,
            tasks: Vec::new(),
            task_scroll: 0,
            selected_task: None,
            detail: None,
            detail_loading: false,
            status: String::new(),
            zones: ZoneMap::default(),
        };
        if offline {
            app.load_demo();
        }
        app
    }

    pub async fn run(&mut self, terminal: &mut Tui) -> Result<()> {
        let mut bus = EventBus::new(Duration::from_millis(250));
        self.tx = bus.tx.clone();

        match &self.mode {
            AppMode::Full => self.start_full(),
            AppMode::TaskDetail(gid) => {
                let gid = gid.clone();
                self.status = format!("Loading task {gid}…");
                if self.client.is_some() {
                    self.detail_loading = true;
                    self.load_detail(gid);
                } else {
                    self.status =
                        "No Asana credentials — run `ninjasana login` first. Press q to quit."
                            .into();
                }
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

    fn start_full(&mut self) {
        if let Some(client) = self.client.clone() {
            self.status = "Connecting to Asana…".into();
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let update = match bootstrap(&client).await {
                    Ok((user, workspace, projects)) => AsanaUpdate::Bootstrap {
                        user,
                        workspace,
                        projects,
                    },
                    Err(err) => AsanaUpdate::Error(format!("{err:#}")),
                };
                let _ = tx.send(Event::Asana(update));
            });
        }
    }

    // ---- event handling ------------------------------------------------

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
            KeyCode::Down | KeyCode::Char('j') => self.select_task_delta(1),
            KeyCode::Up | KeyCode::Char('k') => self.select_task_delta(-1),
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
            MouseEventKind::ScrollDown => self.task_scroll = self.task_scroll.saturating_add(1),
            MouseEventKind::ScrollUp => self.task_scroll = self.task_scroll.saturating_sub(1),
            _ => {}
        }
    }

    fn activate(&mut self, zone: Zone) {
        match zone {
            Zone::Nav(nav) => self.select_nav(nav),
            Zone::TaskRow(index) => self.select_task(index),
            Zone::Quit => self.running = false,
        }
    }

    fn handle_asana(&mut self, update: AsanaUpdate) {
        match update {
            AsanaUpdate::Bootstrap {
                user,
                workspace,
                projects,
            } => {
                self.status = format!("Connected as {} ({}).", user.name, workspace.name);
                self.user_name = Some(user.name);
                self.user_gid = Some(user.gid);
                self.workspace = Some(workspace.gid);
                self.projects = projects;
                self.load_tasks_for(self.nav);
            }
            AsanaUpdate::Tasks { key, tasks } => {
                // Ignore responses for a list the user has since navigated away from.
                if self.current_key() == key {
                    self.tasks = tasks;
                    self.task_scroll = 0;
                    self.selected_task = None;
                    self.detail = None;
                    self.status = format!("{} — {} task(s).", self.nav_title(), self.tasks.len());
                }
            }
            AsanaUpdate::Detail(detail) => {
                self.detail_loading = false;
                self.detail = Some(detail);
            }
            AsanaUpdate::Error(message) => {
                self.detail_loading = false;
                self.status = format!("Asana error: {message}");
            }
        }
    }

    // ---- selection -----------------------------------------------------

    fn select_nav(&mut self, nav: Nav) {
        self.nav = nav;
        self.selected_task = None;
        self.detail = None;
        self.tasks.clear();
        if self.client.is_some() {
            self.status = format!("Loading {}…", self.nav_title());
            self.load_tasks_for(nav);
        } else {
            self.tasks = demo_tasks_for(nav, &self.projects);
            self.status = format!("{} — {} task(s).", self.nav_title(), self.tasks.len());
        }
    }

    fn select_task(&mut self, index: usize) {
        let Some(task) = self.tasks.get(index).cloned() else {
            return;
        };
        self.selected_task = Some(index);
        self.status = format!("Selected: {}", task.name);
        if self.client.is_some() {
            self.detail = None;
            self.detail_loading = true;
            self.load_detail(task.gid);
        } else {
            self.detail = Some(demo_detail(&task));
        }
    }

    fn select_task_delta(&mut self, delta: isize) {
        if self.tasks.is_empty() {
            return;
        }
        let last = self.tasks.len() - 1;
        let next = match self.selected_task {
            Some(i) => (i as isize + delta).clamp(0, last as isize) as usize,
            None => 0,
        };
        self.select_task(next);
    }

    // ---- async loaders -------------------------------------------------

    fn load_tasks_for(&self, nav: Nav) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let tx = self.tx.clone();
        let key = match nav {
            Nav::MyTasks => TaskListKey::MyTasks,
            Nav::Project(i) => match self.projects.get(i) {
                Some(p) => TaskListKey::Project(p.gid.clone()),
                None => return,
            },
        };
        let workspace = self.workspace.clone();
        let user = self.user_gid.clone();
        tokio::spawn(async move {
            let result = match &key {
                TaskListKey::MyTasks => match (workspace, user) {
                    (Some(w), Some(u)) => client.my_tasks(&w, &u).await,
                    _ => Err(anyhow::anyhow!("workspace/user not ready")),
                },
                TaskListKey::Project(gid) => client.tasks_in_project(gid).await,
            };
            let update = match result {
                Ok(tasks) => AsanaUpdate::Tasks { key, tasks },
                Err(err) => AsanaUpdate::Error(format!("{err:#}")),
            };
            let _ = tx.send(Event::Asana(update));
        });
    }

    fn load_detail(&self, gid: String) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let update = match client.task(&gid).await {
                Ok(detail) => AsanaUpdate::Detail(detail),
                Err(err) => AsanaUpdate::Error(format!("{err:#}")),
            };
            let _ = tx.send(Event::Asana(update));
        });
    }

    // ---- helpers -------------------------------------------------------

    fn current_key(&self) -> TaskListKey {
        match self.nav {
            Nav::MyTasks => TaskListKey::MyTasks,
            Nav::Project(i) => TaskListKey::Project(
                self.projects
                    .get(i)
                    .map(|p| p.gid.clone())
                    .unwrap_or_default(),
            ),
        }
    }

    pub fn nav_title(&self) -> String {
        match self.nav {
            Nav::MyTasks => "My Tasks".to_string(),
            Nav::Project(i) => self
                .projects
                .get(i)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Project".to_string()),
        }
    }

    fn load_demo(&mut self) {
        self.projects = demo_projects();
        self.tasks = demo_tasks_for(Nav::MyTasks, &self.projects);
        self.status =
            "Demo mode — no token set. Run `ninjasana login`. Click around; q to quit.".into();
    }
}

// ---- demo data (offline mode) -----------------------------------------

fn demo_projects() -> Vec<crate::asana::Project> {
    ["Ninjasana", "Website Redesign", "Q3 Roadmap"]
        .into_iter()
        .enumerate()
        .map(|(i, name)| crate::asana::Project {
            gid: format!("demo-{i}"),
            name: name.to_string(),
        })
        .collect()
}

fn demo_tasks_for(nav: Nav, projects: &[crate::asana::Project]) -> Vec<Task> {
    let names: &[(&str, bool)] = match nav {
        Nav::MyTasks => &[
            ("Pick the language (Rust + Ratatui)", true),
            ("Create the private GitHub repo", true),
            ("Scaffold the mouse zone system", true),
            ("Wire up PAT login", false),
            ("Render real My Tasks from the API", false),
            ("Browser OAuth login", false),
        ],
        Nav::Project(0) => &[
            ("Three-pane layout", false),
            ("Drag-to-reorder task rows", false),
            ("Right-click context menus", false),
        ],
        _ => &[("Sample task A", false), ("Sample task B", true)],
    };
    let prefix = match nav {
        Nav::Project(i) => projects.get(i).map(|p| p.name.as_str()).unwrap_or("Project"),
        Nav::MyTasks => "me",
    };
    names
        .iter()
        .enumerate()
        .map(|(i, (name, completed))| Task {
            gid: format!("demo-{prefix}-{i}"),
            name: name.to_string(),
            completed: *completed,
        })
        .collect()
}

fn demo_detail(task: &Task) -> TaskDetail {
    TaskDetail {
        gid: task.gid.clone(),
        name: task.name.clone(),
        completed: task.completed,
        notes: "This is demo detail. Connect a real account with `ninjasana login` to see \
                live task notes, assignee, and due date."
            .to_string(),
        assignee: Some(Named {
            name: "You (demo)".to_string(),
        }),
        due_on: Some("2026-06-30".to_string()),
        permalink_url: None,
    }
}

async fn bootstrap(
    client: &Client,
) -> Result<(
    crate::asana::User,
    crate::asana::Workspace,
    Vec<crate::asana::Project>,
)> {
    let user = client.me().await?;
    let workspace = client
        .workspaces()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("your account has no workspaces"))?;
    let projects = client.projects(&workspace.gid).await?;
    Ok((user, workspace, projects))
}
