//! Rendering. Every clickable element registers its rectangle with the app's
//! [`ZoneMap`](crate::app::ZoneMap) as it is drawn, so the click handler and the
//! renderer share one set of coordinates.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};

use crate::app::{App, AppMode, Nav, Zone};

const ACCENT: Color = Color::Cyan;
const DEV_STATUS_FIELD: &str = "Dev Status v2";

/// One row of the middle pane: a section header or a task within a section.
enum Row {
    Header(usize),
    Task(usize, usize),
}

/// Fixed widths for the metadata columns; Name flexes to fill the rest.
struct Columns {
    name: usize,
    due: usize,
    dev: usize,
    tags: usize,
    projects: usize,
}

impl Columns {
    fn for_width(total: usize) -> Self {
        let (due, dev, tags, projects) = (10, 16, 18, 22);
        // 2 cols for the status mark + 5 single-space separators.
        let fixed = 2 + 5 + due + dev + tags + projects;
        let name = total.saturating_sub(fixed).max(12);
        Self {
            name,
            due,
            dev,
            tags,
            projects,
        }
    }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    app.zones.clear();

    let [header, body, status] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    render_header(frame, app, header);
    match app.mode {
        AppMode::TaskDetail(_) => render_detail(frame, app, body),
        AppMode::Full => render_full_body(frame, app, body),
    }
    render_status(frame, app, status);
}

fn render_full_body(frame: &mut Frame, app: &mut App, area: Rect) {
    let show_detail = app.selected.is_some();
    let layout = if show_detail {
        Layout::horizontal([
            Constraint::Length(28),
            Constraint::Min(40),
            Constraint::Length(46),
        ])
    } else {
        Layout::horizontal([Constraint::Length(28), Constraint::Min(40)])
    };
    let chunks = layout.split(area);

    render_nav(frame, app, chunks[0]);
    render_tasks(frame, app, chunks[1]);
    if show_detail {
        render_detail(frame, app, chunks[2]);
    }
}

