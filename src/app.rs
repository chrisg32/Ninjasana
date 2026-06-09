//! Application state, the event loop, and mouse hit-testing.
//!
//! Mouse-native UX hinges on [`ZoneMap`]: during every render the UI registers
//! the screen rectangle of each clickable thing. When a click arrives we look
//! up which zone (if any) contains the cursor. Because Ratatui is immediate
//! mode, the rectangles we hit-test against are the very same ones we just laid
//! out — one coordinate system, no second source of truth.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{
    Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use tokio::sync::mpsc::UnboundedSender;

use crate::asana::{
    AsanaUpdate, Client, Named, Project, Section, Story, Task, TaskListKey, WatchTarget,
};
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
    /// Detail pane: a subtask row (index into `subtasks`).
    Subtask(usize),
    /// Detail pane: an editable field row (index into `detail_cfg.fields`).
    Field(usize),
    /// Detail pane: the comment composer.
    Composer,
    /// Picklist popup: an option index.
    EnumOption(usize),
    /// Date picker: a day-of-month cell.
    DateDay(u8),
    /// Date picker: previous/next month, today, clear.
    DatePrevMonth,
    DateNextMonth,
    DateToday,
    DateClear,
    /// People picker: a user index, or unassign.
    PeopleOption(usize),
    PeopleUnassign,
    /// Confirm dialog buttons.
    ConfirmYes,
    ConfirmNo,
    Quit,
}

/// In-progress column resize of the divider between `left` and `right`. The
/// divider tracks the cursor by trading width between the two columns; the
/// flexible Name column (if it's one of them) absorbs without an explicit width.
#[derive(Clone, Copy)]
pub struct Resize {
    left: usize,
    right: usize,
    start_x: u16,
    left_start: usize,
    right_start: usize,
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

/// What an in-progress text entry will do when submitted.
#[derive(Clone, PartialEq, Eq)]
pub enum InputTarget {
    /// Post a new comment on the current task.
    Comment,
    /// Set the text value of a custom field (carries the field gid).
    Field(String),
    /// Set the number value of a custom field (carries the field gid).
    NumberField(String),
}

/// An active text-entry buffer (comment composer or field edit).
pub struct Input {
    pub target: InputTarget,
    pub buffer: String,
}

/// Which value a non-text edit writes to.
#[derive(Clone)]
pub enum EditField {
    /// A custom field, by gid.
    Custom(String),
    /// The built-in assignee.
    Assignee,
    /// The built-in due date.
    DueOn,
}

/// An open enum/multi-enum picklist.
pub struct Picklist {
    pub title: String,
    pub field_gid: String,
    pub multi: bool,
    pub options: Vec<crate::asana::EnumOption>,
    /// Currently-selected option gids.
    pub selected: Vec<String>,
}

/// An open calendar date picker. `month` is 1–12.
pub struct DatePicker {
    pub target: EditField,
    pub year: i32,
    pub month: u8,
}

/// An open people picker.
pub struct PeoplePicker {
    pub target: EditField,
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

    // Live-update watchers (Asana Events API). The generation counters retire
    // stale watcher tasks when the watched resource changes.
    detail_watch_gen: Arc<AtomicU64>,
    list_watch_gen: Arc<AtomicU64>,
    /// The resource gid the list pane is watching (project gid, or the user's
    /// task-list gid for My Tasks).
    list_resource: Option<String>,
    /// The user's task-list gid (My Tasks watch resource), learned at bootstrap.
    my_tasks_resource: Option<String>,

