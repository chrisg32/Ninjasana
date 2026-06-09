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

use crate::asana::{AsanaUpdate, Client, Named, Project, Section, Task, TaskDetail, TaskListKey};
use crate::event::{Event, EventBus};
use crate::settings::{Column, ProjectSource};
use crate::state::CollapseStore;
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
    /// A section header — toggles collapse.
    Section(usize),
    /// A task row — `(section index, task index)`.
    TaskRow(usize, usize),
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

    /// Resolve a drag's cursor position to a drop target.
    pub fn drop_target(&self, column: u16, row: u16) -> Option<DropTarget> {
        match self.hit(column, row)? {
            Zone::TaskRow(si, ti) => Some(DropTarget::Task(si, ti)),
            Zone::Section(si) => Some(DropTarget::SectionTop(si)),
            _ => None,
        }
    }
}

/// A section plus its UI-only collapsed state.
pub struct SectionView {
    pub gid: Option<String>,
    pub name: String,
    pub tasks: Vec<Task>,
    pub collapsed: bool,
}

/// Where a drag is hovering.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
    /// Over a task row `(section, task)`.
    Task(usize, usize),
    /// Over a section header — drop at the top of that section.
    SectionTop(usize),
}

/// In-progress drag of a task row.
#[derive(Clone, Copy)]
pub struct Drag {
    pub from: (usize, usize),
    pub over: Option<DropTarget>,
    pub moved: bool,
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
    pub projects: Vec<Project>,
    pub nav: Nav,

    // Middle pane.
    pub sections: Vec<SectionView>,
    pub scroll: usize,
    /// `(section index, task index)` of the selected task.
    pub selected: Option<(usize, usize)>,
    /// Number of task/header rows the middle pane can show; set each render.
    pub viewport_rows: usize,

    // Right pane.
    pub detail: Option<TaskDetail>,
    pub detail_loading: bool,

    /// Columns shown in the task table, from the user's config.
    pub columns: Vec<Column>,
    /// Which projects to list in the nav pane.
    project_source: ProjectSource,
    /// In-progress drag of a task row, if any.
    pub drag: Option<Drag>,
    /// Persisted section-collapse state.
    collapse: CollapseStore,

    pub status: String,
    pub zones: ZoneMap,
}