fn render_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered().border_type(BorderType::Rounded);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let who = match &app.user_name {
        Some(name) => format!("  {name}"),
        None => "  demo mode".to_string(),
    };
    let title = Line::from(vec![
        Span::styled(
            " Ninjasana",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  ·  Asana in your terminal"),
        Span::styled(who, Style::new().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(title), inner);

    let label = " Quit ";
    let width = label.len() as u16;
    if inner.width > width + 1 {
        let button = Rect {
            x: inner.x + inner.width - width - 1,
            y: inner.y,
            width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(label).style(
                Style::new()
                    .fg(Color::White)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            button,
        );
        app.zones.push(Zone::Quit, button);
    }
}

fn render_nav(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .title(" Navigation ")
        .border_type(BorderType::Rounded);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut entries: Vec<(Nav, String)> = vec![(Nav::MyTasks, "★ My Tasks".to_string())];
    for (i, project) in app.projects.iter().enumerate() {
        entries.push((Nav::Project(i), format!("# {}", project.name)));
    }

    for (row, (nav, label)) in entries.into_iter().enumerate() {
        let y = inner.y + row as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let rect = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: 1,
        };
        let style = if nav == app.nav {
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        frame.render_widget(Paragraph::new(fit(&format!(" {label}"), inner.width as usize)).style(style), rect);
        app.zones.push(Zone::Nav(nav), rect);
    }
}

fn render_tasks(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .title(format!(" {} ", app.nav_title()))
        .border_type(BorderType::Rounded);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let cols = Columns::for_width(inner.width as usize);

    // Column header row.
    let header = format!(
        "  {} {} {} {} {}",
        fit("Name", cols.name),
        fit("Due Date", cols.due),
        fit(DEV_STATUS_FIELD, cols.dev),
        fit("Tags", cols.tags),
        fit("Projects", cols.projects),
    );
    frame.render_widget(
        Paragraph::new(header).style(Style::new().fg(Color::DarkGray).add_modifier(Modifier::BOLD)),
        Rect {
            height: 1,
            ..inner
        },
    );

    let list_top = inner.y + 1;
    let available = inner.height.saturating_sub(1) as usize;
    app.viewport_rows = available;

    if app.sections.is_empty() {
        frame.render_widget(
            Paragraph::new(" No tasks.").style(Style::new().fg(Color::DarkGray)),
            Rect {
                y: list_top,
                height: 1,
                ..inner
            },
        );
        return;
    }

    // Flatten sections + visible tasks into virtual rows.
    let mut rows: Vec<Row> = Vec::new();
    for (si, section) in app.sections.iter().enumerate() {
        rows.push(Row::Header(si));
        if !section.collapsed {
            for ti in 0..section.tasks.len() {
                rows.push(Row::Task(si, ti));
            }
        }
    }

    let start = app.scroll.min(rows.len().saturating_sub(1));
    for (i, row) in rows.iter().enumerate().skip(start).take(available) {
        let rect = Rect {
            x: inner.x,
            y: list_top + (i - start) as u16,
            width: inner.width,
            height: 1,
        };
        match *row {
            Row::Header(si) => render_section_header(frame, app, si, rect),
            Row::Task(si, ti) => render_task_row(frame, app, si, ti, &cols, rect),
        }
    }
}

fn render_section_header(frame: &mut Frame, app: &mut App, si: usize, rect: Rect) {
    let section = &app.sections[si];
    let chevron = if section.collapsed { "▸" } else { "▾" };
    let label = format!(" {chevron} {} ({})", section.name, section.tasks.len());
    frame.render_widget(
        Paragraph::new(fit(&label, rect.width as usize)).style(
            Style::new()
                .fg(ACCENT)
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::REVERSED),
        ),
        rect,
    );
    app.zones.push(Zone::Section(si), rect);
}

fn render_task_row(frame: &mut Frame, app: &mut App, si: usize, ti: usize, cols: &Columns, rect: Rect) {
    let task = &app.sections[si].tasks[ti];
    let selected = app.selected == Some((si, ti));

    let mark = if task.completed { "✔" } else { "○" };
    let due = task.due_on.clone().unwrap_or_else(|| "—".to_string());
    let dev = task.custom_field(DEV_STATUS_FIELD).unwrap_or_default();
    let tags = task.tag_names().join(", ");
    let projects = task.project_names().join(", ");

    let line = format!(
        "{mark} {} {} {} {} {}",
        fit(&task.name, cols.name),
        fit(&due, cols.due),
        fit(&dev, cols.dev),
        fit(&tags, cols.tags),
        fit(&projects, cols.projects),
    );

    let style = if selected {
        Style::new()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else if task.completed {
        Style::new()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::CROSSED_OUT)
    } else {
        Style::new()
    };

    frame.render_widget(Paragraph::new(line).style(style), rect);
    app.zones.push(Zone::TaskRow(si, ti), rect);
}

fn render_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .title(" Task ")
        .border_type(BorderType::Rounded);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines: Vec<Line> = match &app.detail {
        Some(detail) => {
            let status = if detail.completed {
                Span::styled("● completed", Style::new().fg(Color::Green))
            } else {
                Span::styled("○ incomplete", Style::new().fg(Color::Yellow))
            };
            let assignee = detail
                .assignee
                .as_ref()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| "unassigned".to_string());
            let due = detail.due_on.clone().unwrap_or_else(|| "—".to_string());

            let mut lines = vec![
                Line::from(Span::styled(
                    detail.name.clone(),
                    Style::new().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(status),
                Line::from(format!("Assignee: {assignee}")),
                Line::from(format!("Due: {due}")),
            ];
            if let Some(url) = &detail.permalink_url {
                lines.push(Line::from(Span::styled(
                    url.clone(),
                    Style::new().fg(Color::Blue),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Notes",
                Style::new().fg(Color::DarkGray),
            )));
            for line in detail.notes.lines() {
                lines.push(Line::from(line.to_string()));
            }
            lines
        }
        None if app.detail_loading => vec![Line::from("Loading…")],
        None => vec![Line::from(Span::styled(
            "Select a task to see its details.",
            Style::new().fg(Color::DarkGray),
        ))],
    };

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_status(frame: &mut Frame, app: &mut App, area: Rect) {
    let hints = match app.mode {
        AppMode::Full => " click row: open · click section: collapse · scroll · ↑/↓: move · q: quit ",
        AppMode::TaskDetail(_) => " q: quit ",
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.status),
            Style::new().fg(Color::Black).bg(ACCENT),
        ),
        Span::raw(" "),
        Span::styled(hints, Style::new().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// Truncate (with an ellipsis) or pad `s` to exactly `width` display columns,
/// counting by character (a good-enough proxy for mostly-ASCII task text).
fn fit(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        let mut out = s.to_string();
        out.extend(std::iter::repeat_n(' ', width - chars.len()));
        out
    } else if width == 1 {
        "…".to_string()
    } else {
        let mut out: String = chars[..width - 1].iter().collect();
        out.push('…');
        out
    }
}