    // Right pane.
    /// The gid of the task whose detail is currently shown / loading. Used to
    /// match async detail/stories/subtasks responses to the current selection.
    detail_gid: Option<String>,
    pub detail: Option<Task>,
    pub detail_loading: bool,
    pub stories: Vec<Story>,
    pub stories_loading: bool,
    pub subtasks: Vec<Task>,
    pub activity_tab: ActivityTab,
    /// In-progress text entry (comment composer or a text field edit).
    pub input: Option<Input>,
    /// Open enum/multi-enum picklist, if any.
    pub picklist: Option<Picklist>,
    /// Open calendar date picker, if any.
    pub datepicker: Option<DatePicker>,
    /// Open people picker, if any.
    pub people_picker: Option<PeoplePicker>,
    /// Search query within the people picker.
    pub people_query: String,
    /// Scroll offset within the open picklist / people popup.
    pub popup_scroll: usize,
    /// Cached workspace users (for assignee / people pickers).
    pub users: Vec<crate::asana::User>,
    /// Per-region scroll offsets in the detail pane.
    pub desc_scroll: usize,
    pub props_scroll: usize,
    pub subtasks_scroll: usize,
    pub thread_scroll: usize,
    /// Screen rects of each scrollable detail region (set each render), used to
    /// route the scroll wheel to the region under the cursor.
    pub desc_rect: Option<Rect>,
    pub props_rect: Option<Rect>,
    pub subtasks_rect: Option<Rect>,
    pub thread_rect: Option<Rect>,
    /// Whether the mark-complete confirmation dialog is open.
    pub confirm_complete_open: bool,
    /// Detail pane configuration.
    pub detail_cfg: DetailConfig,
    /// Whether to draw the top header / bottom status bars.
    pub show_header: bool,
    pub show_footer: bool,
    /// Whether the navigation sidebar is collapsed (toggle with Ctrl+B).
    pub nav_collapsed: bool,

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
        show_header: bool,
        show_footer: bool,
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
            detail_watch_gen: Arc::new(AtomicU64::new(0)),
            list_watch_gen: Arc::new(AtomicU64::new(0)),
            list_resource: None,
            my_tasks_resource: None,
            detail_gid: None,
            detail: None,
            detail_loading: false,
            stories: Vec::new(),
            stories_loading: false,
            subtasks: Vec::new(),
            activity_tab: ActivityTab::Comments,
            input: None,
            picklist: None,
            datepicker: None,
            people_picker: None,
            people_query: String::new(),
            popup_scroll: 0,
            users: Vec::new(),
            desc_scroll: 0,
            props_scroll: 0,
            subtasks_scroll: 0,
            thread_scroll: 0,
            desc_rect: None,
            props_rect: None,
            subtasks_rect: None,
            thread_rect: None,
            confirm_complete_open: false,
            detail_cfg,
            show_header,
            show_footer,
            nav_collapsed: false,
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
                    self.detail_gid = Some(gid.clone());
                    self.detail_loading = true;
                    self.stories_loading = true;
                    self.load_detail(gid.clone());
                    self.watch_detail(gid);
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
                    Ok((user, workspace, projects, my_tasks_resource)) => AsanaUpdate::Bootstrap {
                        user,
                        workspace,
                        projects,
                        my_tasks_resource,
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
        // Text entry (comment composer / field edit) captures input first.
        if self.input.is_some() {
            match key.code {
                KeyCode::Esc => self.input = None,
                KeyCode::Enter => self.submit_input(),
                KeyCode::Backspace => {
                    if let Some(input) = self.input.as_mut() {
                        input.buffer.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(input) = self.input.as_mut() {
                        input.buffer.push(c);
                    }
                }
                _ => {}
            }
            return;
        }
        // The confirmation dialog captures input while open.
        if self.confirm_complete_open {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => self.do_complete(),
                KeyCode::Char('n') | KeyCode::Esc => self.confirm_complete_open = false,
                _ => {}
            }
            return;
        }
        // The people picker is type-to-search.
        if self.people_picker.is_some() {
            match key.code {
                KeyCode::Esc => self.people_picker = None,
                KeyCode::Backspace => {
                    self.people_query.pop();
                    self.popup_scroll = 0;
                }
                KeyCode::Char(c) => {
                    self.people_query.push(c);
                    self.popup_scroll = 0;
                }
                _ => {}
            }
            return;
        }
        // Other field-edit popups close on Esc; selection is by click.
        if self.picklist.is_some() || self.datepicker.is_some() {
            if key.code == KeyCode::Esc {
                self.picklist = None;
                self.datepicker = None;
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.running = false,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.running = false
            }
            KeyCode::Char('b') => self.nav_collapsed = !self.nav_collapsed,
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
                // While a field-edit popup is open, route to it; clicking
                // elsewhere dismisses it.
                if self.picklist.is_some() {
                    match zone {
                        Zone::EnumOption(option_index) => self.pick_option(option_index),
                        _ => self.picklist = None,
                    }
                    return;
                }
                if self.datepicker.is_some() {
                    match zone {
                        Zone::DateDay(day) => self.pick_date_day(day),
                        Zone::DatePrevMonth => self.shift_month(-1),
                        Zone::DateNextMonth => self.shift_month(1),
                        Zone::DateToday => self.pick_date_today(),
                        Zone::DateClear => self.clear_date(),
                        _ => self.datepicker = None,
                    }
                    return;
                }
                if self.people_picker.is_some() {
                    match zone {
                        Zone::PeopleOption(i) => self.pick_person(Some(i)),
                        Zone::PeopleUnassign => self.pick_person(None),
                        _ => self.people_picker = None,
                    }
                    return;
                }
                match zone {
                    // A column divider arms a resize between it and its right
                    // neighbor (the handle only exists when a right neighbor does).
                    Zone::ResizeHandle(col) => {
                        self.resize = Some(Resize {
                            left: col,
                            right: col + 1,
                            start_x: mouse.column,
                            left_start: self.effective_width(col),
                            right_start: self.effective_width(col + 1),
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
                    self.apply_resize(resize, mouse.column);
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
            MouseEventKind::ScrollDown => self.scroll_delta(mouse.column, mouse.row, 1),
            MouseEventKind::ScrollUp => self.scroll_delta(mouse.column, mouse.row, -1),
            _ => {}
        }
    }

    fn scroll_delta(&mut self, column: u16, row: u16, delta: isize) {
        // A scrollable popup, when open, captures the wheel.
        if self.picklist.is_some() || self.people_picker.is_some() {
            self.popup_scroll = self.popup_scroll.saturating_add_signed(delta);
        } else {
            self.scroll_at(column, row, delta);
        }
    }

    /// Scroll whichever detail region the cursor is over, else the task list.
    fn scroll_at(&mut self, column: u16, row: u16, delta: isize) {
        let pos = Position { x: column, y: row };
        let target = if self.thread_rect.is_some_and(|r| r.contains(pos)) {
            &mut self.thread_scroll
        } else if self.desc_rect.is_some_and(|r| r.contains(pos)) {
            &mut self.desc_scroll
        } else if self.props_rect.is_some_and(|r| r.contains(pos)) {
            &mut self.props_scroll
        } else if self.subtasks_rect.is_some_and(|r| r.contains(pos)) {
            &mut self.subtasks_scroll
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
            Zone::Subtask(i) => {
                if let Some(task) = self.subtasks.get(i).cloned() {
                    self.open_task(&task);
                }
            }
            Zone::Field(i) => self.edit_field(i),
            Zone::Composer => {
                if self.input.is_none() {
                    self.input = Some(Input {
                        target: InputTarget::Comment,
                        buffer: String::new(),
                    });
                }
                self.status = "Type a comment — Enter to send, Esc to cancel.".into();
            }
            Zone::ConfirmYes => self.do_complete(),
            Zone::ConfirmNo => self.confirm_complete_open = false,
            Zone::Quit => self.running = false,
            // Handled on press / inside an open popup, never routed here.
            Zone::ResizeHandle(_)
            | Zone::EnumOption(_)
            | Zone::DateDay(_)
            | Zone::DatePrevMonth
            | Zone::DateNextMonth
            | Zone::DateToday
            | Zone::DateClear
            | Zone::PeopleOption(_)
            | Zone::PeopleUnassign => {}
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

    /// The custom field backing detail field `index`, if it's a custom field
    /// present on the current task.
    /// Begin editing detail field `index`, dispatching by type.
    fn edit_field(&mut self, index: usize) {
        self.popup_scroll = 0;
        let Some(column) = self.detail_cfg.fields.get(index).cloned() else {
            return;
        };
        match column {
            Column::Assignee => self.open_people_picker(EditField::Assignee),
            Column::DueDate => {
                let current = self.detail.as_ref().and_then(|t| t.due_on.clone());
                self.open_date_picker(EditField::DueOn, current);
            }
            Column::Custom(name) => self.edit_custom_field(&name),
            // Name / Projects / Tags / Completed are not edited here.
            _ => {}
        }
    }

    fn edit_custom_field(&mut self, name: &str) {
        enum Open {
            Picklist {
                gid: String,
                multi: bool,
                options: Vec<crate::asana::EnumOption>,
                selected: Vec<String>,
            },
            Text(String, String),
            Number(String, String),
            Date(String, Option<String>),
            People(String),
        }
        enum Resolved {
            Open(Open),
            NotEditable(String),
            NoOptions,
        }
        let resolved = {
            let Some(cf) = self.detail.as_ref().and_then(|t| {
                t.custom_fields
                    .iter()
                    .find(|f| crate::asana::field_name_matches(&f.name, name))
            }) else {
                self.status = format!("No \"{name}\" field on this task.");
                return;
            };
            let gid = cf.gid.clone();
            if cf.is_enum() || cf.is_multi_enum() {
                if cf.enum_options.is_empty() {
                    Resolved::NoOptions
                } else {
                    let multi = cf.is_multi_enum();
                    let selected = if multi {
                        cf.multi_enum_values.iter().map(|o| o.gid.clone()).collect()
                    } else {
                        cf.enum_value.iter().map(|o| o.gid.clone()).collect()
                    };
                    Resolved::Open(Open::Picklist {
                        gid,
                        multi,
                        options: cf.enum_options.clone(),
                        selected,
                    })
                }
            } else if cf.is_text() {
                Resolved::Open(Open::Text(gid, cf.display_value.clone().unwrap_or_default()))
            } else if cf.is_number() {
                Resolved::Open(Open::Number(gid, cf.display_value.clone().unwrap_or_default()))
            } else if cf.is_date() {
                Resolved::Open(Open::Date(
                    gid,
                    cf.date_value.as_ref().and_then(|d| d.date.clone()),
                ))
            } else if cf.is_people() {
                Resolved::Open(Open::People(gid))
            } else {
                Resolved::NotEditable(cf.resource_subtype.clone())
            }
        };
        let open = match resolved {
            Resolved::Open(open) => open,
            Resolved::NoOptions => {
                self.status = format!("\"{name}\" has no options to choose from.");
                return;
            }
            Resolved::NotEditable(subtype) => {
                let kind = if subtype.is_empty() { "this type" } else { &subtype };
                self.status = format!("\"{name}\" ({kind}) isn't editable yet.");
                return;
            }
        };
        match open {
            Open::Picklist {
                gid,
                multi,
                options,
                selected,
            } => {
                self.picklist = Some(Picklist {
                    title: name.to_string(),
                    field_gid: gid,
                    multi,
                    options,
                    selected,
                })
            }
            Open::Text(gid, cur) => {
                self.input = Some(Input {
                    target: InputTarget::Field(gid),
                    buffer: cur,
                })
            }
            Open::Number(gid, cur) => {
                self.input = Some(Input {
                    target: InputTarget::NumberField(gid),
                    buffer: cur,
                })
            }
            Open::Date(gid, cur) => self.open_date_picker(EditField::Custom(gid), cur),
            Open::People(gid) => self.open_people_picker(EditField::Custom(gid)),
        }
    }

    /// Toggle (multi) or set (single) an enum option from the open picklist.
    fn pick_option(&mut self, option_index: usize) {
        let action = {
            let Some(pl) = self.picklist.as_mut() else {
                return;
            };
            let Some(opt) = pl.options.get(option_index).cloned() else {
                return;
            };
            if pl.multi {
                if let Some(pos) = pl.selected.iter().position(|g| g == &opt.gid) {
                    pl.selected.remove(pos);
                } else {
                    pl.selected.push(opt.gid.clone());
                }
                let names: Vec<String> = pl
                    .options
                    .iter()
                    .filter(|o| pl.selected.contains(&o.gid))
                    .map(|o| o.name.clone())
                    .collect();
                (
                    pl.field_gid.clone(),
                    serde_json::json!(pl.selected.clone()),
                    names.join(", "),
                    false,
                )
            } else {
                (
                    pl.field_gid.clone(),
                    serde_json::json!(opt.gid),
                    opt.name.clone(),
                    true,
                )
            }
        };
        let (field_gid, value, display, close) = action;
        if close {
            self.picklist = None;
        }
        self.write_custom_field(field_gid, value, display);
    }

    // ---- date picker ---------------------------------------------------

    fn open_date_picker(&mut self, target: EditField, current: Option<String>) {
        let (year, month) = current
            .as_deref()
            .and_then(parse_ymd)
            .map(|(y, m, _)| (y, m))
            .unwrap_or_else(today_ym);
        self.datepicker = Some(DatePicker {
            target,
            year,
            month,
        });
    }

    fn shift_month(&mut self, delta: i32) {
        if let Some(dp) = self.datepicker.as_mut() {
            let mut m = dp.month as i32 + delta;
            let mut y = dp.year;
            while m < 1 {
                m += 12;
                y -= 1;
            }
            while m > 12 {
                m -= 12;
                y += 1;
            }
            dp.month = m as u8;
            dp.year = y;
        }
    }

    fn pick_date_day(&mut self, day: u8) {
        let Some(dp) = self.datepicker.take() else {
            return;
        };
        let date = format!("{:04}-{:02}-{:02}", dp.year, dp.month, day);
        self.apply_date(dp.target, Some(date));
    }

    fn pick_date_today(&mut self) {
        let Some(dp) = self.datepicker.take() else {
            return;
        };
        let (y, m, d) = today_ymd();
        self.apply_date(dp.target, Some(format!("{y:04}-{m:02}-{d:02}")));
    }

    fn clear_date(&mut self) {
        let Some(dp) = self.datepicker.take() else {
            return;
        };
        self.apply_date(dp.target, None);
    }

    fn apply_date(&mut self, target: EditField, date: Option<String>) {
        let display = date.clone().unwrap_or_else(|| "—".to_string());
        match target {
            EditField::Custom(field_gid) => {
                let value = match &date {
                    Some(d) => serde_json::json!({ "date": d }),
                    None => serde_json::Value::Null,
                };
                self.write_custom_field(field_gid, value, display);
            }
            EditField::DueOn => {
                if let Some(task) = self.detail.as_mut() {
                    task.due_on = date.clone();
                }
                let value = match &date {
                    Some(d) => serde_json::json!(d),
                    None => serde_json::Value::Null,
                };
                self.write_builtin("due_on", value);
            }
            EditField::Assignee => {}
        }
    }

    // ---- people picker -------------------------------------------------

    fn open_people_picker(&mut self, target: EditField) {
        if self.users.is_empty() {
            self.load_users();
        }
        self.people_query.clear();
        self.popup_scroll = 0;
        self.people_picker = Some(PeoplePicker { target });
    }

    fn load_users(&self) {
        let (Some(client), Some(workspace)) = (self.client.clone(), self.workspace.clone()) else {
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Ok(users) = client.users(&workspace).await {
                let _ = tx.send(Event::Asana(Box::new(AsanaUpdate::Users(users))));
            }
        });
    }

    fn pick_person(&mut self, index: Option<usize>) {
        let Some(pp) = self.people_picker.take() else {
            return;
        };
        let user = index.and_then(|i| self.users.get(i).cloned());
        let gid = user.as_ref().map(|u| u.gid.clone());
        let display = user
            .as_ref()
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "—".to_string());
        match pp.target {
            EditField::Assignee => {
                if let Some(task) = self.detail.as_mut() {
                    task.assignee = user.as_ref().map(|u| Named {
                        name: u.name.clone(),
                    });
                }
                let value = match &gid {
                    Some(g) => serde_json::json!(g),
                    None => serde_json::Value::Null,
                };
                self.write_builtin("assignee", value);
            }
            EditField::Custom(field_gid) => {
                // People custom fields take an array of user gids.
                let value = match &gid {
                    Some(g) => serde_json::json!([g]),
                    None => serde_json::json!([]),
                };
                self.write_custom_field(field_gid, value, display);
            }
            EditField::DueOn => {}
        }
    }

    // ---- writes --------------------------------------------------------

    fn write_custom_field(&mut self, field_gid: String, value: serde_json::Value, display: String) {
        self.update_local_custom_field(&field_gid, &display);
        self.status = "Updating field…".into();
        let Some(task_gid) = self.detail_gid.clone() else {
            return;
        };
        if let Some(client) = self.client.clone() {
            let tx = self.tx.clone();
            tokio::spawn(async move {
                if let Err(err) = client.set_custom_field(&task_gid, &field_gid, value).await {
                    let _ = tx.send(Event::Asana(Box::new(AsanaUpdate::Error(format!(
                        "update failed: {err:#}"
                    )))));
                }
            });
        }
    }

    fn write_builtin(&mut self, field: &'static str, value: serde_json::Value) {
        self.status = "Updating…".into();
        let Some(task_gid) = self.detail_gid.clone() else {
            return;
        };
        if let Some(client) = self.client.clone() {
            let tx = self.tx.clone();
            tokio::spawn(async move {
                if let Err(err) = client.set_field(&task_gid, field, value).await {
                    let _ = tx.send(Event::Asana(Box::new(AsanaUpdate::Error(format!(
                        "update failed: {err:#}"
                    )))));
                }
            });
        }
    }

    fn submit_input(&mut self) {
        let Some(input) = self.input.take() else {
            return;
        };
        let text = input.buffer.trim().to_string();
        match input.target {
            InputTarget::Comment => self.post_comment(text),
            InputTarget::Field(field_gid) => {
                let value = if text.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(text)
                };
                let display = if text.is_empty() { "—".to_string() } else { text };
                self.write_custom_field(field_gid, value, display);
            }
            InputTarget::NumberField(field_gid) => {
                let value = if text.is_empty() {
                    serde_json::Value::Null
                } else {
                    match text.parse::<f64>() {
                        Ok(n) => serde_json::json!(n),
                        Err(_) => {
                            self.status = "Not a valid number.".into();
                            return;
                        }
                    }
                };
                let display = if text.is_empty() { "—".to_string() } else { text };
                self.write_custom_field(field_gid, value, display);
            }
        }
    }

    fn post_comment(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let Some(task_gid) = self.detail_gid.clone() else {
            return;
        };
        if let Some(client) = self.client.clone() {
            let tx = self.tx.clone();
            self.status = "Posting comment…".into();
            tokio::spawn(async move {
                let update = match client.add_comment(&task_gid, &text).await {
                    // Re-fetch the thread so the new comment appears.
                    Ok(()) => match client.stories(&task_gid).await {
                        Ok(stories) => AsanaUpdate::Stories {
                            gid: task_gid,
                            stories,
                        },
                        Err(err) => AsanaUpdate::Error(format!("{err:#}")),
                    },
                    Err(err) => AsanaUpdate::Error(format!("comment failed: {err:#}")),
                };
                let _ = tx.send(Event::Asana(Box::new(update)));
            });
        } else {
            self.stories.push(Story {
                kind: "comment".to_string(),
                text,
                created_at: String::new(),
                created_by: Some(Named {
                    name: "You (demo)".to_string(),
                }),
            });
        }
    }

    fn update_local_custom_field(&mut self, field_gid: &str, display: &str) {
        if let Some(task) = self.detail.as_mut()
            && let Some(field) = task.custom_fields.iter_mut().find(|f| f.gid == field_gid)
        {
            field.display_value = Some(display.to_string());
        }
    }

    fn handle_asana(&mut self, update: AsanaUpdate) {
        match update {
            AsanaUpdate::Bootstrap {
                user,
                workspace,
                projects,
                my_tasks_resource,
            } => {
                self.status = format!("Connected as {} ({}).", user.name, workspace.name);
                self.user_name = Some(user.name);
                self.user_gid = Some(user.gid);
                self.workspace = Some(workspace.gid);
                self.projects = projects;
                self.my_tasks_resource = (!my_tasks_resource.is_empty()).then_some(my_tasks_resource);
                self.load_tasks_for(self.nav);
                self.watch_list_for_nav();
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
                // Match against the in-flight gid (detail may arrive after we
                // started loading a different task). Scroll offsets are reset in
                // `open_task`, not here, so a live refresh keeps your place.
                if self.detail_gid.as_deref() == Some(detail.gid.as_str()) {
                    self.detail_loading = false;
                    self.detail = Some(detail);
                }
            }
            AsanaUpdate::Stories { gid, stories } => {
                if self.detail_gid.as_deref() == Some(gid.as_str()) {
                    self.stories_loading = false;
                    self.stories = stories;
                }
            }
            AsanaUpdate::Subtasks { gid, subtasks } => {
                if self.detail_gid.as_deref() == Some(gid.as_str()) {
                    self.subtasks = subtasks;
                }
            }
            AsanaUpdate::Users(users) => self.users = users,
            AsanaUpdate::ResourceChanged { target, gid } => self.on_resource_changed(target, &gid),
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
            self.watch_list_for_nav();
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
        self.open_task(&task_obj);
    }

    /// Show a task in the detail pane and (re)load its detail, stories, and
    /// subtasks. Used by selecting a row and by clicking a subtask.
    fn open_task(&mut self, task: &Task) {
        self.detail_gid = Some(task.gid.clone());
        self.stories.clear();
        self.subtasks.clear();
        self.input = None;
        self.picklist = None;
        self.datepicker = None;
        self.people_picker = None;
        self.desc_scroll = 0;
        self.props_scroll = 0;
        self.subtasks_scroll = 0;
        self.thread_scroll = 0;
        if self.client.is_some() {
            self.detail = None;
            self.detail_loading = true;
            self.stories_loading = true;
            self.load_detail(task.gid.clone());
            self.watch_detail(task.gid.clone());
        } else {
            self.detail = Some(demo_detail(task));
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
        // Detail, stories, and subtasks load concurrently.
        let tx = self.tx.clone();
        let c = client.clone();
        let g = gid.clone();
        tokio::spawn(async move {
            let update = match c.task(&g).await {
                Ok(detail) => AsanaUpdate::Detail(detail),
                Err(err) => AsanaUpdate::Error(format!("{err:#}")),
            };
            let _ = tx.send(Event::Asana(Box::new(update)));
        });

        let tx = self.tx.clone();
        let c = client.clone();
        let g = gid.clone();
        tokio::spawn(async move {
            let update = match c.stories(&g).await {
                Ok(stories) => AsanaUpdate::Stories { gid: g, stories },
                Err(err) => AsanaUpdate::Error(format!("loading comments: {err:#}")),
            };
            let _ = tx.send(Event::Asana(Box::new(update)));
        });

        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Ok(subtasks) = client.subtasks(&gid).await {
                let _ = tx.send(Event::Asana(Box::new(AsanaUpdate::Subtasks { gid, subtasks })));
            }
        });
    }

    // ---- live updates (Events API) -------------------------------------

    /// Watch the open task for changes, retiring any previous detail watcher.
    fn watch_detail(&self, gid: String) {
        if let Some(client) = self.client.clone() {
            let generation = self.detail_watch_gen.fetch_add(1, Ordering::SeqCst) + 1;
            spawn_watch(
                client,
                gid,
                WatchTarget::Detail,
                self.detail_watch_gen.clone(),
                generation,
                self.tx.clone(),
            );
        }
    }

    /// Watch the current list's resource (project gid, or the My Tasks list).
    fn watch_list_for_nav(&mut self) {
        let resource = match self.nav {
            Nav::MyTasks => self.my_tasks_resource.clone(),
            Nav::Project(i) => self.projects.get(i).map(|p| p.gid.clone()),
        };
        self.list_resource = resource.clone();
        if let (Some(client), Some(resource)) = (self.client.clone(), resource) {
            let generation = self.list_watch_gen.fetch_add(1, Ordering::SeqCst) + 1;
            spawn_watch(
                client,
                resource,
                WatchTarget::List,
                self.list_watch_gen.clone(),
                generation,
                self.tx.clone(),
            );
        }
    }

    /// React to a watcher detecting a change, ignoring stale resources.
    fn on_resource_changed(&mut self, target: WatchTarget, gid: &str) {
        match target {
            WatchTarget::Detail => {
                if self.detail_gid.as_deref() == Some(gid) {
                    // Refresh in place; scroll offsets are preserved.
                    self.load_detail(gid.to_string());
                }
            }
            WatchTarget::List => {
                if self.list_resource.as_deref() == Some(gid) {
                    self.load_tasks_for(self.nav);
                }
            }
        }
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

    /// Apply a resize drag on the divider between `left` and `right`, keeping it
    /// under the cursor. Whichever side is the flexible Name column absorbs the
    /// change without an explicit width; otherwise the two trade width.
    fn apply_resize(&mut self, resize: Resize, cursor_x: u16) {
        let min = MIN_COL_WIDTH as i32;
        let delta = cursor_x as i32 - resize.start_x as i32;
        let left_flex = self.columns[resize.left].is_name();
        let right_flex = self.columns[resize.right].is_name();

        if left_flex {
            // Left side flexes: shrink the right column; flex grows to fill.
            let new_right = (resize.right_start as i32 - delta).max(min);
            let key = self.columns[resize.right].key();
            self.ui_state.set_column_width(&key, new_right as usize);
        } else if right_flex {
            // Right side flexes: grow the left column; flex shrinks to fit.
            let new_left = (resize.left_start as i32 + delta).max(min);
            let key = self.columns[resize.left].key();
            self.ui_state.set_column_width(&key, new_left as usize);
        } else {
            // Both fixed: trade width, keeping the right column at/above the min.
            let max_take = (resize.right_start as i32 - min).max(0);
            let change = (resize.left_start as i32 + delta).max(min) - resize.left_start as i32;
            let change = change.min(max_take);
            let left_key = self.columns[resize.left].key();
            let right_key = self.columns[resize.right].key();
            self.ui_state
                .set_column_width(&left_key, (resize.left_start as i32 + change) as usize);
            self.ui_state
                .set_column_width(&right_key, (resize.right_start as i32 - change) as usize);
        }
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
            gid: "demo-field-devstatus".to_string(),
            name: "Priority".to_string(),
            display_value: Some(if completed {
                "Done".to_string()
            } else {
                "2. Development".to_string()
            }),
            resource_subtype: "enum".to_string(),
            enum_options: ["1. Backlog", "2. Development", "3. Review", "Done"]
                .iter()
                .enumerate()
                .map(|(i, name)| crate::asana::EnumOption {
                    gid: format!("demo-opt-{i}"),
                    name: name.to_string(),
                })
                .collect(),
            enum_value: None,
            multi_enum_values: Vec::new(),
            date_value: None,
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
                    demo_task("d-later-1", "Color-coded Status", false, Some("2026-07-01")),
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

/// Spawn a background poller for `resource` via the Events API. It exits as soon
/// as `generation` no longer matches `mine` — i.e. a newer watcher replaced it,
/// or the channel closed. On a detected change it sends a `ResourceChanged`.
fn spawn_watch(
    client: Client,
    resource: String,
    target: WatchTarget,
    generation: Arc<AtomicU64>,
    mine: u64,
    tx: UnboundedSender<Event>,
) {
    tokio::spawn(async move {
        let mut sync: Option<String> = None;
        loop {
            if generation.load(Ordering::SeqCst) != mine {
                return;
            }
            // A transient error just falls through to the retry delay.
            if let Ok((new_sync, changed)) = client.events(&resource, sync.as_deref()).await {
                sync = Some(new_sync);
                if changed {
                    let update = AsanaUpdate::ResourceChanged {
                        target,
                        gid: resource.clone(),
                    };
                    if tx.send(Event::Asana(Box::new(update))).is_err() {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(4)).await;
        }
    });
}

async fn bootstrap(
    client: &Client,
    source: ProjectSource,
) -> Result<(
    crate::asana::User,
    crate::asana::Workspace,
    Vec<Project>,
    String,
)> {
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
    // The user's task-list gid is the watch resource for My Tasks (best-effort).
    let my_tasks_resource = client
        .user_task_list_gid(&workspace.gid, &user.gid)
        .await
        .unwrap_or_default();
    Ok((user, workspace, projects, my_tasks_resource))
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

/// Today as `(year, month, day)` in UTC.
fn today_ymd() -> (i32, u8, u8) {
    let date = time::OffsetDateTime::now_utc().date();
    (date.year(), u8::from(date.month()), date.day())
}

fn today_ym() -> (i32, u8) {
    let (y, m, _) = today_ymd();
    (y, m)
}

/// Parse a `YYYY-MM-DD` string into `(year, month, day)`.
fn parse_ymd(s: &str) -> Option<(i32, u8, u8)> {
    let mut parts = s.split('-');
    let y = parts.next()?.parse().ok()?;
    let m = parts.next()?.parse().ok()?;
    let d = parts.next()?.parse().ok()?;
    Some((y, m, d))
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
            project("Engineering"),
            project("Q3 Roadmap"),
            project("Customer Support"),
        ];
        let names = vec![
            "q3 roadmap".to_string(),
            "Engineering".to_string(),
            "Not A Project".to_string(),
        ];
        let result: Vec<String> = order_by_names(all, &names)
            .into_iter()
            .map(|p| p.name)
            .collect();
        // Configured order is preserved; unmatched names are dropped.
        assert_eq!(result, vec!["Q3 Roadmap", "Engineering"]);
    }
}
