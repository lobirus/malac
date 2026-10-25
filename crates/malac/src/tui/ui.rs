//! Draws a frame: header, transcript (or the welcome screen), sidebar,
//! prompt, status line, function keys and whatever floats above them.
//! Every clickable area is recorded in the app's hit map.

use std::time::Instant;

use malac_core::AgentStatus;
use malac_core::event::TodoStatus;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use super::app::{Action, App, Focus, Overlay, Target, short_path};
use super::attach::Attachment;
use super::bars::{self, Spot, TabInfo};
use super::menu::{self, Place};
use super::pig;
use super::select;
use super::text;
use super::theme::theme;
use super::transcript::Ctx;
use crate::render::tokens;
use crate::repl::{PIG, TAGLINE, update_notice};

const SIDEBAR_WIDTH: u16 = 34;
const MAX_INPUT_ROWS: usize = 8;

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.hits.clear();
    app.refresh_snapshots();
    let area = frame.area();
    if area.width < 20 || area.height < 8 {
        frame.render_widget(Paragraph::new("malac: the terminal is too small"), area);
        return;
    }
    let text_width = (area.width as usize).saturating_sub(5).max(1);
    let chips = chip_rows(&app.attachments, area.width.saturating_sub(4));
    let rows = app.editor.rows(text_width).len().clamp(1, MAX_INPUT_ROWS) + chips.len();
    let [header, body, input, status, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(rows as u16 + 2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, app, header);
    let sidebar = app.sidebar.unwrap_or(area.width >= 120) && area.width >= 80;
    app.geometry.sidebar = sidebar;
    if sidebar {
        let [main, side] =
            Layout::horizontal([Constraint::Min(30), Constraint::Length(SIDEBAR_WIDTH)])
                .areas(body);
        draw_transcript(frame, app, main);
        draw_sidebar(frame, app, side);
    } else {
        draw_transcript(frame, app, body);
    }
    draw_input(frame, app, input, &chips);
    draw_line(frame, app, status, |app, width| {
        let status = status_info(app);
        bars::status_line(&status, width)
    });
    draw_line(frame, app, keys, |_, width| bars::key_bar(width));
    draw_completions(frame, app, input);
    draw_files(frame, app, input);
    draw_overlay(frame, app);
    if app.approval_visible() {
        let position = (0, app.approvals.len());
        if let Some(approval) = app.approvals.front() {
            approval.draw(frame, position, &mut app.hits);
        }
    }
}

fn draw_line(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    layout: impl FnOnce(&App, u16) -> (Line<'static>, Vec<Spot>),
) {
    let (line, spots) = layout(app, area.width);
    frame.render_widget(Paragraph::new(line), area);
    push_spots(app, area, spots);
}

fn push_spots(app: &mut App, area: Rect, spots: Vec<Spot>) {
    for (x, width, target) in spots {
        app.hits
            .push(Rect::new(area.x + x, area.y, width, 1), target);
    }
}

fn draw_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let highlighted = match &app.overlay {
        Some(Overlay::Menu(menu)) => match menu.place {
            Place::Bar(index) => Some(index),
            _ => None,
        },
        _ => None,
    };
    let tabs: Vec<TabInfo> = (0..app.tabs.len())
        .map(|i| TabInfo {
            label: app.tab_label(i),
            mark: app.tab_mark(i),
        })
        .collect();
    let right = format!("malac {}", env!("CARGO_PKG_VERSION"));
    let (line, spots) = bars::header(highlighted, &tabs, app.active, &right, area.width);
    frame.render_widget(Paragraph::new(line), area);
    app.geometry.titles = spots
        .iter()
        .filter(|(_, _, t)| matches!(t, Target::MenuTitle(_)))
        .map(|(x, _, _)| area.x + x)
        .collect();
    app.geometry.tabs_x = spots
        .iter()
        .find(|(_, _, t)| matches!(t, Target::Tab(_)))
        .map_or(area.x, |(x, _, _)| area.x + x);
    push_spots(app, area, spots);
}

fn status_info(app: &App) -> bars::Status {
    let config = &app.session.runtime.config;
    let tab = app.tab();
    let shown = app.shown();
    let model = shown.map(|v| v.model.clone()).unwrap_or_default();
    let context = shown.filter(|v| v.context > 0).map(|v| {
        let window = config.model_config(&v.model).context_window;
        (v.context, window)
    });
    let (mut used, mut cost) = (0, 0.0);
    for (_, view) in app.agents.tree(tab.root()) {
        let u = view.total;
        used += u.input_tokens + u.cache_read_tokens + u.cache_write_tokens + u.output_tokens;
        cost += u.cost;
    }
    let web = (!app.session.web_urls.is_empty()).then_some(app.session.web_urls.len() > 1);
    bars::Status {
        left: app.activity(),
        model,
        mode: app.session.runtime.policy.mode(),
        context,
        tokens: used,
        cost,
        running: app.running_count(),
        subagents: app.agents.tree(tab.root()).len().saturating_sub(1),
        jobs: app.running_jobs(),
        spinner: app.spinner(),
        approvals: app.approvals.len(),
        web,
        update: app.update_segment(),
        branch: app.branch.clone(),
        clock: app.clock().to_owned(),
    }
}

