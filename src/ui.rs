//! Rendering. Every clickable element registers its rectangle with the app's
//! [`ZoneMap`](crate::app::ZoneMap) as it is drawn, so the click handler and the
//! renderer share one set of coordinates.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use crate::app::{App, View, Zone};

pub fn render(frame: &mut Frame, app: &mut App) {
    app.zones.clear();

    let [header, body, status] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let [sidebar, main] =
        Layout::horizontal([Constraint::Length(24), Constraint::Min(0)]).areas(body);

    render_header(frame, app, header);
    render_sidebar(frame, app, sidebar);
    render_main(frame, app, main);
    render_status(frame, app, status);
}

fn render_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered().border_type(BorderType::Rounded);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let who = match &app.user {
        Some(name) => format!("  {name}"),
        None => "  demo mode".to_string(),
    };
    let title = Line::from(vec![
        Span::styled(" Ninjasana", Style::new().fg(Color::Cyan).bold()),
        Span::raw("  ·  Asana in your terminal"),
        Span::styled(who, Style::new().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(title), inner);

    // A real, clickable Quit button pinned to the right.
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
            Paragraph::new(label).style(Style::new().fg(Color::White).bg(Color::Red).bold()),
            button,
        );
        app.zones.push(Zone::Quit, button);
    }
}

fn render_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .title(" Views ")
        .border_type(BorderType::Rounded);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    for (i, view) in View::ALL.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let row = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: 1,
        };
        let label = format!(" {} {}", view.icon(), view.title());
        let style = if *view == app.view {
            Style::new().fg(Color::Black).bg(Color::Cyan).bold()
        } else {
            Style::new()
        };
        frame.render_widget(Paragraph::new(label).style(style), row);
        app.zones.push(Zone::Sidebar(*view), row);
    }
}

fn render_main(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .title(format!(" {} ", app.view.title()))
        .border_type(BorderType::Rounded);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let height = inner.height as usize;
    let start = app.scroll.min(app.tasks.len().saturating_sub(1));

    for (offset, (index, task)) in app
        .tasks
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .enumerate()
    {
        let row = Rect {
            x: inner.x,
            y: inner.y + offset as u16,
            width: inner.width,
            height: 1,
        };
        let check = if task.completed { "[x]" } else { "[ ]" };
        let label = format!(" {check} {}", task.name);

        let style = if app.selected == Some(index) {
            Style::new().fg(Color::Black).bg(Color::Yellow).bold()
        } else if task.completed {
            Style::new()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::CROSSED_OUT)
        } else {
            Style::new()
        };

        frame.render_widget(Paragraph::new(label).style(style), row);
        app.zones.push(Zone::TaskRow(index), row);
    }
}

fn render_status(frame: &mut Frame, app: &mut App, area: Rect) {
    let hints = " click: select · scroll: list · ↑/↓ or j/k: move · q: quit ";
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.status),
            Style::new().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(" "),
        Span::styled(hints, Style::new().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