impl App {
    pub fn new(
        mode: AppMode,
        client: Option<Client>,
        columns: Vec<Column>,
        project_source: ProjectSource,
    ) -> Self {
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
            sections: Vec::new(),
            scroll: 0,
            selected: None,
            viewport_rows: 0,
            detail: None,
            detail_loading: false,
            columns,
            project_source,
            drag: None,
            collapse: CollapseStore::load(),
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
            let source = self.project_source.clone();
            tokio::spawn(async move {
                let update = match bootstrap(&client, source).await {
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
                    // Pressing a task row both selects it and arms a potential
                    // drag; whether it becomes a drag depends on later motion.
                    if let Zone::TaskRow(si, ti) = zone {
                        self.drag = Some(Drag {
                            from: (si, ti),
                            over: None,
                            moved: false,
                        });
                    }
                    self.activate(zone);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.drag.is_some() {
                    let target = self.zones.drop_target(mouse.column, mouse.row);
                    if let Some(drag) = self.drag.as_mut() {
                        drag.moved = true;
                        if let Some(target) = target {
                            drag.over = Some(target);
                        }
                    }
                    self.status = "Drag to reorder — release to drop.".into();
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(drag) = self.drag.take()
                    && drag.moved
                    && let Some(target) = drag.over
                {
                    self.apply_reorder(drag.from, target);
                }
            }
            MouseEventKind::ScrollDown => self.scroll = self.scroll.saturating_add(1),
            MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(1),
            _ => {}
        }
    }

    fn activate(&mut self, zone: Zone) {
        match zone {
            Zone::Nav(nav) => self.select_nav(nav),
            Zone::Section(index) => {
                if index < self.sections.len() {
                    let collapsed = !self.sections[index].collapsed;
                    self.sections[index].collapsed = collapsed;
                    let nav = self.nav_key();
                    let key = section_key(&self.sections[index]);
                    self.collapse.set(&nav, &key, collapsed);
                }
            }
            Zone::TaskRow(section, task) => self.select_task(section, task),
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
            AsanaUpdate::Tasks { key, sections } => {
                // Ignore responses for a list the user has since navigated away from.
                if self.current_key() == key {
                    let count: usize = sections.iter().map(|s| s.tasks.len()).sum();
                    self.set_sections(sections);
                    self.status = format!("{} — {count} task(s).", self.nav_title());
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
        self.sections.clear();
        self.selected = None;
        self.detail = None;
        self.scroll = 0;
        if self.client.is_some() {
            self.status = format!("Loading {}…", self.nav_title());
            self.load_tasks_for(nav);
        } else {
            self.set_sections(demo_sections_for(nav, &self.projects));
            let count: usize = self.sections.iter().map(|s| s.tasks.len()).sum();
            self.status = format!("{} — {count} task(s).", self.nav_title());
        }
    }

    fn select_task(&mut self, section: usize, task: usize) {
        let Some(task_obj) = self
            .sections
            .get(section)
            .and_then(|s| s.tasks.get(task))
            .cloned()
        else {
            return;
        };
        self.selected = Some((section, task));
        self.status = format!("Selected: {}", task_obj.name);
        self.ensure_visible();
        if self.client.is_some() {
            self.detail = None;
            self.detail_loading = true;
            self.load_detail(task_obj.gid);
        } else {
            self.detail = Some(demo_detail(&task_obj));
        }
    }

    fn select_task_delta(&mut self, delta: isize) {
        let visible = self.visible_tasks();
        if visible.is_empty() {
            return;
        }
        let current = self
            .selected
            .and_then(|sel| visible.iter().position(|p| *p == sel));
        let next = match current {
            Some(i) => (i as isize + delta).clamp(0, visible.len() as isize - 1) as usize,
            None => 0,
        };
        let (section, task) = visible[next];
        self.select_task(section, task);
    }

    // ---- drag-to-reorder ----------------------------------------------

    /// Move the task at `from` to the drop target, updating the local model
    /// immediately and then persisting. Dropping onto a row that sits *below*
    /// the dragged task inserts *after* it (so dragging down lands where you'd
    /// expect); dropping above inserts before; dropping on a header goes to the
    /// top of that section.
    fn apply_reorder(&mut self, from: (usize, usize), target: DropTarget) {
        let (fs, ft) = from;
        if fs >= self.sections.len() || ft >= self.sections[fs].tasks.len() {
            return;
        }

        let (ts, raw) = match target {
            DropTarget::SectionTop(s) => (s, 0),
            DropTarget::Task(s, h) => {
                let dragging_down = matches!(
                    (self.row_index_of((fs, ft)), self.row_index_of((s, h))),
                    (Some(src), Some(tgt)) if tgt > src
                );
                (s, if dragging_down { h + 1 } else { h })
            }
        };
        if ts >= self.sections.len() {
            return;
        }

        let task = self.sections[fs].tasks.remove(ft);
        // Removing an earlier index in the same section shifts the target left.
        let mut tb = raw;
        if fs == ts && tb > ft {
            tb -= 1;
        }
        tb = tb.min(self.sections[ts].tasks.len());

        // True no-op: same section, same resulting slot.
        if fs == ts && tb == ft {
            self.sections[fs].tasks.insert(ft, task);
            return;
        }

        self.sections[ts].tasks.insert(tb, task);
        self.selected = Some((ts, tb));
        self.ensure_visible();
        self.persist_reorder(fs, ts, tb);
    }

    fn persist_reorder(&mut self, from_section: usize, ts: usize, tb: usize) {
        let Some(client) = self.client.clone() else {
            self.status = "Reordered (demo — not saved).".into();
            return;
        };

        // Pull what we need, then drop the borrow before touching `status`.
        let (task_gid, section_gid, task_name, insert_before) = {
            let Some(section) = self.sections.get(ts) else {
                return;
            };
            let Some(task) = section.tasks.get(tb) else {
                return;
            };
            (
                task.gid.clone(),
                section.gid.clone(),
                task.name.clone(),
                section.tasks.get(tb + 1).map(|t| t.gid.clone()),
            )
        };

        let Some(section_gid) = section_gid else {
            self.status = "Reordered locally — this section has no id to save to.".into();
            return;
        };
        let tx = self.tx.clone();

        if matches!(self.nav, Nav::MyTasks) {
            // Asana's API can move a task between My Tasks sections but does not
            // expose ordering within one — persist moves, keep reorders local.
            if from_section == ts {
                self.status =
                    format!("Reordered {task_name} locally — Asana doesn't save My Tasks order.");
                return;
            }
            self.status = format!("Moving {task_name}…");
            tokio::spawn(async move {
                if let Err(err) = client.set_assignee_section(&task_gid, &section_gid).await {
                    let _ =
                        tx.send(Event::Asana(AsanaUpdate::Error(format!("move failed: {err:#}"))));
                }
            });
        } else {
            self.status = format!("Moving {task_name}…");
            tokio::spawn(async move {
                if let Err(err) = client
                    .move_task_in_section(&section_gid, &task_gid, insert_before.as_deref())
                    .await
                {
                    let _ = tx.send(Event::Asana(AsanaUpdate::Error(format!(
                        "reorder failed: {err:#}"
                    ))));
                }
            });
        }
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
                    (Some(w), Some(u)) => client.my_tasks_sections(&w, &u).await,
                    _ => Err(anyhow::anyhow!("workspace/user not ready")),
                },
                TaskListKey::Project(gid) => client.project_sections(gid).await,
            };
            let update = match result {
                Ok(sections) => AsanaUpdate::Tasks { key, sections },
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

    fn set_sections(&mut self, sections: Vec<Section>) {
        let nav = self.nav_key();
        self.sections = sections
            .into_iter()
            .map(|s| {
                let collapsed = self.collapse.is_collapsed(&nav, &section_key_parts(&s.gid, &s.name));
                SectionView {
                    gid: s.gid,
                    name: s.name,
                    tasks: s.tasks,
                    collapsed,
                }
            })
            .collect();
        self.scroll = 0;
        self.selected = None;
        self.detail = None;
    }

    /// Stable per-list key for the collapse store.
    fn nav_key(&self) -> String {
        match self.nav {
            Nav::MyTasks => "my_tasks".to_string(),
            Nav::Project(i) => self
                .projects
                .get(i)
                .map(|p| format!("project:{}", p.gid))
                .unwrap_or_else(|| "project:?".to_string()),
        }
    }

    /// Flattened `(section, task)` indices that are currently visible (i.e. not
    /// inside a collapsed section).
    fn visible_tasks(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (si, section) in self.sections.iter().enumerate() {
            if section.collapsed {
                continue;
            }
            for ti in 0..section.tasks.len() {
                out.push((si, ti));
            }
        }
        out
    }

    /// The virtual row index (counting section headers and visible tasks) of a
    /// given task, used to keep the selection on screen.
    fn row_index_of(&self, target: (usize, usize)) -> Option<usize> {
        let mut row = 0;
        for (si, section) in self.sections.iter().enumerate() {
            row += 1; // header
            if section.collapsed {
                continue;
            }
            for ti in 0..section.tasks.len() {
                if (si, ti) == target {
                    return Some(row);
                }
                row += 1;
            }
        }
        None
    }

    fn ensure_visible(&mut self) {
        let Some(sel) = self.selected else {
            return;
        };
        let Some(row) = self.row_index_of(sel) else {
            return;
        };
        if row < self.scroll {
            self.scroll = row;
        } else if self.viewport_rows > 0 && row >= self.scroll + self.viewport_rows {
            self.scroll = row + 1 - self.viewport_rows;
        }
    }

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
        self.set_sections(demo_sections_for(Nav::MyTasks, &self.projects));
        self.status =
            "Demo mode — no token set. Run `ninjasana login`. Click around; q to quit.".into();
    }
}

/// Stable key for a section within its list (prefers the gid; falls back to a
/// name-derived key for synthetic sections).
fn section_key(section: &SectionView) -> String {
    section_key_parts(&section.gid, &section.name)
}

fn section_key_parts(gid: &Option<String>, name: &str) -> String {
    match gid {
        Some(g) => g.clone(),
        None => format!("name:{name}"),
    }
}

// ---- demo data (offline mode) -----------------------------------------

fn demo_projects() -> Vec<Project> {
    ["Ninjasana", "Website Redesign", "Q3 Roadmap"]
        .into_iter()
        .enumerate()
        .map(|(i, name)| Project {
            gid: format!("demo-{i}"),
            name: name.to_string(),
            members: Vec::new(),
        })
        .collect()
}

fn demo_task(gid: &str, name: &str, completed: bool, due: Option<&str>) -> Task {
    use crate::asana::CustomField;
    Task {
        gid: gid.to_string(),
        name: name.to_string(),
        completed,
        due_on: due.map(str::to_string),
        assignee: Some(Named {
            name: "You (demo)".to_string(),
        }),
        assignee_section: None,
        memberships: Vec::new(),
        tags: vec![Named {
            name: "demo".to_string(),
        }],
        custom_fields: vec![CustomField {
            name: "Dev Status v2".to_string(),
            display_value: Some(if completed {
                "Done".to_string()
            } else {
                "2. Development".to_string()
            }),
        }],
    }
}

fn demo_sections_for(nav: Nav, projects: &[Project]) -> Vec<Section> {
    match nav {
        Nav::MyTasks => vec![
            Section {
                gid: None,
                name: "Now".to_string(),
                tasks: vec![
                    demo_task("d-now-0", "Wire up PAT login", true, None),
                    demo_task("d-now-1", "Render My Tasks with sections", false, Some("2026-06-12")),
                ],
            },
            Section {
                gid: None,
                name: "Later".to_string(),
                tasks: vec![
                    demo_task("d-later-0", "Drag a row to reorder it", false, None),
                    demo_task("d-later-1", "Color-coded Dev Status", false, Some("2026-07-01")),
                ],
            },
        ],
        Nav::Project(i) => {
            let prefix = projects.get(i).map(|p| p.name.as_str()).unwrap_or("Project");
            vec![
                Section {
                    gid: None,
                    name: "To do".to_string(),
                    tasks: vec![
                        demo_task(&format!("d-{prefix}-0"), "Three-pane layout", true, None),
                        demo_task(&format!("d-{prefix}-1"), "Collapsible sections", false, None),
                    ],
                },
                Section {
                    gid: None,
                    name: "Done".to_string(),
                    tasks: vec![demo_task(
                        &format!("d-{prefix}-2"),
                        "Pick the language",
                        true,
                        None,
                    )],
                },
            ]
        }
    }
}

fn demo_detail(task: &Task) -> TaskDetail {
    TaskDetail {
        gid: task.gid.clone(),
        name: task.name.clone(),
        completed: task.completed,
        notes: "This is demo detail. Connect a real account with `ninjasana login` to see \
                live task notes, assignee, and due date."
            .to_string(),
        assignee: task.assignee.clone(),
        due_on: task.due_on.clone(),
        permalink_url: None,
    }
}

async fn bootstrap(
    client: &Client,
    source: ProjectSource,
) -> Result<(crate::asana::User, crate::asana::Workspace, Vec<Project>)> {
    let user = client.me().await?;
    let workspace = client
        .workspaces()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("your account has no workspaces"))?;
    let projects = match source {
        ProjectSource::Favorites => client.favorite_projects(&workspace.gid).await?,
        ProjectSource::Member => client.member_projects(&workspace.gid, &user.gid).await?,
        ProjectSource::Explicit(names) => {
            let all = client.all_projects(&workspace.gid).await?;
            order_by_names(all, &names)
        }
    };
    Ok((user, workspace, projects))
}

/// Pick projects matching `names` (case-insensitive), in the order `names`
/// lists them. Names with no match are skipped.
fn order_by_names(all: Vec<Project>, names: &[String]) -> Vec<Project> {
    names
        .iter()
        .filter_map(|name| {
            let want = name.trim().to_lowercase();
            all.iter()
                .find(|p| p.name.trim().to_lowercase() == want)
                .cloned()
        })
        .collect()
}
