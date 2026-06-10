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
    if app.datepicker.is_some() {
        render_datepicker_popup(frame, app);
    }
    if app.people_picker.is_some() {
        render_people_popup(frame, app);
    }
    if app.confirm_complete_open {
        render_confirm_dialog(frame, app);
    }
    if let Some(buffer) = app.new_task_buffer() {
        render_new_task_dialog(frame, buffer);
    }
}

fn render_full_body(frame: &mut Frame, app: &mut App, area: Rect) {
    let show_detail = app.detail_open();
    let show_nav = !app.nav_collapsed;

    let mut constraints = Vec::new();
    if show_nav {
        constraints.push(Constraint::Length(28));
    }
    constraints.push(Constraint::Min(40));
    if show_detail {
        constraints.push(Constraint::Length(46));
    }
    let chunks = Layout::horizontal(constraints).split(area);

    let mut idx = 0;
    if show_nav {
        render_nav(frame, app, chunks[idx]);
        idx += 1;
    }
    render_tasks(frame, app, chunks[idx]);
    idx += 1;
    if show_detail {
        render_detail(frame, app, chunks[idx]);
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
    for (i, (column, width)) in app.active_columns().iter().zip(&widths).enumerate() {
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

    // Register a 1-column drag handle on every divider (the right edge of each
    // non-last column). Spanning the full pane height makes it easy to grab.
    let mut x = inner.x + 2;
    for (i, width) in widths.iter().enumerate() {
        x += *width as u16;
        if i + 1 < app.active_columns().len() {
            app.zones.push(
                Zone::ResizeHandle(i),
                Rect {
                    x,
                    y: inner.y,
                    width: 1,
                    height: inner.height,
                },
            );
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
        for (i, (column, width)) in app.active_columns().iter().zip(widths).enumerate() {
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
        for (i, (column, width)) in app.active_columns().iter().zip(widths).enumerate() {
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

    // Hyperlink cells open in the browser; registered after the row so a click
    // on a link wins over selecting the task.
    let mut links: Vec<(Rect, String)> = Vec::new();
    let mut x = rect.x + 2;
    for (column, width) in app.active_columns().iter().zip(widths) {
        let value = column.value(&task);
        if is_url(&value) {
            let w = (*width as u16).min(rect.right().saturating_sub(x));
            if w > 0 {
                links.push((
                    Rect {
                        x,
                        y: rect.y,
                        width: w,
                        height: 1,
                    },
                    value,
                ));
            }
        }
        x += *width as u16 + 1; // column + divider
    }
    for (cell, url) in links {
        app.zones.push(Zone::OpenUrl(url), cell);
    }
}

fn render_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .title(" Task ")
        .border_type(BorderType::Rounded);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // Stale rects shouldn't capture the wheel when a region isn't shown.
    app.desc_rect = None;
    app.props_rect = None;
    app.subtasks_rect = None;
    app.thread_rect = None;

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

    // Title height tracks how many rows the (wrapped) name needs, capped.
    let title_h = (wrapped_lines(&task.name, inner.width as usize) as u16).clamp(1, 3);
    let [buttons, title_area, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(title_h),
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

    // Each region is its own bordered, independently-scrollable box, but heights
    // are content-aware so space isn't wasted: Properties/Subtasks size to their
    // (capped) content, and the leftover is split between Description and the
    // Conversation by how much each actually needs.
    enum Seg {
        Description,
        Properties,
        Subtasks,
        Tabs,
        Thread,
        Composer,
    }
    let body_h = body.height;
    let content_w = inner.width.saturating_sub(2) as usize; // inside a region's border
    let show_desc = app.detail_cfg.show_description && !task.notes.trim().is_empty();
    let show_props = !app.detail_cfg.fields.is_empty();
    let show_subs = !app.subtasks.is_empty();

    // Content-sized (border + rows), each capped so it can't hog the pane.
    let region = |rows: usize, cap: u16| -> u16 { ((rows as u16) + 2).clamp(3, cap.max(3)) };
    let props_h = if show_props {
        region(app.detail_cfg.fields.len(), body_h / 3)
    } else {
        0
    };
    let subs_h = if show_subs {
        region(app.subtasks.len(), body_h / 4)
    } else {
        0
    };

    // Description and Conversation share what's left (after tabs + composer).
    let avail = body_h.saturating_sub(1 + 3 + props_h + subs_h);
    let desc_lines = wrapped_lines(&task.notes, content_w);
    let thread_lines = thread_line_count(app, content_w);
    let desc_want = if show_desc {
        ((desc_lines as u16) + 2).max(3)
    } else {
        0
    };
    let thread_want = ((thread_lines as u16) + 2).max(3);
    let (desc_h, thread_h) = if !show_desc {
        (0, avail)
    } else if desc_want + thread_want <= avail {
        // Both fit without scrolling; give the surplus to the bigger one.
        let surplus = avail - desc_want - thread_want;
        if desc_lines >= thread_lines {
            (desc_want + surplus, thread_want)
        } else {
            (desc_want, thread_want + surplus)
        }
    } else {
        // Can't both fit; split proportionally, each at least 3 rows.
        let total = (desc_want + thread_want).max(1);
        let d = ((avail as u32 * desc_want as u32) / total as u32) as u16;
        let d = d.clamp(3.min(avail), avail.saturating_sub(3.min(avail)));
        (d, avail - d)
    };

    let mut segs: Vec<(Seg, u16)> = Vec::new();
    if show_desc {
        segs.push((Seg::Description, desc_h));
    }
    if show_props {
        segs.push((Seg::Properties, props_h));
    }
    if show_subs {
        segs.push((Seg::Subtasks, subs_h));
    }
    segs.push((Seg::Tabs, 1));
    segs.push((Seg::Thread, thread_h));
    segs.push((Seg::Composer, 3));

    let constraints: Vec<Constraint> = segs.iter().map(|(_, h)| Constraint::Length(*h)).collect();
    let chunks = Layout::vertical(constraints).split(body);
    for ((seg, _), &rect) in segs.iter().zip(chunks.iter()) {
        match seg {
            Seg::Description => render_description_block(frame, app, &task, rect),
            Seg::Properties => render_properties_block(frame, app, &task, rect),
            Seg::Subtasks => render_subtasks_block(frame, app, rect),
            Seg::Tabs => render_activity_tabs(frame, app, rect),
            Seg::Thread => render_thread_block(frame, app, rect),
            Seg::Composer => render_composer(frame, app, rect),
        }
    }
}

/// Rows that `text` occupies when hard-wrapped to `width` (counting newlines).
fn wrapped_lines(text: &str, width: usize) -> usize {
    if width == 0 {
        return text.lines().count().max(1);
    }
    text.lines()
        .map(|line| {
            let n = line.chars().count();
            if n == 0 { 1 } else { n.div_ceil(width) }
        })
        .sum::<usize>()
        .max(1)
}

/// Rows the conversation thread occupies for the active tab (for sizing).
fn thread_line_count(app: &App, width: usize) -> usize {
    let comments_only = app.activity_tab == ActivityTab::Comments;
    let stories: Vec<_> = app
        .stories
        .iter()
        .filter(|s| !comments_only || s.is_comment())
        .collect();
    if stories.is_empty() {
        return 1;
    }
    stories
        .iter()
        .map(|s| 1 + wrapped_lines(&s.text, width) + 1)
        .sum()
}

/// A bordered, titled region used in the detail pane.
fn region_block(title: &str) -> Block<'_> {
    Block::bordered()
        .title(format!(" {title} "))
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray))
}

fn render_description_block(frame: &mut Frame, app: &mut App, task: &Task, area: Rect) {
    let block = region_block("Description");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.desc_rect = Some(inner);
    frame.render_widget(
        Paragraph::new(task.notes.clone())
            .wrap(Wrap { trim: false })
            .scroll((app.desc_scroll as u16, 0)),
        inner,
    );
}

fn render_properties_block(frame: &mut Frame, app: &mut App, task: &Task, area: Rect) {
    let block = region_block("Properties");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.props_rect = Some(inner);
    if inner.height == 0 {
        return;
    }

    let dim = Style::new().fg(Color::DarkGray);
    // (rendered line, label width, Some(url) if the value is a hyperlink).
    let rows: Vec<(Line, u16, Option<String>)> = app
        .detail_cfg
        .fields
        .iter()
        .map(|column| {
            let value = column.value(task);
            let shown = if value.is_empty() {
                "—".to_string()
            } else {
                value.clone()
            };
            let label = format!("{}: ", column.title());
            let label_width = label.chars().count() as u16;
            let line = Line::from(vec![
                Span::styled(label, dim),
                Span::styled(shown, field_value_style(column, &value)),
            ]);
            (line, label_width, is_url(&value).then_some(value))
        })
        .collect();

    let start = app.props_scroll.min(rows.len().saturating_sub(1));
    for (offset, (line, label_width, url)) in
        rows.iter().skip(start).take(inner.height as usize).enumerate()
    {
        let rect = Rect {
            x: inner.x,
            y: inner.y + offset as u16,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(line.clone()), rect);
        app.zones.push(Zone::Field(start + offset), rect);
        // Clicking a hyperlink value opens it; clicking the label still edits.
        if let Some(url) = url {
            let lx = inner.x + (*label_width).min(inner.width);
            let w = inner.width.saturating_sub(*label_width);
            if w > 0 {
                app.zones.push(
                    Zone::OpenUrl(url.clone()),
                    Rect {
                        x: lx,
                        y: rect.y,
                        width: w,
                        height: 1,
                    },
                );
            }
        }
    }
}

fn render_subtasks_block(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = region_block("Subtasks");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.subtasks_rect = Some(inner);
    if inner.height == 0 {
        return;
    }

    let width = inner.width as usize;
    let start = app.subtasks_scroll.min(app.subtasks.len().saturating_sub(1));
    for (offset, sub) in app
        .subtasks
        .iter()
        .enumerate()
        .skip(start)
        .take(inner.height as usize)
    {
        let rect = Rect {
            x: inner.x,
            y: inner.y + (offset - start) as u16,
            width: inner.width,
            height: 1,
        };
        let mark = if sub.completed { "✔" } else { "○" };
        frame.render_widget(
            Paragraph::new(fit(&format!("{mark} {}", sub.name), width)),
            rect,
        );
        app.zones.push(Zone::Subtask(offset), rect);
    }
}

fn render_thread_block(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = region_block("Conversation");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.thread_rect = Some(inner);
    render_thread(frame, app, inner);
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

fn render_composer(frame: &mut Frame, app: &mut App, area: Rect) {
    // The new-task entry is a separate modal, not this composer.
    let input = app
        .input
        .as_ref()
        .filter(|i| !matches!(i.target, crate::app::InputTarget::NewTask));
    let editing_comment = matches!(
        input.map(|i| &i.target),
        Some(crate::app::InputTarget::Comment)
    );
    let active = input.is_some();
    let (title, body): (&str, String) = match input {
        Some(input) => {
            let label = match input.target {
                crate::app::InputTarget::Comment => " New comment ",
                _ => " Edit field ",
            };
            (label, format!("{}▏", input.buffer))
        }
        None => (" Comment ", "Click to add a comment…".to_string()),
    };
    let border = if active {
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
    let text_style = if active {
        Style::new()
    } else {
        Style::new().fg(Color::DarkGray)
    };
    frame.render_widget(
        Paragraph::new(body).style(text_style).wrap(Wrap { trim: false }),
        inner,
    );
    // Only the comment composer is click-to-focus; field edits open from a row.
    if !active || editing_comment {
        app.zones.push(Zone::Composer, area);
    }
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

fn render_new_task_dialog(frame: &mut Frame, buffer: &str) {
    let screen = frame.area();
    let w = 56u16.min(screen.width.saturating_sub(2));
    let h = 5u16.min(screen.height.saturating_sub(2));
    let rect = centered(screen, w, h);
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" New task ")
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height == 0 {
        return;
    }
    let [field, _gap, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(format!("{buffer}▏")).wrap(Wrap { trim: false }),
        field,
    );
    frame.render_widget(
        Paragraph::new("Enter: create (assigned to you) · Esc: cancel")
            .style(Style::new().fg(Color::DarkGray)),
        hint,
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
    let Some(pl) = app.picklist.as_ref() else {
        return;
    };
    let title = pl.title.clone();
    let multi = pl.multi;
    // (name, selected) per option.
    let options: Vec<(String, bool)> = pl
        .options
        .iter()
        .map(|o| (o.name.clone(), pl.selected.contains(&o.gid)))
        .collect();

    let screen = frame.area();
    let w = 40u16.min(screen.width.saturating_sub(2));
    let h = (options.len() as u16 + 2).clamp(3, screen.height.saturating_sub(2));
    let rect = centered(screen, w, h);
    frame.render_widget(Clear, rect);
    let suffix = if multi { " (multi) " } else { " " };
    let block = Block::bordered()
        .title(format!(" {title}{suffix}"))
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let height = inner.height as usize;
    let start = app.popup_scroll.min(options.len().saturating_sub(1));
    for (offset, (i, (name, selected))) in
        options.iter().enumerate().skip(start).take(height).enumerate()
    {
        let row = Rect {
            x: inner.x,
            y: inner.y + offset as u16,
            width: inner.width,
            height: 1,
        };
        let marker = match (multi, *selected) {
            (true, true) => "[x] ",
            (true, false) => "[ ] ",
            (false, true) => "● ",
            (false, false) => "  ",
        };
        let style = if *selected {
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        frame.render_widget(
            Paragraph::new(fit(&format!("{marker}{name}"), inner.width as usize)).style(style),
            row,
        );
        app.zones.push(Zone::EnumOption(i), row);
    }
}

fn render_datepicker_popup(frame: &mut Frame, app: &mut App) {
    let Some(dp) = app.datepicker.as_ref() else {
        return;
    };
    let (year, month_num) = (dp.year, dp.month);
    let Ok(month) = time::Month::try_from(month_num) else {
        return;
    };
    let days = month.length(year);
    let first_weekday = time::Date::from_calendar_date(year, month, 1)
        .map(|d| d.weekday().number_days_from_sunday())
        .unwrap_or(0);
    let (ty, tm, td) = today_ymd_ui();

    let screen = frame.area();
    let w = 30u16.min(screen.width.saturating_sub(2));
    let h = 12u16.min(screen.height.saturating_sub(2));
    let rect = centered(screen, w, h);
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" Pick a date ")
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.width < 21 || inner.height < 8 {
        return;
    }

    // Header: ‹  Month YYYY  › with prev/next zones.
    let header = Rect { height: 1, ..inner };
    let prev = Rect { width: 1, ..header };
    let next = Rect { x: header.x + header.width - 1, width: 1, ..header };
    frame.render_widget(Paragraph::new("‹"), prev);
    frame.render_widget(Paragraph::new("›").alignment(ratatui::layout::Alignment::Right), next);
    frame.render_widget(
        Paragraph::new(format!("{} {year}", month_name(month_num)))
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::new().add_modifier(Modifier::BOLD)),
        header,
    );
    app.zones.push(Zone::DatePrevMonth, prev);
    app.zones.push(Zone::DateNextMonth, next);

    // Weekday labels (3 cols per day).
    let grid_x = inner.x;
    let dow = Rect { y: inner.y + 1, height: 1, ..inner };
    frame.render_widget(
        Paragraph::new("Su Mo Tu We Th Fr Sa").style(Style::new().fg(Color::DarkGray)),
        dow,
    );

    // Day grid.
    let mut col = first_weekday as u16;
    let mut row: u16 = 0;
    for day in 1..=days {
        let cell = Rect {
            x: grid_x + col * 3,
            y: inner.y + 2 + row,
            width: 2,
            height: 1,
        };
        let is_today = (year, month_num, day) == (ty, tm, td);
        let style = if is_today {
            Style::new().fg(Color::Black).bg(ACCENT)
        } else {
            Style::new()
        };
        frame.render_widget(Paragraph::new(format!("{day:>2}")).style(style), cell);
        app.zones.push(Zone::DateDay(day), cell);
        col += 1;
        if col == 7 {
            col = 0;
            row += 1;
        }
    }

    // Footer: [Today] [Clear].
    let footer = Rect { y: inner.y + inner.height - 1, height: 1, ..inner };
    let today_rect = Rect { width: 7, ..footer };
    let clear_rect = Rect { x: footer.x + 8, width: 7, ..footer };
    frame.render_widget(
        Paragraph::new(" Today ").style(Style::new().fg(Color::Black).bg(Color::Green)),
        today_rect,
    );
    frame.render_widget(
        Paragraph::new(" Clear ").style(Style::new().fg(Color::White).bg(Color::Red)),
        clear_rect,
    );
    app.zones.push(Zone::DateToday, today_rect);
    app.zones.push(Zone::DateClear, clear_rect);
}

fn render_people_popup(frame: &mut Frame, app: &mut App) {
    if app.people_picker.is_none() {
        return;
    }
    let query = app.people_query.to_lowercase();
    // Keep original indices so clicks map straight back to `app.users`.
    let filtered: Vec<(usize, String)> = app
        .users
        .iter()
        .enumerate()
        .filter(|(_, u)| query.is_empty() || u.name.to_lowercase().contains(&query))
        .map(|(i, u)| (i, u.name.clone()))
        .collect();

    let screen = frame.area();
    let w = 38u16.min(screen.width.saturating_sub(2));
    let h = 18u16.min(screen.height.saturating_sub(2));
    let rect = centered(screen, w, h);
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" Assign ")
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height < 2 {
        return;
    }

    // Search box on top.
    let search = Rect { height: 1, ..inner };
    frame.render_widget(
        Paragraph::new(format!("Search: {}▏", app.people_query)).style(Style::new().fg(ACCENT)),
        search,
    );

    let list_top = inner.y + 1;
    let list_h = inner.height.saturating_sub(1) as usize;

    if app.users.is_empty() {
        frame.render_widget(
            Paragraph::new("Loading users…").style(Style::new().fg(Color::DarkGray)),
            Rect {
                y: list_top,
                height: 1,
                ..inner
            },
        );
        return;
    }

    // Item 0 unassigns; the rest are the (filtered) users.
    let mut items: Vec<(Option<usize>, String)> = vec![(None, "(unassign)".to_string())];
    items.extend(filtered.into_iter().map(|(i, name)| (Some(i), name)));

    let start = app.popup_scroll.min(items.len().saturating_sub(1));
    for (offset, (orig, name)) in items.iter().skip(start).take(list_h).enumerate() {
        let row = Rect {
            x: inner.x,
            y: list_top + offset as u16,
            width: inner.width,
            height: 1,
        };
        let style = if orig.is_none() {
            Style::new().fg(Color::DarkGray)
        } else {
            Style::new()
        };
        frame.render_widget(
            Paragraph::new(fit(&format!("  {name}"), inner.width as usize)).style(style),
            row,
        );
        match orig {
            Some(i) => app.zones.push(Zone::PeopleOption(*i), row),
            None => app.zones.push(Zone::PeopleUnassign, row),
        }
    }
}

/// Center a `w`×`h` rect within `area`.
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

fn month_name(month: u8) -> &'static str {
    [
        "January", "February", "March", "April", "May", "June", "July", "August", "September",
        "October", "November", "December",
    ]
    .get((month.saturating_sub(1)) as usize)
    .copied()
    .unwrap_or("")
}

fn today_ymd_ui() -> (i32, u8, u8) {
    let date = time::OffsetDateTime::now_utc().date();
    (date.year(), u8::from(date.month()), date.day())
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
            " click: open · drag: reorder/resize · ↑/↓: move · n: new · b: nav · q: quit "
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
/// "Priority") are colored like a status pill by keyword; metadata is
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
    let columns = app.active_columns();
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
/// Whether a value is a clickable web link.
fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

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