fn draw_transcript(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let mut area = area;
    let shown = app.tab().shown().to_owned();
    if !app.tab().view.is_empty() {
        draw_breadcrumb(frame, app, Rect { height: 1, ..area });
        area.y += 1;
        area.height = area.height.saturating_sub(1);
    }
    let queued: Vec<String> = app
        .tab()
        .queued
        .iter()
        .take(3)
        .map(|p| match p.attachments.len() {
            0 => p.text.clone(),
            n => format!("{} (+{n} attached)", text::first_line(&p.text)),
        })
        .collect();
    if !queued.is_empty() {
        let rows = queued.len() as u16;
        let bottom = Rect::new(
            area.x + 1,
            area.bottom().saturating_sub(rows),
            area.width.saturating_sub(2),
            rows,
        );
        for (i, prompt) in queued.iter().enumerate() {
            let line = Line::from(vec![
                Span::styled("› ", t.faint()),
                Span::styled(text::first_line(prompt).to_owned(), t.dim()),
                Span::styled("  queued", t.faint()),
            ]);
            let line = text::truncate_line(line, bottom.width as usize);
            frame.render_widget(
                Paragraph::new(line),
                Rect::new(bottom.x, bottom.y + i as u16, bottom.width, 1),
            );
        }
        area.height = area.height.saturating_sub(rows);
    }

    let content = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    };
    app.geometry.transcript = content;
    app.hits.push(content, Target::Transcript);
    let empty = app
        .agents
        .get(&shown)
        .is_none_or(|v| v.transcript.is_empty());
    if empty && app.tab().view.is_empty() && app.tab().queued.is_empty() {
        draw_welcome(frame, app, area);
        return;
    }
    let ctx = Ctx {
        width: content.width as usize,
        now: Instant::now(),
        snapshots: &app.snapshots,
    };
    let Some(view) = app.agents.get_mut(&shown) else {
        return;
    };
    let transcript = &mut view.transcript;
    transcript.prepare(&ctx);
    let total = transcript.height();
    let height = content.height as usize;
    let max_top = total.saturating_sub(height);
    let tab = &mut app.tabs[app.active];
    let top = tab.scroll.map_or(max_top, |top| top.min(max_top));
    if tab.scroll.is_some() && top >= max_top {
        tab.scroll = None;
    }
    let selected = tab.selected.filter(|_| {
        app.focus == Focus::Transcript || matches!(app.overlay, Some(Overlay::Menu(_)))
    });
    let highlight = app
        .selection
        .as_ref()
        .filter(|s| s.agent == shown && s.width == content.width as usize);
    let buffer = frame.buffer_mut();
    for (index, first_row) in transcript.visible(top..top + height) {
        let item = &transcript.items()[index];
        let start = first_row + usize::from(item.gap());
        for (i, (line, text)) in item.lines().iter().zip(item.rows()).enumerate() {
            let row = start + i;
            if row < top || row >= top + height {
                continue;
            }
            let y = content.y + (row - top) as u16;
            let rect = Rect::new(content.x, y, content.width, 1);
            buffer.set_line(rect.x, rect.y, line, rect.width);
            if selected == Some(index) {
                buffer.set_style(rect, Style::new().bg(t.selection_bg));
                buffer[(area.x, y)]
                    .set_symbol("▎")
                    .set_style(t.fg(t.accent));
            }
            if let Some(columns) = highlight.and_then(|s| s.columns(row)) {
                let from = columns.start.max(text.inset);
                let to = columns
                    .end
                    .min(select::text_end(line))
                    .min(rect.width as usize);
                for x in from..to {
                    buffer[(rect.x + x as u16, y)].set_style(t.highlight());
                }
            }
            app.hits.push(rect, Target::Entry { index, line: i });
        }
    }
    if let Some(top) = tab.scroll {
        let below = total.saturating_sub(top + height);
        let label = if total > tab.seen {
            " new output ↓ ".to_owned()
        } else {
            format!(" ↓ {below} more ")
        };
        let width = text::width(&label) as u16;
        let rect = Rect::new(
            content.right().saturating_sub(width),
            content.bottom().saturating_sub(1),
            width,
            1,
        );
        let style = if total > tab.seen {
            t.selected()
        } else {
            Style::new().bg(t.selection_bg)
        };
        frame.render_widget(Paragraph::new(Line::styled(label, style)), rect);
        app.hits.push(rect, Target::Do(Action::JumpBottom));
    }
}

fn draw_breadcrumb(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let tab = app.tab();
    let mut ids = vec![tab.root().to_owned()];
    ids.extend(tab.view.iter().cloned());
    let mut spans = vec![Span::styled(" ‹ ", t.fg(t.accent))];
    let mut x = area.x + 3;
    let back = Rect::new(area.x, area.y, 3, 1);
    for (i, id) in ids.iter().enumerate() {
        let label = app
            .agents
            .get(id)
            .map_or("agent", |v| v.label.as_str())
            .to_owned();
        if i > 0 {
            spans.push(Span::styled(" › ", t.faint()));
            x += 3;
        }
        let last = i + 1 == ids.len();
        let style = if last { t.bold() } else { t.dim() };
        x += text::width(&label) as u16;
        spans.push(Span::styled(label, style));
    }
    spans.push(Span::styled("   esc back", t.faint()));
    let width = x.saturating_sub(area.x);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    app.hits.push(back, Target::Do(Action::Back));
    app.hits.push(
        Rect::new(area.x, area.y, width, 1),
        Target::Do(Action::Back),
    );
}

fn welcome_text(app: &App) -> Vec<(Line<'static>, Option<Action>)> {
    let t = theme();
    let runtime = &app.session.runtime;
    let model = app.shown().map(|v| v.model.clone()).unwrap_or_default();
    let mut lines = vec![
        (
            Line::from(vec![
                Span::styled(format!("malac {}", env!("CARGO_PKG_VERSION")), t.bold()),
                Span::styled(format!(" · {TAGLINE}"), t.dim()),
            ]),
            None,
        ),
        (Line::default(), None),
    ];
    let field = |name: &str, value: String, style: Style| {
        Line::from(vec![
            Span::styled(format!("{name:<8}"), t.dim()),
            Span::styled(value, style),
        ])
    };
    lines.push((field("model", model, Style::new()), Some(Action::PickModel)));
    let mode = runtime.policy.mode();
    let mode_style = if mode == malac_core::permission::Mode::Autopilot {
        t.fg(t.warning)
    } else {
        Style::new()
    };
    lines.push((
        field("mode", mode.as_str().to_owned(), mode_style),
        Some(Action::CycleMode),
    ));
    lines.push((
        field("cwd", short_path(&app.tab_cwd()), Style::new()),
        Some(Action::FolderDialog),
    ));
    for url in &app.session.web_urls {
        lines.push((
            field(
                "web",
                url.clone(),
                t.fg(t.accent).add_modifier(Modifier::UNDERLINED),
            ),
            Some(Action::CopyWebUrl),
        ));
    }
    if let Some(notice) = update_notice(&app.update) {
        lines.push((
            Line::styled(notice, t.fg(t.accent)),
            Some(Action::UpdateDialog),
        ));
    }
    lines.push((Line::default(), None));
    for tip in [
        "Describe a task and press Enter; Shift+Enter or Ctrl+J adds a line.",
        "F4 picks a model, F5 cycles the mode, F9 opens the menus.",
        "/ lists commands, F1 shows every key, Ctrl+T opens a tab.",
        "@ attaches a file, Ctrl+V an image; drag over text to copy it.",
    ] {
        lines.push((Line::styled(tip, t.dim()), None));
    }
    if !app.recent.is_empty() {
        lines.push((Line::default(), None));
        lines.push((Line::styled("Recent sessions here", t.bold()), None));
        for summary in &app.recent {
            let when = summary
                .updated
                .with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string();
            lines.push((
                Line::from(vec![
                    Span::styled(format!("{when}  "), t.dim()),
                    Span::raw(summary.title.clone()),
                ]),
                Some(Action::Resume(summary.meta.id.clone())),
            ));
        }
    }
    lines
}

fn draw_welcome(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let text = welcome_text(app);
    let text_width = text.iter().map(|(l, _)| l.width()).max().unwrap_or(0) as u16;
    let text_height = text.len() as u16;
    let pig_w = pig::WIDTH as u16;
    let pig_h = pig::HEIGHT as u16;
    let art_ok = !t.no_color;
    let side_by_side = art_ok
        && area.width >= pig_w + 4 + text_width.min(60)
        && area.height >= pig_h.max(text_height);
    let stacked = art_ok && area.width >= pig_w + 2 && area.height >= pig_h + 1 + text_height;

    let mut put =
        |frame: &mut Frame, x: u16, y: u16, line: Line<'static>, action: Option<Action>| {
            if y >= area.bottom() || x >= area.right() {
                return;
            }
            let width = area.right() - x;
            let line = text::truncate_line(line, width as usize);
            let used = (line.width() as u16).min(width);
            frame.render_widget(Paragraph::new(line), Rect::new(x, y, width, 1));
            if let Some(action) = action {
                app.hits
                    .push(Rect::new(x, y, used.max(1), 1), Target::Do(action));
            }
        };

    if side_by_side {
        let total_w = pig_w + 4 + text_width.min(area.width - pig_w - 4);
        let x = area.x + (area.width.saturating_sub(total_w)) / 2;
        let y = area.y + (area.height.saturating_sub(pig_h)) / 2;
        for (i, line) in pig::lines(t.truecolor).into_iter().enumerate() {
            put(frame, x, y + i as u16, line, None);
        }
        let text_y = y + pig_h.saturating_sub(text_height) / 2;
        for (i, (line, action)) in text.into_iter().enumerate() {
            put(frame, x + pig_w + 4, text_y + i as u16, line, action);
        }
    } else if stacked {
        let y = area.y + (area.height.saturating_sub(pig_h + 1 + text_height)) / 2;
        let pig_x = area.x + (area.width - pig_w) / 2;
        for (i, line) in pig::lines(t.truecolor).into_iter().enumerate() {
            put(frame, pig_x, y + i as u16, line, None);
        }
        let x = area.x + (area.width.saturating_sub(text_width)) / 2;
        for (i, (line, action)) in text.into_iter().enumerate() {
            put(frame, x, y + pig_h + 1 + i as u16, line, action);
        }
    } else {
        let art_w = PIG[0].len() as u16;
        let x = area.x + 1;
        let y = area.y
            + (area
                .height
                .saturating_sub(text_height.max(PIG.len() as u16)))
                / 3;
        for (i, art) in PIG.iter().enumerate() {
            put(frame, x, y + i as u16, Line::styled(*art, t.bold()), None);
        }
        for (i, (line, action)) in text.into_iter().enumerate() {
            put(frame, x + art_w + 1, y + i as u16, line, action);
        }
    }
}

fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let buffer = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        buffer[(area.x, y)].set_symbol("│").set_style(t.faint());
    }
    let inner = Rect::new(
        area.x + 2,
        area.y,
        area.width.saturating_sub(3),
        area.height,
    );
    let width = inner.width as usize;
    let mut lines: Vec<(Line<'static>, Option<Action>)> = Vec::new();
    let heading = |title: &str| (Line::styled(title.to_owned(), t.bold().fg(t.dim)), None);

    lines.push(heading("Agents"));
    let shown = app.tab().shown().to_owned();
    for (depth, view) in app.agents.tree(app.tab().root()) {
        let snapshot = app.snapshots.get(&view.id);
        let (mark, style) = match snapshot.map(|s| s.status) {
            Some(AgentStatus::Running) => (app.spinner(), t.dim()),
            Some(AgentStatus::Completed) => ("●", t.fg(t.accent)),
            Some(AgentStatus::Failed) => ("●", t.fg(t.error)),
            Some(AgentStatus::Cancelled) => ("●", t.fg(t.warning)),
            _ => ("○", t.faint()),
        };
        let label_style = if view.id == shown {
            t.bold()
        } else {
            Style::new()
        };
        let indent = "  ".repeat(depth);
        lines.push((
            Line::from(vec![
                Span::raw(indent.clone()),
                Span::styled(format!("{mark} "), style),
                Span::styled(view.label.clone(), label_style),
            ]),
            Some(Action::OpenAgent(view.id.clone())),
        ));
        if let Some(activity) = snapshot
            .map(|s| s.activity.as_str())
            .filter(|a| !a.is_empty())
        {
            lines.push((
                Line::styled(format!("{indent}  {}", text::first_line(activity)), t.dim()),
                Some(Action::OpenAgent(view.id.clone())),
            ));
        }
    }

    if let Some(view) = app.shown().filter(|v| !v.todos.is_empty()) {
        lines.push((Line::default(), None));
        lines.push(heading("Todos"));
        for todo in &view.todos {
            let (mark, style) = match todo.status {
                TodoStatus::Completed => ("✓", t.dim().add_modifier(Modifier::CROSSED_OUT)),
                TodoStatus::InProgress => ("▸", t.bold()),
                TodoStatus::Pending => ("○", Style::new()),
            };
            lines.push((
                Line::from(vec![
                    Span::styled(
                        format!("{mark} "),
                        if mark == "✓" { t.fg(t.accent) } else { style },
                    ),
                    Span::styled(todo.content.clone(), style),
                ]),
                None,
            ));
        }
    }

    lines.push((Line::default(), None));
    lines.push(heading("Usage"));
    let mut total = malac_core::message::Usage::default();
    for (_, view) in app.agents.tree(app.tab().root()) {
        total.add(&view.total);
    }
    lines.push((
        Line::styled(
            format!(
                "in {} · out {}",
                tokens(total.input_tokens + total.cache_read_tokens),
                tokens(total.output_tokens)
            ),
            t.dim(),
        ),
        Some(Action::ShowCost),
    ));
    if total.cost > 0.0 {
        lines.push((
            Line::styled(format!("${:.4}", total.cost), t.dim()),
            Some(Action::ShowCost),
        ));
    }

    let servers = app.session.runtime.servers();
    if !servers.is_empty() {
        lines.push((Line::default(), None));
        lines.push(heading("MCP"));
        for server in servers {
            let (mark, style) = match server.error {
                Some(_) => ("✗", t.fg(t.error)),
                None => ("●", t.fg(t.accent)),
            };
            lines.push((
                Line::from(vec![
                    Span::styled(format!("{mark} "), style),
                    Span::raw(server.name),
                    Span::styled(
                        if server.tools > 0 {
                            format!("  {} tools", server.tools)
                        } else {
                            String::new()
                        },
                        t.dim(),
                    ),
                ]),
                Some(Action::ShowMcp),
            ));
        }
    }
    if let Some(url) = app.session.web_urls.first() {
        lines.push((Line::default(), None));
        lines.push(heading("Web UI"));
        lines.push((
            Line::styled(text::truncate(url, width).into_owned(), t.fg(t.accent)),
            Some(Action::CopyWebUrl),
        ));
    }

    for (i, (line, action)) in lines.into_iter().take(inner.height as usize).enumerate() {
        let rect = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        let line = text::truncate_line(line, width);
        frame.render_widget(Paragraph::new(line), rect);
        if let Some(action) = action {
            app.hits.push(rect, Target::Do(action));
        }
    }
}

/// One attachment chip: which attachment, where it starts and its text.
struct Chip {
    index: usize,
    x: u16,
    label: String,
}

/// Attachments laid out as chips in rows of at most `width` columns.
fn chip_rows(attachments: &[Attachment], width: u16) -> Vec<Vec<Chip>> {
    let mut rows: Vec<Vec<Chip>> = Vec::new();
    let mut x = 0;
    for (index, attachment) in attachments.iter().enumerate() {
        let room = (width as usize).saturating_sub(6).max(8);
        let label = format!(" {} ", text::truncate(&attachment.label(), room));
        let w = text::width(&label) as u16 + 2;
        if rows.is_empty() || (x > 0 && x + w > width) {
            rows.push(Vec::new());
            x = 0;
        }
        if let Some(row) = rows.last_mut() {
            row.push(Chip { index, x, label });
        }
        x += w + 1;
    }
    rows
}

