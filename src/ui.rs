//! Rendering. Every clickable element registers its rectangle with the app's
//! [`ZoneMap`](crate::app::ZoneMap) as it is drawn, so the click handler and the
//! renderer share one set of coordinates. Columns are driven by the user's
//! config ([`crate::settings::Column`]), not hardcoded.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};

use crate::app::{ActivityTab, App, AppMode, DropTarget, Nav, Zone};
use crate::asana::Task;
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

    // Header and status bars are optional (configurable, any mode).
    let mut constraints = Vec::new();
    if app.show_header {
        constraints.push(Constraint::Length(3));
    }
    constraints.push(Constraint::Min(0));
    if app.show_footer {
        constraints.push(Constraint::Length(1));
    }
    let chunks = Layout::vertical(constraints).split(frame.area());

    let mut idx = 0;
    if app.show_header {
        render_header(frame, app, chunks[idx]);
        idx += 1;
    }
    let body = chunks[idx];
    idx += 1;
    match app.mode {
        AppMode::TaskDetail(_) => render_detail(frame, app, body),
        AppMode::Full => render_full_body(frame, app, body),
    }
    if app.show_footer {
        render_status(frame, app, chunks[idx]);
    }

    // Modal overlays last, so their zones sit on top.
    if app.picklist.is_some() {
        render_picklist_popup(frame, app);
    }
    if app.confirm_complete_open {
        render_confirm_dialog(frame, app);
    }
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
    app.detail_upper_rect = None;
    app.detail_thread_rect = None;

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let Some(task) = app.detail.clone() else {
        let msg = if app.detail_loading {
            "Loading…"
        } else {
            "Select a task to see its details."
        };
        frame.render_widget(
            Paragraph::new(msg).style(Style::new().fg(Color::DarkGray)),
            inner,
        );
        return;
    };

    // buttons · title · [ description + fields ] · tabs · [ thread ]
    let [buttons, title_area, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(0),
    ])
    .areas(inner);

    render_detail_buttons(frame, app, &task, buttons);
    frame.render_widget(
        Paragraph::new(task.name.clone())
            .style(Style::new().add_modifier(Modifier::BOLD))
            .wrap(Wrap { trim: false }),
        title_area,
    );

    // upper (description + fields + subtasks) · tabs · thread · composer
    let [upper, tabbar, thread, composer] = Layout::vertical([
        Constraint::Percentage(45),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .areas(body);
    app.detail_upper_rect = Some(upper);
    app.detail_thread_rect = Some(thread);

    render_detail_upper(frame, app, &task, upper);
    render_activity_tabs(frame, app, tabbar);
    render_thread(frame, app, thread);
    render_composer(frame, app, composer);
}

fn render_detail_buttons(frame: &mut Frame, app: &mut App, task: &Task, area: Rect) {
    if area.width < 10 {
        return;
    }
    let (label, style) = if task.completed {
        (
            " ✓ Completed ",
            Style::new().fg(Color::Black).bg(Color::Green),
        )
    } else {
        (
            " ✓ Mark complete ",
            Style::new()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
    };
    let w = (label.chars().count() as u16).min(area.width);
    let complete_rect = Rect {
        width: w,
        ..area
    };
    frame.render_widget(Paragraph::new(label).style(style), complete_rect);
    app.zones.push(Zone::MarkComplete, complete_rect);

    let copy = " Copy Link ";
    let cw = copy.chars().count() as u16;
    if area.width > w + cw + 1 {
        let copy_rect = Rect {
            x: area.x + area.width - cw,
            width: cw,
            ..area
        };
        frame.render_widget(
            Paragraph::new(copy).style(Style::new().fg(Color::White).bg(Color::Blue)),
            copy_rect,
        );
        app.zones.push(Zone::CopyLink, copy_rect);
    }
}

/// A virtual row in the upper detail region. Some rows are interactive.
struct UpperRow<'a> {
    spans: Vec<Span<'a>>,
    zone: Option<Zone>,
}

fn render_detail_upper(frame: &mut Frame, app: &mut App, task: &Task, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let width = area.width as usize;
    let mut rows: Vec<UpperRow> = Vec::new();

    let dim = Style::new().fg(Color::DarkGray);
    let header = |s: &str| UpperRow {
        spans: vec![Span::styled(
            s.to_string(),
            Style::new().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
        )],
        zone: None,
    };

    if app.detail_cfg.show_description && !task.notes.trim().is_empty() {
        rows.push(header("Description"));
        for line in task.notes.lines() {
            for chunk in wrap_hard(line, width) {
                rows.push(UpperRow {
                    spans: vec![Span::raw(chunk)],
                    zone: None,
                });
            }
        }
        rows.push(UpperRow { spans: vec![], zone: None });
    }

    // Fields (clickable to edit).
    for (i, column) in app.detail_cfg.fields.iter().enumerate() {
        let value = column.value(task);
        let shown = if value.is_empty() {
            "—".to_string()
        } else {
            value.clone()
        };
        rows.push(UpperRow {
            spans: vec![
                Span::styled(format!("{}: ", column.title()), dim),
                Span::styled(shown, field_value_style(column, &value)),
            ],
            zone: Some(Zone::Field(i)),
        });
    }

    // Subtasks (clickable to open).
    if !app.subtasks.is_empty() {
        rows.push(UpperRow { spans: vec![], zone: None });
        rows.push(header("Subtasks"));
        for (i, sub) in app.subtasks.iter().enumerate() {
            let mark = if sub.completed { "✔" } else { "○" };
            rows.push(UpperRow {
                spans: vec![Span::raw(fit(&format!("{mark} {}", sub.name), width))],
                zone: Some(Zone::Subtask(i)),
            });
        }
    }

    let height = area.height as usize;
    let start = app.detail_scroll.min(rows.len().saturating_sub(1));
    for (offset, row) in rows.iter().enumerate().skip(start).take(height).enumerate() {
        let (_, row) = row;
        let rect = Rect {
            x: area.x,
            y: area.y + offset as u16,
            width: area.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(Line::from(row.spans.clone())), rect);
        if let Some(zone) = &row.zone {
            app.zones.push(zone.clone(), rect);
        }
    }
}

fn render_composer(frame: &mut Frame, app: &mut App, area: Rect) {
    let editing_comment = matches!(
        app.input.as_ref().map(|i| &i.target),
        Some(crate::app::InputTarget::Comment)
    );
    let (title, body): (&str, String) = match &app.input {
        Some(input) => {
            let label = match input.target {
                crate::app::InputTarget::Comment => " New comment ",
                crate::app::InputTarget::Field(_) => " Edit field ",
            };
            (label, format!("{}▏", input.buffer))
        }
        None => (" Comment ", "Click to add a comment…".to_string()),
    };
    let border = if app.input.is_some() {
        Style::new().fg(ACCENT)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    let block = Block::bordered()
        .title(title)
        .border_type(BorderType::Rounded)
        .border_style(border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let text_style = if app.input.is_some() {
        Style::new()
    } else {
        Style::new().fg(Color::DarkGray)
    };
    frame.render_widget(
        Paragraph::new(body).style(text_style).wrap(Wrap { trim: false }),
        inner,
    );
    // Only the comment composer is click-to-focus; field edits open from a row.
    if app.input.is_none() || editing_comment {
        app.zones.push(Zone::Composer, area);
    }
}

/// Hard-wrap `text` to `width` columns (by character — fine for mostly ASCII).
fn wrap_hard(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(width)
        .map(|c| c.iter().collect::<String>())
        .collect()
}

fn render_activity_tabs(frame: &mut Frame, app: &mut App, area: Rect) {
    let tabs = [
        (ActivityTab::Comments, " Comments "),
        (ActivityTab::AllActivity, " All activity "),
    ];
    let mut x = area.x;
    for (tab, label) in tabs {
        let w = label.chars().count() as u16;
        if x + w > area.x + area.width {
            break;
        }
        let rect = Rect {
            x,
            y: area.y,
            width: w,
            height: 1,
        };
        let style = if app.activity_tab == tab {
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        frame.render_widget(Paragraph::new(label).style(style), rect);
        app.zones.push(Zone::Tab(tab), rect);
        x += w + 1;
    }
}

fn render_thread(frame: &mut Frame, app: &App, area: Rect) {
    let comments_only = app.activity_tab == ActivityTab::Comments;
    let stories: Vec<_> = app
        .stories
        .iter()
        .filter(|s| !comments_only || s.is_comment())
        .collect();

    if stories.is_empty() {
        let msg = if app.stories_loading {
            "Loading…"
        } else if comments_only {
            "No comments yet."
        } else {
            "No activity yet."
        };
        frame.render_widget(
            Paragraph::new(msg).style(Style::new().fg(Color::DarkGray)),
            area,
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for story in stories {
        let author = story
            .created_by
            .as_ref()
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "Someone".to_string());
        lines.push(Line::from(vec![
            Span::styled(author, Style::new().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  ·  {}", format_time(&story.created_at)),
                Style::new().fg(Color::DarkGray),
            ),
        ]));
        for line in story.text.lines() {
            lines.push(Line::from(line.to_string()));
        }
        lines.push(Line::from(""));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.thread_scroll as u16, 0)),
        area,
    );
}

fn render_confirm_dialog(frame: &mut Frame, app: &mut App) {
    let screen = frame.area();
    let w = 44u16.min(screen.width.saturating_sub(2));
    let h = 7u16.min(screen.height.saturating_sub(2));
    let rect = Rect {
        x: screen.x + (screen.width.saturating_sub(w)) / 2,
        y: screen.y + (screen.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" Confirm ")
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let name = app
        .detail
        .as_ref()
        .map(|t| t.name.clone())
        .unwrap_or_default();
    let [text_area, _gap, button_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(format!("Mark complete?\n{name}")).wrap(Wrap { trim: false }),
        text_area,
    );

    // [ Yes ]   [ No ] centered.
    let yes = " Yes ";
    let no = " No ";
    let (yw, nw) = (yes.len() as u16, no.len() as u16);
    let total = yw + nw + 3;
    let start = button_area.x + (button_area.width.saturating_sub(total)) / 2;
    let yes_rect = Rect {
        x: start,
        width: yw,
        ..button_area
    };
    let no_rect = Rect {
        x: start + yw + 3,
        width: nw,
        ..button_area
    };
    frame.render_widget(
        Paragraph::new(yes).style(Style::new().fg(Color::Black).bg(Color::Green)),
        yes_rect,
    );
    frame.render_widget(
        Paragraph::new(no).style(Style::new().fg(Color::White).bg(Color::Red)),
        no_rect,
    );
    app.zones.push(Zone::ConfirmYes, yes_rect);
    app.zones.push(Zone::ConfirmNo, no_rect);
}

fn render_picklist_popup(frame: &mut Frame, app: &mut App) {
    // Resolve the field's options up front, then drop the borrow before drawing.
    let Some(field_index) = app.picklist else {
        return;
    };
    let resolved = {
        let Some(Column::Custom(name)) = app.detail_cfg.fields.get(field_index) else {
            return;
        };
        let Some(task) = app.detail.as_ref() else {
            return;
        };
        task.custom_fields.iter().find(|f| &f.name == name).map(|cf| {
            (
                name.clone(),
                cf.enum_options.iter().map(|o| o.name.clone()).collect::<Vec<_>>(),
                cf.display_value.clone().unwrap_or_default(),
            )
        })
    };
    let Some((title, options, current)) = resolved else {
        return;
    };

    let screen = frame.area();
    let w = 36u16.min(screen.width.saturating_sub(2));
    let h = (options.len() as u16 + 2).clamp(3, screen.height.saturating_sub(2));
    let rect = Rect {
        x: screen.x + (screen.width.saturating_sub(w)) / 2,
        y: screen.y + (screen.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(format!(" {title} "))
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    for (i, name) in options.iter().enumerate().take(inner.height as usize) {
        let row = Rect {
            x: inner.x,
            y: inner.y + i as u16,
            width: inner.width,
            height: 1,
        };
        let selected = *name == current;
        let style = if selected {
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        let marker = if selected { "● " } else { "  " };
        frame.render_widget(
            Paragraph::new(fit(&format!("{marker}{name}"), inner.width as usize)).style(style),
            row,
        );
        app.zones.push(Zone::EnumOption(field_index, i), row);
    }
}

/// Color a detail field value like the table: tags magenta, status-style custom
/// fields by keyword, others plain.
fn field_value_style(column: &Column, value: &str) -> Style {
    match column {
        Column::Tags => Style::new().fg(Color::Magenta),
        Column::Custom(_) if !value.is_empty() => Style::new().fg(status_color(value)),
        _ => Style::new(),
    }
}

/// Trim an ISO-8601 timestamp to `YYYY-MM-DD HH:MM`.
fn format_time(iso: &str) -> String {
    let trimmed: String = iso.chars().take(16).collect();
    trimmed.replace('T', " ")
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
