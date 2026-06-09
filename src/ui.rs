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
    let show_detail = app.selected_task.is_some();
    let layout = if show_detail {
        Layout::horizontal([
            Constraint::Length(28),
            Constraint::Min(20),
            Constraint::Length(46),
        ])
    } else {
        Layout::horizontal([Constraint::Length(28), Constraint::Min(20)])
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

    // Build the nav entries: My Tasks pinned on top, then projects.
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
        let selected = nav == app.nav;
        let style = if selected {
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        frame.render_widget(Paragraph::new(format!(" {label}")).style(style), rect);
        app.zones.push(Zone::Nav(nav), rect);
    }
}

fn render_tasks(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .title(format!(" {} ", app.nav_title()))
        .border_type(BorderType::Rounded);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.tasks.is_empty() {
        frame.render_widget(
            Paragraph::new(" No tasks.").style(Style::new().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    let height = inner.height as usize;
    let start = app.task_scroll.min(app.tasks.len().saturating_sub(1));

    for (offset, (index, task)) in app
        .tasks
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .enumerate()
    {
        let rect = Rect {
            x: inner.x,
            y: inner.y + offset as u16,
            width: inner.width,
            height: 1,
        };
        let check = if task.completed { "[x]" } else { "[ ]" };
        let label = format!(" {check} {}", task.name);

        let style = if app.selected_task == Some(index) {
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

        frame.render_widget(Paragraph::new(label).style(style), rect);
        app.zones.push(Zone::TaskRow(index), rect);
    }
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
        AppMode::Full => " click: open · scroll: list · ↑/↓ or j/k: move · q: quit ",
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