fn draw_input(frame: &mut Frame, app: &mut App, area: Rect, chips: &[Vec<Chip>]) {
    let t = theme();
    let focused = app.focus == Focus::Input && app.overlay.is_none() && !app.approval_visible();
    let border = if focused { t.fg(t.accent) } else { t.faint() };
    let hint = if app.tab().read_only() {
        ""
    } else if app.busy() {
        " enter queues · esc stops "
    } else if !app.attachments.is_empty() {
        " enter send · ⌫ at the start drops the last file "
    } else {
        " enter send · @ attach a file "
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title_bottom(Line::styled(hint, t.faint()).right_aligned());
    let mut inner = block.inner(area);
    frame.render_widget(block, area);
    for (y, row) in chips.iter().enumerate() {
        let y = inner.y + y as u16;
        for chip in row {
            let x = inner.x + chip.x;
            let chip_style = Style::new().bg(t.selection_bg);
            let label_width = text::width(&chip.label) as u16;
            let line = Line::from(vec![
                Span::styled(chip.label.clone(), chip_style),
                Span::styled("× ", chip_style.fg(t.dim)),
            ]);
            frame.render_widget(Paragraph::new(line), Rect::new(x, y, label_width + 2, 1));
            app.hits.push(
                Rect::new(x + label_width.saturating_sub(1), y, 3, 1),
                Target::Chip(chip.index),
            );
        }
    }
    let chip_rows = (chips.len() as u16).min(inner.height.saturating_sub(1));
    inner.y += chip_rows;
    inner.height -= chip_rows;
    app.geometry.input = inner;
    app.hits.push(inner, Target::Input);
    if app.tab().read_only() {
        let line = Line::styled(
            "this tab watches a subagent · prompts go to session tabs",
            t.dim(),
        );
        frame.render_widget(Paragraph::new(line), inner);
        return;
    }
    let width = app.input_width();
    if app.editor.is_empty() {
        let line = Line::from(vec![
            Span::styled("› ", t.dim()),
            Span::styled(
                "Ask malac to do something · / for commands · F1 help",
                t.faint(),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(text::truncate_line(line, inner.width as usize)),
            inner,
        );
        if focused {
            frame.set_cursor_position((inner.x + 2, inner.y));
        }
        app.geometry.input_first = 0;
        return;
    }
    let rows = app.editor.rows(width);
    let (cursor_row, cursor_col) = app.editor.cursor_position(width);
    let height = inner.height as usize;
    let first = cursor_row.saturating_sub(height.saturating_sub(1));
    app.geometry.input_first = first;
    let source = app.editor.text();
    let selection = app.editor.selection();
    for (i, row) in rows.iter().enumerate().skip(first).take(height) {
        let starts_line = row.start == 0 || source.as_bytes()[row.start - 1] == b'\n';
        let prefix = if i == 0 {
            Span::styled("› ", t.dim())
        } else if starts_line {
            Span::styled("· ", t.faint())
        } else {
            Span::raw("  ")
        };
        let mut spans = vec![prefix];
        // The selected part of the row, if any, split out to highlight it.
        let cut = selection
            .as_ref()
            .map(|s| {
                (
                    s.start.clamp(row.start, row.end),
                    s.end.clamp(row.start, row.end),
                )
            })
            .filter(|(from, to)| from < to);
        match cut {
            Some((from, to)) => {
                spans.push(Span::raw(source[row.start..from].to_owned()));
                spans.push(Span::styled(source[from..to].to_owned(), t.highlight()));
                spans.push(Span::raw(source[to..row.end].to_owned()));
            }
            None => spans.push(Span::raw(source[row.start..row.end].to_owned())),
        }
        let y = inner.y + (i - first) as u16;
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }
    if focused {
        let x = inner.x + 2 + cursor_col as u16;
        let y = inner.y + (cursor_row - first) as u16;
        frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), y));
    }
}

fn draw_completions(frame: &mut Frame, app: &mut App, input: Rect) {
    let completions = app.completions();
    if completions.is_empty() || app.overlay.is_some() || app.approval_visible() {
        return;
    }
    let t = theme();
    let shown = completions.len().min(8);
    let selected = app.completion.min(completions.len() - 1);
    let first = selected.saturating_sub(shown - 1);
    let width = input.width.min(72);
    let height = shown as u16 + 2;
    let area = Rect::new(input.x, input.y.saturating_sub(height), width, height);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.faint());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    for (row, (index, (name, args, help))) in completions
        .iter()
        .enumerate()
        .skip(first)
        .take(shown)
        .enumerate()
    {
        let style = if index == selected {
            t.selected()
        } else {
            Style::new()
        };
        let help_style = if index == selected { style } else { t.dim() };
        let command = format!(" /{name} {args}");
        let gap = 26usize.saturating_sub(text::width(&command));
        let line = Line::from(vec![
            Span::styled(command, style),
            Span::styled(" ".repeat(gap), style),
            Span::styled(format!("{help} "), help_style),
        ]);
        let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        let line = text::truncate_line(line, inner.width as usize);
        frame.render_widget(Paragraph::new(line).style(style), rect);
        app.hits.push(rect, Target::Completion(index));
    }
}

/// The `@` file picker, above the prompt like the command completions.
fn draw_files(frame: &mut Frame, app: &mut App, input: Rect) {
    if app.file_query().is_none() || app.overlay.is_some() || app.approval_visible() {
        return;
    }
    let t = theme();
    let matches = app.file_matches();
    let shown = matches.len().clamp(1, 8);
    let selected = app.file_choice.min(matches.len().saturating_sub(1));
    let first = selected.saturating_sub(shown - 1);
    let width = input.width.min(80);
    let height = shown as u16 + 2;
    let area = Rect::new(input.x, input.y.saturating_sub(height), width, height);
    frame.render_widget(Clear, area);
    let folder = app.files.root.file_name().map_or_else(
        || short_path(&app.files.root),
        |name| name.to_string_lossy().into_owned(),
    );
    let room = (width as usize).saturating_sub(30);
    let title = format!(" @ {} ", text::truncate(&folder, room));
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.faint())
        .title(Line::styled(title, t.dim()))
        .title_bottom(Line::styled(" tab attach · esc close ", t.faint()).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if matches.is_empty() {
        let note = if app.files.list.is_none() {
            " indexing files…"
        } else {
            " no matching files"
        };
        frame.render_widget(Paragraph::new(Line::styled(note, t.dim())), inner);
        return;
    }
    for (row, (index, found)) in matches
        .iter()
        .enumerate()
        .skip(first)
        .take(shown)
        .enumerate()
    {
        let style = if index == selected {
            t.selected()
        } else {
            Style::new()
        };
        let hit = if index == selected {
            style.add_modifier(Modifier::BOLD)
        } else {
            t.fg(t.accent).add_modifier(Modifier::BOLD)
        };
        let mut spans = vec![Span::styled(" ", style)];
        for (i, c) in found.path.chars().enumerate() {
            let s = if found.positions.contains(&i) {
                hit
            } else {
                style
            };
            spans.push(Span::styled(c.to_string(), s));
        }
        let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        let line = text::truncate_line(Line::from(spans), inner.width as usize);
        frame.render_widget(Paragraph::new(line).style(style), rect);
        app.hits.push(rect, Target::FileMatch(index));
    }
}

