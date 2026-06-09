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

use crate::asana::{AsanaUpdate, Client, Named, Project, Section, Story, Task, TaskListKey};
use crate::event::{Event, EventBus};
use crate::settings::{Column, DetailConfig, ProjectSource};
use crate::state::UiState;

/// Minimum width a resizable column can be dragged to.
const MIN_COL_WIDTH: usize = 3;
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

/// The two tabs in the detail pane's conversation area.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActivityTab {
    Comments,
    AllActivity,
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
    /// A column divider — drag to resize the column at this index.
    ResizeHandle(usize),
    /// Detail pane: the Mark-complete button.
    MarkComplete,
    /// Detail pane: the Copy-link button.
    CopyLink,
    /// Detail pane: a conversation tab.
    Tab(ActivityTab),
    /// Confirm dialog buttons.
    ConfirmYes,
    ConfirmNo,
    Quit,
}

/// In-progress column resize.
#[derive(Clone, Copy)]
pub struct Resize {
    col: usize,
    start_x: u16,
    start_width: usize,
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
    pub detail: Option<Task>,
    pub detail_loading: bool,
    pub stories: Vec<Story>,
    pub stories_loading: bool,
    pub activity_tab: ActivityTab,
    /// Scroll offset of the description+fields region.
    pub detail_scroll: usize,
    /// Scroll offset of the conversation region.
    pub thread_scroll: usize,
    /// Screen rects of the two scrollable detail regions (set each render).
    pub detail_upper_rect: Option<Rect>,
    pub detail_thread_rect: Option<Rect>,
    /// Whether the mark-complete confirmation dialog is open.
    pub confirm_complete_open: bool,
    /// Detail pane configuration.
    pub detail_cfg: DetailConfig,

    /// Columns shown in the task table, from the user's config.
    pub columns: Vec<Column>,
    /// Which projects to list in the nav pane.
    project_source: ProjectSource,
    /// In-progress drag of a task row, if any.
    pub drag: Option<Drag>,
    /// In-progress column resize, if any.
    resize: Option<Resize>,
    /// Persisted UI state (section collapse, column widths).
    ui_state: UiState,

    pub status: String,
    pub zones: ZoneMap,
}

