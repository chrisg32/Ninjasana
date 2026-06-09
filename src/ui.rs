//! Rendering. Every clickable element registers its rectangle with the app's
//! [`ZoneMap`](crate::app::ZoneMap) as it is drawn, so the click handler and the
//! renderer share one set of coordinates. Columns are driven by the user's
//! config ([`crate::settings::Column`]), not hardcoded.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};

use crate::app::{App, AppMode, DropTarget, Nav, Zone};
use crate::settings::Column;

const ACCENT: Color = Color::Cyan;
/// Column separator / resize handle glyph.
const DIVIDER: char = '│';

/// One row of the middle pane: a section header or a task within a section.
enum Row {
    Header(usize),
    Task(usize, usize),
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
        frame.render_widget(
            Paragraph::new(fit(&format!(" {label}"), inner.width as usize)).style(style),
            rect,
        );
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

    // Column widths: fixed columns keep their (possibly resized) width; Name
    // flexes to fill. The leading 2 columns are reserved for the mark ("○ ").
    let widths = distribute_widths(app, (inner.width as usize).saturating_sub(2));

    let mut header = String::from("  ");
    for (i, (column, width)) in app.columns.iter().zip(&widths).enumerate() {
        if i > 0 {
            header.push(DIVIDER);
        }
        header.push_str(&fit(&column.title(), *width));
    }
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
            Row::Task(si, ti) => render_task_row(frame, app, si, ti, &widths, rect),
        }
    }

    // Register a 1-column drag handle on each fixed column's right-edge divider.
    // Spanning the full pane height makes the thin handle easy to grab.
    let mut x = inner.x + 2;
    for (i, width) in widths.iter().enumerate() {
        x += *width as u16;
        if i + 1 < app.columns.len() {
            if !app.columns[i].is_name() {
                app.zones.push(
                    Zone::ResizeHandle(i),
                    Rect {
                        x,
                        y: inner.y,
                        width: 1,
                        height: inner.height,
                    },
                );
            }
            x += 1; // the divider column
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

fn render_task_row(
    frame: &mut Frame,
    app: &mut App,
    si: usize,
    ti: usize,
    widths: &[usize],
    rect: Rect,
) {
    let task = app.sections[si].tasks[ti].clone();
    let selected = app.selected == Some((si, ti));
    let drag = app.drag;
    let is_grabbed = drag.is_some_and(|d| d.moved && d.from == (si, ti));
    let is_drop = drag.is_some_and(|d| d.moved && d.over == Some(DropTarget::Task(si, ti)));

    let mark = if is_grabbed {
        "≡"
    } else if task.completed {
        "✔"
    } else {
        "○"
    };

    // Special rows (selected / completed / mid-drag) render with one uniform
    // style; ordinary rows get per-column color coding.
    if selected || task.completed || is_grabbed || is_drop {
        let style = if is_drop {
            Style::new()
                .fg(ACCENT)
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED)
        } else if is_grabbed {
            Style::new()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC)
        } else if selected {
            Style::new()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::CROSSED_OUT)
        };
        let mut line = format!("{mark} ");
        for (i, (column, width)) in app.columns.iter().zip(widths).enumerate() {
            if i > 0 {
                line.push(DIVIDER);
            }
            line.push_str(&fit(&column.value(&task), *width));
        }
        frame.render_widget(Paragraph::new(line).style(style), rect);
    } else {
        let mut spans: Vec<Span> = vec![Span::styled(
            format!("{mark} "),
            Style::new().fg(Color::DarkGray),
        )];
        for (i, (column, width)) in app.columns.iter().zip(widths).enumerate() {
            if i > 0 {
                spans.push(Span::styled(
                    DIVIDER.to_string(),
                    Style::new().fg(Color::DarkGray),
                ));
            }
            let value = column.value(&task);
            spans.push(Span::styled(fit(&value, *width), column_style(column, &value)));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), rect);
    }

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
        AppMode::Full => {
            " click: open · drag row: reorder · drag │: resize · click section: collapse · ↑/↓: move · q: quit "
        }
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

/// Per-column foreground styling. Tags get a tag color; custom fields (e.g.
/// "Dev Status v2") are colored like a status pill by keyword; metadata is
/// dimmed; Name keeps the default.
fn column_style(column: &Column, value: &str) -> Style {
    match column {
        Column::Name => Style::new(),
        Column::Tags => Style::new().fg(Color::Magenta),
        Column::Completed => Style::new().fg(Color::Green),
        Column::Custom(_) => Style::new()
            .fg(status_color(value))
            .add_modifier(Modifier::BOLD),
        _ => Style::new().fg(Color::Gray),
    }
}

/// Map a status-like value to a color, mirroring Asana's pill palette.
fn status_color(value: &str) -> Color {
    let v = value.to_lowercase();
    if v.is_empty() {
        Color::DarkGray
    } else if ["done", "complete", "merged", "closed", "shipped"]
        .iter()
        .any(|k| v.contains(k))
    {
        Color::Green
    } else if ["review", "qa", "verify", "test"].iter().any(|k| v.contains(k)) {
        Color::Magenta
    } else if ["progress", "develop", "doing", "active"]
        .iter()
        .any(|k| v.contains(k))
    {
        Color::Cyan
    } else if ["block", "hold", "waiting", "stuck"].iter().any(|k| v.contains(k)) {
        Color::Red
    } else if ["backlog", "todo", "to do", "new", "ready", "triage"]
        .iter()
        .any(|k| v.contains(k))
    {
        Color::Yellow
    } else {
        Color::Blue
    }
}

/// Distribute `total` columns: fixed columns keep their (possibly resized)
/// width, `Name` columns split whatever is left (min 12 each).
fn distribute_widths(app: &App, total: usize) -> Vec<usize> {
    let columns = &app.columns;
    let separators = columns.len().saturating_sub(1);
    let fixed: usize = columns
        .iter()
        .filter(|c| !c.is_name())
        .map(|c| app.column_width(c))
        .sum();
    let name_count = columns.iter().filter(|c| c.is_name()).count().max(1);
    let remaining = total.saturating_sub(fixed + separators);
    let name_width = (remaining / name_count).max(12);
    columns
        .iter()
        .map(|c| {
            if c.is_name() {
                name_width
            } else {
                app.column_width(c)
            }
        })
        .collect()
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