fn draw_overlay(frame: &mut Frame, app: &mut App) {
    // Rebuilt every frame so status and activity stay live.
    if matches!(app.overlay, Some(Overlay::Agents(_))) {
        let rows = app.agent_rows();
        if let Some(Overlay::Agents(picker)) = &mut app.overlay {
            picker.set_rows(rows);
        }
    }
    if matches!(app.overlay, Some(Overlay::AgentMap(_))) {
        let nodes = app.agent_nodes();
        let spinner = app.spinner();
        if let Some(Overlay::AgentMap(map)) = &mut app.overlay {
            map.set_nodes(nodes, spinner);
        }
    }
    let anchor = match &app.overlay {
        Some(Overlay::Menu(menu)) => match menu.place {
            Place::Bar(index) => (app.geometry.titles.get(index).copied().unwrap_or(0), 1),
            Place::Tabs => (app.geometry.tabs_x, 1),
            Place::At(x, y) => (x, y),
        },
        _ => (0, 0),
    };
    let hits = &mut app.hits;
    match &mut app.overlay {
        Some(Overlay::Menu(menu)) => menu::draw(frame, menu, anchor, hits),
        Some(Overlay::Models(p) | Overlay::Sessions(p) | Overlay::Agents(p)) => p.draw(frame, hits),
        Some(Overlay::AgentMap(map)) => map.draw(frame, hits),
        Some(Overlay::Folder(browser)) => browser.draw(frame, hits),
        Some(Overlay::Jobs(view)) => view.draw(frame, &app.jobs, hits),
        Some(Overlay::Help(help)) => help.draw(frame, hits),
        Some(Overlay::Rename(rename)) => rename.draw(frame, hits),
        Some(Overlay::Update(dialog)) => dialog.draw(frame, &app.update, hits),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, OnceLock, RwLock};
    use std::time::{Duration, Instant};

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use malac_core::config::{Config, ProviderConfig};
    use malac_core::event::{Envelope, Event, TurnStatus};
    use malac_core::permission::{Decision, PermissionRequest};
    use malac_core::{AgentSpec, Runtime};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;
    use serde_json::json;
    use tokio::sync::{mpsc, oneshot};

    use super::*;
    use crate::repl::Session;
    use crate::tui::Pending;

    /// Sessions written by these tests go to a scratch directory.
    fn scratch_home() -> &'static PathBuf {
        static HOME: OnceLock<PathBuf> = OnceLock::new();
        HOME.get_or_init(|| {
            let dir = std::env::temp_dir().join("malac-tui-tests");
            std::fs::create_dir_all(dir.join("project")).unwrap();
            unsafe { std::env::set_var("MALAC_HOME", &dir) };
            dir
        })
    }

    fn app() -> App {
        let home = scratch_home();
        let mut config = Config {
            model: Some("lora/smart".into()),
            ..Default::default()
        };
        config.providers.insert(
            "lora".into(),
            ProviderConfig {
                base_url: "http://127.0.0.1:9/v1".into(),
                models: vec!["smart".into(), "fast".into()],
                ..Default::default()
            },
        );
        config.control.enabled = false;
        let runtime = Runtime::new(config, home.join("project")).unwrap();
        let handle = runtime.spawn_agent(AgentSpec::default()).unwrap();
        let session = Session {
            main: Arc::new(RwLock::new(handle.id.clone())),
            runtime,
            handle,
            approver: None,
            web_urls: Vec::new(),
        };
        let (tx, _) = mpsc::unbounded_channel();
        let mut app = App::new(session, tx);
        app.recent.clear();
        app.branch = Some("main".into());
        app
    }

    fn draw_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        app.tick();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn screen(buffer: &Buffer) -> String {
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                let row: String = (0..area.width).map(|x| buffer[(x, y)].symbol()).collect();
                row.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render(app: &mut App, width: u16, height: u16) -> String {
        screen(&draw_buffer(app, width, height))
    }

    fn copied() -> Option<String> {
        crate::tui::term::COPIED.with(|c| c.borrow_mut().take())
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(press(KeyCode::Char(c)), false);
        }
    }

    fn emit(app: &mut App, event: Event) {
        let agent = app.tab().root().to_owned();
        app.on_bus(&Envelope {
            agent,
            seq: 0,
            at: chrono::Utc::now(),
            event,
        });
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Presses a key and performs what it asks for.
    async fn key(app: &mut App, code: KeyCode) {
        if let Some(action) = app.on_key(press(code), false) {
            app.perform(action).await;
        }
    }

    fn mouse(app: &mut App, kind: MouseEventKind, x: u16, y: u16) -> Option<Action> {
        app.on_mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }

    /// Presses and releases; what either asks for.
    fn click(app: &mut App, x: u16, y: u16, button: MouseButton) -> Option<Action> {
        let pressed = mouse(app, MouseEventKind::Down(button), x, y);
        let released = mouse(app, MouseEventKind::Up(button), x, y);
        pressed.or(released)
    }

    /// Where `needle` is on screen.
    fn find(frame: &str, needle: &str) -> (u16, u16) {
        for (y, line) in frame.lines().enumerate() {
            if let Some(i) = line.find(needle) {
                return (text::width(&line[..i]) as u16, y as u16);
            }
        }
        panic!("`{needle}` is not on screen:\n{frame}");
    }

    fn a_turn(app: &mut App) {
        emit(
            app,
            Event::UserMessage {
                text: "run the tests".into(),
            },
        );
        emit(
            app,
            Event::TextDelta {
                text: "Running the **test suite** now.".into(),
            },
        );
        emit(
            app,
            Event::ToolStarted {
                call_id: "c1".into(),
                tool: "bash".into(),
                summary: "cargo test".into(),
                input: json!({"command": "cargo test"}),
            },
        );
        emit(
            app,
            Event::ToolFinished {
                call_id: "c1".into(),
                tool: "bash".into(),
                output: "running 46 tests\ntest result: ok. 46 passed".into(),
                is_error: false,
                millis: 2300,
            },
        );
        emit(
            app,
            Event::TextDelta {
                text: "All 46 tests pass.".into(),
            },
        );
        emit(
            app,
            Event::TurnFinished {
                status: TurnStatus::Completed,
                text: "All 46 tests pass.".into(),
            },
        );
    }

    fn answer(app: &mut App, prompt: &str, text: &str) {
        emit(
            app,
            Event::UserMessage {
                text: prompt.into(),
            },
        );
        emit(app, Event::TextDelta { text: text.into() });
        emit(
            app,
            Event::TurnFinished {
                status: TurnStatus::Completed,
                text: text.into(),
            },
        );
    }

    #[tokio::test]
    async fn answers_lead_and_tool_calls_recede() {
        let mut app = app();
        a_turn(&mut app);
        let buffer = draw_buffer(&mut app, 90, 24);
        let frame = screen(&buffer);
        println!("{frame}");
        let t = theme();
        let (x, y) = find(&frame, "All 46 tests pass.");
        assert_eq!(buffer[(x - 2, y)].symbol(), "┃", "the answer has a rule");
        assert_eq!(buffer[(x - 2, y)].fg, t.accent, "in the accent");
        assert_eq!(
            buffer[(x, y)].fg,
            Color::Reset,
            "its text is the plain foreground"
        );
        assert!(!buffer[(x, y)].modifier.contains(Modifier::DIM));
        let (x, y) = find(&frame, "Running the");
        assert_eq!(buffer[(x - 2, y)].symbol(), "│");
        assert_eq!(
            buffer[(x - 2, y)].fg,
            t.faint,
            "text before the answer has a quiet rule"
        );
        assert_eq!(buffer[(x, y)].fg, Color::Reset);
        let (tool_x, tool_y) = find(&frame, "bash cargo test");
        assert_eq!(buffer[(tool_x, tool_y)].fg, t.dim, "tool calls are grey");
        assert_eq!(tool_x, x + 2, "and nested under the text");
        assert_eq!(
            frame.lines().nth(tool_y as usize + 1),
            Some(""),
            "on one line"
        );
        let (x, y) = find(&frame, "› run the tests");
        assert_eq!(buffer[(x + 2, y)].bg, t.code_bg, "prompts keep their band");
    }

    #[tokio::test]
    async fn dragging_selects_text_and_copies_it() {
        let mut app = app();
        answer(
            &mut app,
            "why",
            "The parser drops the last token when the input ends without a newline. \
             A final check fixes it.\n\nRun `cargo test` to see.",
        );
        let frame = render(&mut app, 60, 24);
        println!("{frame}");
        let (x0, y0) = find(&frame, "parser");
        let (x1, y1) = find(&frame, "final");
        assert!(y1 > y0, "the paragraph wraps");
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x0, y0);
        mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            x1 + 4,
            y1,
        );
        let buffer = draw_buffer(&mut app, 60, 24);
        let t = theme();
        assert_eq!(
            buffer[(x0, y0)].bg,
            t.highlight_bg,
            "the selection is highlighted"
        );
        assert_ne!(buffer[(x0 - 1, y0)].bg, t.highlight_bg);
        assert_eq!(buffer[(x1 + 4, y1)].bg, t.highlight_bg);
        assert_ne!(buffer[(x1 + 5, y1)].bg, t.highlight_bg);
        assert_eq!(
            mouse(&mut app, MouseEventKind::Up(MouseButton::Left), x1 + 4, y1),
            None,
            "a drag is not a click"
        );
        let text = copied().unwrap();
        assert!(text.starts_with("parser drops the last token"), "{text}");
        assert!(
            text.ends_with("newline. A final"),
            "wrapped rows join with a space: {text}"
        );
        assert!(render(&mut app, 60, 24).contains("✓ Copied"));

        app.on_key(press(KeyCode::Esc), false);
        assert!(app.selection.is_none(), "Esc clears the selection");

        let (x, y) = find(&frame, "cargo");
        click(&mut app, x + 1, y, MouseButton::Left);
        click(&mut app, x + 1, y, MouseButton::Left);
        assert_eq!(
            copied().as_deref(),
            Some("cargo"),
            "a double click takes a word"
        );
        let (x, y) = find(&frame, "without");
        click(&mut app, x, y, MouseButton::Left);
        click(&mut app, x, y, MouseButton::Left);
        click(&mut app, x, y, MouseButton::Left);
        let line = copied().unwrap();
        assert!(
            line.starts_with("The parser") && line.ends_with("fixes it."),
            "{line}"
        );
        click(&mut app, 0, 0, MouseButton::Left);
        assert!(app.selection.is_none(), "a click elsewhere clears it");
    }

    #[tokio::test]
    async fn dragging_past_the_top_scrolls_the_selection_along() {
        let mut app = app();
        let rows: Vec<String> = (1..=40).map(|i| format!("row {i}")).collect();
        answer(&mut app, "count", &rows.join("\n\n"));
        let frame = render(&mut app, 60, 24);
        let first_shown = (1..=40)
            .find(|i| frame.contains(&format!("row {i}\n")))
            .unwrap();
        let (x, y) = find(&frame, "row 40");
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x + 5, y);
        let above = app.geometry.transcript.y - 1;
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), x, above);
        assert!(
            app.tab().scroll.is_some(),
            "dragging past the top scrolls up"
        );
        for _ in 0..6 {
            app.tick();
        }
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), x, above);
        let text = copied().unwrap();
        assert!(text.ends_with("row 39\n\nrow 40"), "{text}");
        let earlier = format!("row {}", first_shown - 2);
        assert!(
            text.contains(&earlier),
            "holding past the edge keeps scrolling: {text}"
        );
    }

    #[tokio::test]
    async fn the_prompt_selects_with_the_mouse_too() {
        let mut app = app();
        type_text(&mut app, "rename the parser module");
        let frame = render(&mut app, 80, 20);
        let (x, y) = find(&frame, "parser");
        click(&mut app, x, y, MouseButton::Left);
        click(&mut app, x, y, MouseButton::Left);
        assert_eq!(copied().as_deref(), Some("parser"));
        let buffer = draw_buffer(&mut app, 80, 20);
        assert_eq!(buffer[(x, y)].bg, theme().highlight_bg);
        type_text(&mut app, "lexer");
        assert_eq!(
            app.editor.text(),
            "rename the lexer module",
            "typing replaces it"
        );
    }

    #[tokio::test]
    async fn the_agent_map_draws_the_tree() {
        let mut app = app();
        let root = app.tab().root().to_owned();
        let runtime = app.session.runtime.clone();
        let mut spawn = |label: &str, parent: &str, depth| {
            let spec = AgentSpec {
                label: label.into(),
                parent: Some(parent.to_owned()),
                depth,
                ..Default::default()
            };
            let handle = runtime.spawn_agent(spec).unwrap();
            app.on_bus(&Envelope {
                agent: handle.id.clone(),
                seq: 0,
                at: chrono::Utc::now(),
                event: Event::AgentStarted {
                    label: label.into(),
                    parent: Some(parent.to_owned()),
                    model: "lora/fast".into(),
                    cwd: "/work".into(),
                },
            });
            handle.id
        };
        let lister = spawn("lister", &root, 1);
        let grep = spawn("grep", &lister, 2);
        let tester = spawn("tester", &root, 1);
        let (reply, _answer) = oneshot::channel();
        app.on_approval(Pending {
            request: PermissionRequest {
                id: "r1".into(),
                agent: grep.clone(),
                agent_label: "grep".into(),
                tool: "bash".into(),
                summary: "rg parse".into(),
                input: json!({"command": "rg parse"}),
            },
            reply,
        });
        app.approvals_hidden = true;
        let frame = render(&mut app, 120, 34);
        assert!(frame.contains("3 subagents"), "the footer counts them");
        let (x, y) = find(&frame, "3 subagents");
        let action = click(&mut app, x, y, MouseButton::Left).unwrap();
        assert_eq!(action, Action::AgentMap);
        app.perform(action).await;
        let frame = render(&mut app, 120, 34);
        println!("{frame}");
        assert!(frame.contains("Agent map"));
        assert!(frame.contains(" main "));
        assert!(frame.contains(" tab 1 "));
        assert!(frame.contains(" lister "));
        assert!(frame.contains(" d1 "));
        assert!(frame.contains(" d2 "));
        assert!(
            frame.contains("? awaiting approval"),
            "grep waits for approval"
        );
        assert!(frame.contains("┃──┬──▸│"), "children hang off their parent");
        assert!(frame.contains("│─────▸│"));
        let (lister_x, lister_y) = find(&frame, " lister ");
        let (tester_x, tester_y) = find(&frame, " tester ");
        let (grep_x, _) = find(&frame, " grep ");
        assert_eq!(lister_x, tester_x, "siblings share a column");
        assert!(tester_y > lister_y && grep_x > lister_x);
        assert!(frame.contains("┏"), "the shown agent is selected");

        click(&mut app, tester_x + 2, tester_y + 2, MouseButton::Left);
        let action = click(&mut app, tester_x + 2, tester_y + 2, MouseButton::Left);
        assert_eq!(
            action,
            Some(Action::OpenAgent(tester.clone())),
            "a double click opens"
        );
        assert!(app.overlay.is_none());
        app.perform(Action::AgentMap).await;
        render(&mut app, 120, 34);
        app.on_key(press(KeyCode::Right), false);
        assert_eq!(
            app.on_key(press(KeyCode::Char('t')), false),
            Some(Action::PinAgent(lister.clone()))
        );
        app.perform(Action::AgentMap).await;
        assert_eq!(
            app.on_key(press(KeyCode::F(3)), false),
            Some(Action::AgentList)
        );
        app.perform(Action::AgentList).await;
        assert!(matches!(app.overlay, Some(Overlay::Agents(_))));
        app.overlay = None;
        key(&mut app, KeyCode::F(3)).await;
        assert!(
            matches!(app.overlay, Some(Overlay::Agents(_))),
            "F3 opens the last view"
        );
    }

    #[tokio::test]
    async fn at_attaches_files_from_the_folder() {
        let mut app = app();
        let root = app.tab_cwd();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("target/main.o"), "").unwrap();
        std::fs::write(root.join("notes.md"), "# notes\n").unwrap();
        app.files = crate::tui::app::FileIndex {
            root: root.clone(),
            list: Some(crate::tui::files::index(&root)),
            loading: false,
        };
        type_text(&mut app, "explain @mai");
        let frame = render(&mut app, 90, 24);
        println!("{frame}");
        assert!(frame.contains("src/main.rs"));
        assert!(!frame.contains("target/main.o"), ".gitignore is honored");
        app.on_key(press(KeyCode::Tab), false);
        assert_eq!(app.editor.text(), "explain ");
        let frame = render(&mut app, 90, 24);
        println!("{frame}");
        assert!(
            frame.contains("▤ src/main.rs · 13 B ×"),
            "the file is a chip"
        );

        app.on_paste(&root.join("notes.md").display().to_string());
        assert_eq!(app.attachments.len(), 2, "a dropped path attaches");
        assert_eq!(app.editor.text(), "explain ");
        let frame = render(&mut app, 90, 24);
        let (x, y) = find(&frame, "notes.md · 8 B ×");
        click(&mut app, x + 14, y, MouseButton::Left);
        assert_eq!(app.attachments.len(), 1, "clicking × removes a chip");
        app.editor.set_text("");
        app.on_key(press(KeyCode::Backspace), false);
        assert!(
            app.attachments.is_empty(),
            "Backspace at the start removes one"
        );
    }

    #[tokio::test]
    async fn choosing_a_folder_opens_a_tab_there() {
        let mut app = app();
        let dir = app.tab_cwd().join("elsewhere");
        std::fs::create_dir_all(&dir).unwrap();
        type_text(&mut app, "/cd");
        let action = app.on_key(press(KeyCode::Enter), false).unwrap();
        assert_eq!(action, Action::FolderDialog);
        app.perform(action).await;
        let frame = render(&mut app, 100, 30);
        println!("{frame}");
        assert!(frame.contains("Open folder"));
        assert!(frame.contains("Work in this folder"));
        assert!(frame.contains("elsewhere/"));
        type_text(&mut app, "elsew");
        app.on_key(press(KeyCode::Enter), false);
        let action = app.on_key(press(KeyCode::Enter), false).unwrap();
        assert_eq!(action, Action::OpenFolder(dir.clone()));
        app.perform(action).await;
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tab_cwd(), dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn background_commands_become_jobs() {
        use std::os::unix::process::CommandExt;
        let mut app = app();
        let log = scratch_home().join(format!("job-{}.log", std::process::id()));
        std::fs::write(&log, "compiling\nlistening on :3000\n").unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let input = json!({"command": "npm run dev", "background": true});
        emit(
            &mut app,
            Event::ToolStarted {
                call_id: "j1".into(),
                tool: "bash".into(),
                summary: "npm run dev".into(),
                input,
            },
        );
        emit(
            &mut app,
            Event::ToolFinished {
                call_id: "j1".into(),
                tool: "bash".into(),
                output: format!(
                    "Started in background (pid {}). Output is written to {}",
                    child.id(),
                    log.display()
                ),
                is_error: false,
                millis: 5,
            },
        );
        let frame = render(&mut app, 120, 30);
        let (x, y) = find(&frame, "1 job");
        let action = click(&mut app, x, y, MouseButton::Left).unwrap();
        assert_eq!(action, Action::Jobs);
        app.perform(action).await;
        let frame = render(&mut app, 120, 30);
        println!("{frame}");
        assert!(frame.contains("running"));
        assert!(frame.contains("npm run dev"));
        assert!(frame.contains("listening on :3000"), "the log's tail");
        let action = app.on_key(press(KeyCode::Char('k')), false).unwrap();
        assert_eq!(action, Action::StopJob("j1".into()));
        app.perform(action).await;
        assert_eq!(app.jobs[0].status, crate::tui::jobs::JobStatus::Stopped);
        child.wait().unwrap();
        std::fs::remove_file(log).unwrap();
    }

    #[tokio::test]
    async fn welcome_screen() {
        let mut app = app();
        let frame = render(&mut app, 100, 30);
        println!("{frame}");
        let banner = format!(
            "malac {} · Multi-Agent Loop for Autonomous Coding",
            env!("CARGO_PKG_VERSION")
        );
        assert!(frame.contains(&banner));
        assert!(frame.contains("(  (oo)  )"), "the small pig fits 30 rows");
        assert!(frame.contains("lora/smart"));
        let frame = render(&mut app, 160, 44);
        println!("{frame}");
        assert!(frame.contains("▀"), "the illustrated pig needs room");
        assert!(frame.contains("Describe a task and press Enter"));
    }

    #[tokio::test]
    async fn tool_cards_expand_on_click() {
        let mut app = app();
        a_turn(&mut app);
        let frame = render(&mut app, 90, 24);
        println!("{frame}");
        assert!(frame.contains("› run the tests"));
        assert!(frame.contains("Running the test suite now."));
        assert!(frame.contains("● bash cargo test"));
        assert!(frame.contains("2 lines  2.3s ▸"));
        assert!(frame.contains("lora/smart · 2.3s") || frame.contains("─ lora/smart"));
        let (x, y) = find(&frame, "● bash");
        let action = click(&mut app, x, y, MouseButton::Left).unwrap();
        app.perform(action).await;
        let frame = render(&mut app, 90, 24);
        println!("{frame}");
        assert!(frame.contains("│ $ cargo test"));
        assert!(frame.contains("│ test result: ok. 46 passed"));
        assert!(frame.contains("▾"));
    }

    #[tokio::test]
    async fn permission_dialog_answers() {
        let mut app = app();
        let (reply, answer) = oneshot::channel();
        app.on_approval(Pending {
            request: PermissionRequest {
                id: "r1".into(),
                agent: app.tab().root().to_owned(),
                agent_label: "main".into(),
                tool: "bash".into(),
                summary: "rm -rf target".into(),
                input: json!({"command": "rm -rf target"}),
            },
            reply,
        });
        let frame = render(&mut app, 90, 30);
        println!("{frame}");
        assert!(frame.contains("Permission"));
        assert!(frame.contains("main wants to run bash"));
        assert!(frame.contains("command: rm -rf target"));
        assert!(frame.contains(" Allow  y    Always  a    Deny  n "));
        assert!(frame.contains("? 1 waiting"));
        assert!(
            app.on_key(press(KeyCode::Char('y')), false).is_none(),
            "too soon"
        );
        app.approvals[0].arrived = Instant::now() - Duration::from_secs(1);
        let (x, y) = find(&frame, "Always  a");
        let action = click(&mut app, x, y, MouseButton::Left).unwrap();
        app.perform(action).await;
        assert_eq!(answer.await.unwrap(), Decision::AllowSession);
        assert!(!render(&mut app, 90, 30).contains("Permission"));
    }

    #[tokio::test]
    async fn model_picker_filters() {
        let mut app = app();
        let frame = render(&mut app, 90, 30);
        let (x, y) = find(&frame, "4Model");
        let action = click(&mut app, x, y, MouseButton::Left).unwrap();
        assert_eq!(action, Action::PickModel);
        app.perform(action).await;
        let frame = render(&mut app, 90, 30);
        println!("{frame}");
        assert!(frame.contains(" Model "));
        assert!(frame.contains("● smart"));
        assert!(frame.contains("  fast"));
        app.on_key(press(KeyCode::Char('f')), false);
        let action = app.on_key(press(KeyCode::Enter), false);
        assert_eq!(action, Some(Action::SetModel("lora/fast".into())));
    }

    #[tokio::test]
    async fn menu_bar_and_tabs() {
        let mut app = app();
        a_turn(&mut app);
        app.perform(Action::NewTab).await;
        app.perform(Action::TabMenu).await;
        app.overlay = None;
        let frame = render(&mut app, 100, 26);
        println!("{frame}");
        let header = frame.lines().next().unwrap();
        assert!(header.contains(
            " Session  Model  Mode  Agents  View  Web  Help  │ 1 run the tests ✓  2 new × "
        ));
        let (x, y) = find(&frame, "1 run the tests");
        let action = click(&mut app, x, y, MouseButton::Left).unwrap();
        app.perform(action).await;
        assert_eq!(app.active, 0);

        key(&mut app, KeyCode::F(9)).await;
        key(&mut app, KeyCode::Enter).await;
        let frame = render(&mut app, 100, 26);
        println!("{frame}");
        assert!(frame.contains("New tab"));
        assert!(frame.contains("Rename tab…"));
        key(&mut app, KeyCode::Right).await;
        let frame = render(&mut app, 100, 26);
        assert!(frame.contains("All models…"));
        let action = app.on_key(press(KeyCode::Char('a')), false);
        assert_eq!(action, Some(Action::PickModel));

        let (x, y) = find(&frame, "Mode ");
        app.overlay = None;
        click(&mut app, x, y, MouseButton::Left);
        let frame = render(&mut app, 100, 26);
        println!("{frame}");
        assert!(frame.contains("Autopilot"));
        let (x, y) = find(&frame, "Autopilot");
        let action = click(&mut app, x, y, MouseButton::Left).unwrap();
        assert_eq!(
            action,
            Action::SetMode(malac_core::permission::Mode::Autopilot)
        );
    }

    #[tokio::test]
    async fn context_menu_on_entries() {
        let mut app = app();
        a_turn(&mut app);
        let frame = render(&mut app, 90, 24);
        let (x, y) = find(&frame, "● bash");
        click(&mut app, x + 4, y, MouseButton::Right);
        let frame = render(&mut app, 90, 24);
        println!("{frame}");
        assert!(frame.contains("Copy"));
        assert!(frame.contains("Expand"));
        let action = app.on_key(press(KeyCode::Char('e')), false).unwrap();
        assert!(matches!(action, Action::ToggleEntry(_)));
    }

    #[tokio::test]
    async fn footer_segments_are_clickable() {
        let mut app = app();
        a_turn(&mut app);
        let frame = render(&mut app, 120, 30);
        let status = frame.lines().nth(28).unwrap();
        println!("{status}\n{}", frame.lines().nth(29).unwrap());
        assert!(status.contains("lora/smart │ ask │"));
        assert!(status.contains("│ main │"));
        let (x, y) = find(&frame, "│ ask");
        let action = click(&mut app, x + 2, y, MouseButton::Left).unwrap();
        assert_eq!(action, Action::CycleMode);
        let (x, y) = find(&frame, "10Quit");
        assert_eq!(click(&mut app, x, y, MouseButton::Left), Some(Action::Quit));
    }

    #[tokio::test]
    async fn prompts_queue_while_busy_and_stop_drops_them() {
        let mut app = app();
        app.send("first".into());
        app.send("second".into());
        app.send("third".into());
        assert!(app.busy());
        let queued: Vec<&str> = app.tab().queued.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(queued, ["second", "third"]);
        let frame = render(&mut app, 90, 24);
        assert!(frame.contains("› second  queued"), "{frame}");
        app.perform(Action::Stop).await;
        assert!(app.tab().queued.is_empty());
        assert!(render(&mut app, 90, 24).contains("dropped 2 queued"));
    }

    #[tokio::test]
    async fn update_segment_opens_the_dialog() {
        let mut app = app();
        app.update = malac_core::update::Status {
            current: "0.1.0".into(),
            latest: Some("0.2.0".into()),
            available: true,
            installed: None,
            checked_at: None,
            method: malac_core::update::Method::SelfUpdate,
            release_url: "https://github.com/lobirus/malac/releases/tag/v0.2.0".into(),
            hint: None,
        };
        let frame = render(&mut app, 120, 30);
        let (x, y) = find(&frame, "update 0.2.0");
        let action = click(&mut app, x, y, MouseButton::Left).unwrap();
        assert_eq!(action, Action::UpdateDialog);
        app.perform(action).await;
        let frame = render(&mut app, 120, 30);
        println!("{frame}");
        assert!(frame.contains("latest     0.2.0"));
        assert!(frame.contains(" Install now  i "));
        assert!(frame.contains("releases/tag/v0.2.0"));
    }

    #[tokio::test]
    async fn resuming_shows_the_saved_conversation() {
        let mut app = app();
        let id = uuid::Uuid::now_v7().to_string();
        let records = [
            json!({"kind": "meta", "id": id, "label": "main", "parent": null,
                   "cwd": scratch_home().join("project"), "model": "lora/fast",
                   "created": "2026-10-25T10:00:00Z"}),
            json!({"kind": "message", "message": {"role": "user",
                   "content": [{"type": "text", "text": "fix the parser"}]}}),
            json!({"kind": "message", "message": {"role": "assistant",
                   "content": [{"type": "text", "text": "Fixed the **off-by-one**."}]}}),
        ];
        let lines: Vec<String> = records.iter().map(|r| r.to_string()).collect();
        let dir = malac_core::session::sessions_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.jsonl")), lines.join("\n") + "\n").unwrap();
        app.perform(Action::Resume(id.clone())).await;
        assert_eq!(app.tabs.len(), 1, "the empty tab is reused");
        assert_eq!(app.tab().root(), id);
        let frame = render(&mut app, 90, 24);
        println!("{frame}");
        assert!(frame.contains("› fix the parser"));
        assert!(frame.contains("Fixed the off-by-one."));
        assert!(frame.lines().next().unwrap().contains("1 fix the parser"));
    }

    #[tokio::test]
    async fn slash_completion_runs_the_highlighted_command() {
        let mut app = app();
        render(&mut app, 90, 24);
        for c in "/he".chars() {
            app.on_key(press(KeyCode::Char(c)), false);
        }
        let frame = render(&mut app, 90, 24);
        println!("{frame}");
        assert!(frame.contains("/help"));
        assert_eq!(app.on_key(press(KeyCode::Enter), false), Some(Action::Help));
    }
}