impl App {
    pub fn new(
        mode: AppMode,
        client: Option<Client>,
        columns: Vec<Column>,
        project_source: ProjectSource,
        detail_cfg: DetailConfig,
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
            stories: Vec::new(),
            stories_loading: false,
            activity_tab: ActivityTab::Comments,
            detail_scroll: 0,
            thread_scroll: 0,
            detail_upper_rect: None,
            detail_thread_rect: None,
            confirm_complete_open: false,
            detail_cfg,
            columns,
            project_source,
            drag: None,
            resize: None,
            ui_state: UiState::load(),
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
                Some(Event::Asana(update)) => self.handle_asana(*update),
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
                let _ = tx.send(Event::Asana(Box::new(update)));
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
        // The confirmation dialog captures input while open.
        if self.confirm_complete_open {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => self.do_complete(),
                KeyCode::Char('n') | KeyCode::Esc => self.confirm_complete_open = false,
                _ => {}
            }
            return;
        }
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
                let Some(zone) = self.zones.hit(mouse.column, mouse.row) else {
                    return;
                };
                // While the confirm dialog is open, only its buttons respond.
                if self.confirm_complete_open {
                    match zone {
                        Zone::ConfirmYes => self.do_complete(),
                        Zone::ConfirmNo => self.confirm_complete_open = false,
                        _ => {}
                    }
                    return;
                }
                match zone {
                    // A column divider arms a resize.
                    Zone::ResizeHandle(col) => {
                        self.resize = Some(Resize {
                            col,
                            start_x: mouse.column,
                            start_width: self.effective_width(col),
                        });
                    }
                    // Pressing a task row selects it and arms a potential
                    // drag; whether it becomes a drag depends on motion.
                    Zone::TaskRow(si, ti) => {
                        self.drag = Some(Drag {
                            from: (si, ti),
                            over: None,
                            moved: false,
                        });
                        self.activate(zone);
                    }
                    other => self.activate(other),
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(resize) = self.resize {
                    let delta = mouse.column as i32 - resize.start_x as i32;
                    let width = (resize.start_width as i32 + delta).max(MIN_COL_WIDTH as i32) as usize;
                    let key = self.columns[resize.col].key();
                    self.ui_state.set_column_width(&key, width);
                    self.status = format!("{} width: {width}", self.columns[resize.col].title());
                } else if self.drag.is_some() {
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
                if self.resize.take().is_some() {
                    // Width was persisted live during the drag.
                } else if let Some(drag) = self.drag.take()
                    && drag.moved
                    && let Some(target) = drag.over
                {
                    self.apply_reorder(drag.from, target);
                }
            }
            MouseEventKind::ScrollDown => self.scroll_at(mouse.column, mouse.row, 1),
            MouseEventKind::ScrollUp => self.scroll_at(mouse.column, mouse.row, -1),
            _ => {}
        }
    }

    /// Scroll whichever region the cursor is over: the conversation thread, the
    /// description/fields region, or (default) the task list.
    fn scroll_at(&mut self, column: u16, row: u16, delta: isize) {
        let pos = Position { x: column, y: row };
        let target = if self.detail_thread_rect.is_some_and(|r| r.contains(pos)) {
            &mut self.thread_scroll
        } else if self.detail_upper_rect.is_some_and(|r| r.contains(pos)) {
            &mut self.detail_scroll
        } else {
            &mut self.scroll
        };
        *target = target.saturating_add_signed(delta);
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
                    self.ui_state.set_collapsed(&nav, &key, collapsed);
                }
            }
            Zone::TaskRow(section, task) => self.select_task(section, task),
            Zone::MarkComplete => self.mark_complete(),
            Zone::CopyLink => self.copy_link(),
            Zone::Tab(tab) => {
                self.activity_tab = tab;
                self.thread_scroll = 0;
            }
            Zone::ConfirmYes => self.do_complete(),
            Zone::ConfirmNo => self.confirm_complete_open = false,
            Zone::Quit => self.running = false,
            // Resize handles are handled on press, never routed here.
            Zone::ResizeHandle(_) => {}
        }
    }

    // ---- detail actions ------------------------------------------------

    fn mark_complete(&mut self) {
        // Nothing to do if there's no task or it's already complete.
        if self.detail.as_ref().is_none_or(|t| t.completed) {
            return;
        }
        if self.detail_cfg.confirm_complete {
            self.confirm_complete_open = true;
        } else {
            self.do_complete();
        }
    }

    fn do_complete(&mut self) {
        self.confirm_complete_open = false;
        let Some(task) = self.detail.as_mut() else {
            return;
        };
        task.completed = true; // optimistic
        let gid = task.gid.clone();
        let name = task.name.clone();
        self.status = format!("Marked complete: {name}");
        if let Some(client) = self.client.clone() {
            let tx = self.tx.clone();
            tokio::spawn(async move {
                if let Err(err) = client.set_completed(&gid, true).await {
                    let _ = tx.send(Event::Asana(Box::new(AsanaUpdate::Error(format!(
                        "mark complete failed: {err:#}"
                    )))));
                }
            });
        }
    }

    fn copy_link(&mut self) {
        let Some(url) = self.detail.as_ref().and_then(|t| t.permalink_url.clone()) else {
            self.status = "No link available for this task.".into();
            return;
        };
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(url)) {
            Ok(()) => self.status = "Task link copied to clipboard.".into(),
            Err(err) => self.status = format!("Couldn't copy link: {err}"),
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
                self.detail_scroll = 0;
                self.detail = Some(detail);
            }
            AsanaUpdate::Stories { gid, stories } => {
                // Ignore stories for a task we've navigated away from.
                if self.detail.as_ref().is_some_and(|t| t.gid == gid) {
                    self.stories_loading = false;
                    self.stories = stories;
                    self.thread_scroll = 0;
                }
            }
            AsanaUpdate::Error(message) => {
                self.detail_loading = false;
                self.stories_loading = false;
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
        self.stories.clear();
        self.detail_scroll = 0;
        self.thread_scroll = 0;
        if self.client.is_some() {
            self.detail = None;
            self.detail_loading = true;
            self.stories_loading = true;
            self.load_detail(task_obj.gid);
        } else {
            self.detail = Some(demo_detail(&task_obj));
            self.stories = demo_stories();
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
        let layout: Vec<(usize, bool)> = self
            .sections
            .iter()
            .map(|s| (s.tasks.len(), s.collapsed))
            .collect();
        let Some((ts, tb)) = resolve_drop(&layout, from, target) else {
            return;
        };

        let (fs, ft) = from;
        let task = self.sections[fs].tasks.remove(ft);
        let tb = tb.min(self.sections[ts].tasks.len());
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
                    let _ = tx.send(Event::Asana(Box::new(AsanaUpdate::Error(format!(
                        "move failed: {err:#}"
                    )))));
                }
            });
        } else {
            self.status = format!("Moving {task_name}…");
            tokio::spawn(async move {
                if let Err(err) = client
                    .move_task_in_section(&section_gid, &task_gid, insert_before.as_deref())
                    .await
                {
                    let _ = tx.send(Event::Asana(Box::new(AsanaUpdate::Error(format!(
                        "reorder failed: {err:#}"
                    )))));
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
            let _ = tx.send(Event::Asana(Box::new(update)));
        });
    }

    fn load_detail(&self, gid: String) {
        let Some(client) = self.client.clone() else {
            return;
        };
        // Fetch the task detail and its stories concurrently.
        let tx = self.tx.clone();
        let detail_client = client.clone();
        let detail_gid = gid.clone();
        tokio::spawn(async move {
            let update = match detail_client.task(&detail_gid).await {
                Ok(detail) => AsanaUpdate::Detail(detail),
                Err(err) => AsanaUpdate::Error(format!("{err:#}")),
            };
            let _ = tx.send(Event::Asana(Box::new(update)));
        });

        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Ok(stories) = client.stories(&gid).await {
                let _ = tx.send(Event::Asana(Box::new(AsanaUpdate::Stories { gid, stories })));
            }
        });
    }

    // ---- helpers -------------------------------------------------------

    fn set_sections(&mut self, sections: Vec<Section>) {
        let nav = self.nav_key();
        self.sections = sections
            .into_iter()
            .map(|s| {
                let collapsed = self.ui_state.is_collapsed(&nav, &section_key_parts(&s.gid, &s.name));
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

    /// Display width for a column: a persisted resize override, else its
    /// default. `Name` returns 0, the renderer's "flex to fill" sentinel.
    pub fn column_width(&self, column: &Column) -> usize {
        if column.is_name() {
            0
        } else {
            self.ui_state
                .column_width(&column.key())
                .unwrap_or_else(|| column.width())
        }
    }

    /// Effective width of the column at `index` (used when starting a resize).
    fn effective_width(&self, index: usize) -> usize {
        self.columns
            .get(index)
            .map(|c| {
                self.ui_state
                    .column_width(&c.key())
                    .unwrap_or_else(|| c.width())
            })
            .unwrap_or(MIN_COL_WIDTH)
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
        notes: format!(
            "Demo description for \"{name}\".\n\nConnect a real account with \
             `ninjasana login` to see live notes, fields, and the comment thread."
        ),
        permalink_url: None,
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

fn demo_detail(task: &Task) -> Task {
    task.clone()
}

fn demo_stories() -> Vec<Story> {
    use crate::asana::Story;
    vec![
        Story {
            kind: "system".to_string(),
            text: "created this task".to_string(),
            created_at: "2026-06-09T14:00:00.000Z".to_string(),
            created_by: Some(Named {
                name: "You (demo)".to_string(),
            }),
        },
        Story {
            kind: "comment".to_string(),
            text: "This is a demo comment. Log in to see the real thread.".to_string(),
            created_at: "2026-06-09T15:30:00.000Z".to_string(),
            created_by: Some(Named {
                name: "A Teammate".to_string(),
            }),
        },
    ]
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

/// Resolve a drag from `from` to a `target` into a final `(section, index)` to
/// insert at, or `None` for an invalid or no-op move.
///
/// `layout` is `(visible task count, collapsed)` per section. Dropping on a row
/// *below* the dragged task inserts after it (so dragging down lands where you
/// expect); above inserts before; a section header drops at the section top.
/// The returned index is already adjusted for removing the dragged task first.
fn resolve_drop(
    layout: &[(usize, bool)],
    from: (usize, usize),
    target: DropTarget,
) -> Option<(usize, usize)> {
    let (fs, ft) = from;
    if fs >= layout.len() || ft >= layout[fs].0 {
        return None;
    }

    // Virtual row index of a task, counting section headers and visible tasks.
    let row_index = |sec: usize, task: usize| -> Option<usize> {
        let mut row = 0;
        for (i, (len, collapsed)) in layout.iter().enumerate() {
            row += 1; // header
            if *collapsed {
                continue;
            }
            for ti in 0..*len {
                if (i, ti) == (sec, task) {
                    return Some(row);
                }
                row += 1;
            }
        }
        None
    };

    let (ts, raw) = match target {
        DropTarget::SectionTop(s) => (s, 0),
        DropTarget::Task(s, h) => {
            let dragging_down = matches!(
                (row_index(fs, ft), row_index(s, h)),
                (Some(src), Some(tgt)) if tgt > src
            );
            (s, if dragging_down { h + 1 } else { h })
        }
    };
    if ts >= layout.len() {
        return None;
    }

    // Account for removing the dragged task before inserting.
    let mut tb = raw;
    if fs == ts && tb > ft {
        tb -= 1;
    }
    let len_after = if fs == ts {
        layout[ts].0.saturating_sub(1)
    } else {
        layout[ts].0
    };
    tb = tb.min(len_after);

    // No-op: same section, same resulting slot.
    if fs == ts && tb == ft {
        return None;
    }
    Some((ts, tb))
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

#[cfg(test)]
mod tests {
    use super::{order_by_names, resolve_drop, DropTarget};
    use crate::asana::Project;

    /// Two sections: 3 tasks then 2 tasks, none collapsed.
    fn layout() -> Vec<(usize, bool)> {
        vec![(3, false), (2, false)]
    }

    #[test]
    fn drag_down_inserts_after_hovered_row() {
        // The reported bug: dragging task (0,1) down onto (0,2) should land it
        // *after* (0,2), i.e. index 2 — not be a no-op or land one too high.
        assert_eq!(
            resolve_drop(&layout(), (0, 1), DropTarget::Task(0, 2)),
            Some((0, 2))
        );
    }

    #[test]
    fn drag_up_inserts_before_hovered_row() {
        assert_eq!(
            resolve_drop(&layout(), (0, 2), DropTarget::Task(0, 0)),
            Some((0, 0))
        );
    }

    #[test]
    fn dropping_on_self_is_a_noop() {
        assert_eq!(resolve_drop(&layout(), (0, 1), DropTarget::Task(0, 1)), None);
        // Dropping on the slot just below itself is also a no-op.
        assert_eq!(resolve_drop(&layout(), (0, 0), DropTarget::Task(0, 0)), None);
    }

    #[test]
    fn drag_across_sections() {
        // (0,0) dropped onto (1,0): it's below in global order, so after it.
        assert_eq!(
            resolve_drop(&layout(), (0, 0), DropTarget::Task(1, 0)),
            Some((1, 1))
        );
    }

    #[test]
    fn drop_on_section_header_goes_to_top() {
        assert_eq!(
            resolve_drop(&layout(), (0, 0), DropTarget::SectionTop(1)),
            Some((1, 0))
        );
        // ...but onto its own section's top, from the top, is a no-op.
        assert_eq!(resolve_drop(&layout(), (0, 0), DropTarget::SectionTop(0)), None);
    }

    #[test]
    fn out_of_range_source_is_rejected() {
        assert_eq!(resolve_drop(&layout(), (0, 9), DropTarget::Task(1, 0)), None);
        assert_eq!(resolve_drop(&layout(), (5, 0), DropTarget::Task(1, 0)), None);
    }

    fn project(name: &str) -> Project {
        Project {
            gid: format!("gid-{name}"),
            name: name.to_string(),
            members: Vec::new(),
        }
    }

    #[test]
    fn order_by_names_matches_case_insensitively_in_order() {
        let all = vec![
            project("ISMS"),
            project("Sprint - Maximilian"),
            project("Customer Support"),
        ];
        let names = vec![
            "sprint - maximilian".to_string(),
            "ISMS".to_string(),
            "Not A Project".to_string(),
        ];
        let result: Vec<String> = order_by_names(all, &names)
            .into_iter()
            .map(|p| p.name)
            .collect();
        // Configured order is preserved; unmatched names are dropped.
        assert_eq!(result, vec!["Sprint - Maximilian", "ISMS"]);
    }
}
