//! Draws a frame: header, transcript (or the welcome screen), sidebar,
//! prompt, status line, function keys and whatever floats above them.
//! Every clickable area is recorded in the app's hit map.

use std::path::Path;
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
    let right = match app.connection_name() {
        Some(name) => format!("{name} › {}", app.workspace_name),
        None => app.workspace_name.clone(),
    };
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
    let tab = app.tab();
    let shown = app.shown();
    let context = shown.filter(|v| v.context > 0).map(|v| {
        let window = app.config.model_config(&v.model).context_window;
        (v.context, window)
    });
    let tree = app.tree(tab);
    let (mut used, mut cost) = (0, 0.0);
    for (_, view) in &tree {
        let u = app.snapshots.get(&view.id).map_or(view.total, |s| s.usage);
        used += u.input_tokens + u.cache_read_tokens + u.cache_write_tokens + u.output_tokens;
        cost += u.cost;
    }
    let urls = app.backend.web_urls();
    let web = (!urls.is_empty()).then_some(app.daemon.remote);
    bars::Status {
        left: app.activity(),
        model: app.current_model(),
        mode: tab.mode,
        context,
        tokens: used,
        cost,
        running: app.running_count(),
        subagents: tree.len().saturating_sub(1),
        jobs: app.running_jobs(),
        spinner: app.spinner(),
        approvals: app.approvals.len(),
        web,
        update: app.update_segment(),
        daemon: app.daemon_segment(),
        control: app.control_issue(),
        connection: app.connection_name().map(str::to_owned),
        awake: app.awake.awake,
        branch: app.branch.clone(),
        clock: app.clock().to_owned(),
    }
}

/// The tab's top-level agents, when it has more than one: click to show one.
fn draw_agent_strip(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let tab = app.tab();
    let mut x = area.x + 1;
    let mut spans = vec![Span::raw(" ")];
    let mut spots = Vec::new();
    for agent in tab.agents.clone() {
        let running = app.agents.tree(&agent).iter().any(|(_, v)| {
            app.snapshots
                .get(&v.id)
                .is_some_and(|s| s.status == malac_core::AgentStatus::Running)
        });
        let waiting = app
            .approvals
            .iter()
            .any(|a| app.agents.root_of(&a.request.agent) == agent || a.request.agent == agent);
        let (mark, mark_style) = match (waiting, running) {
            (true, _) => ("?", t.fg(t.warning)),
            (_, true) => (app.spinner(), t.fg(t.accent)),
            _ => ("●", t.faint()),
        };
        let label = format!(" {} ", text::truncate(&app.agent_label(&agent), 20));
        let shown = app.tab().agent.as_deref() == Some(agent.as_str());
        let style = if shown { t.selected() } else { t.dim() };
        let width = text::width(&label) as u16 + 2;
        spans.push(Span::styled(format!("{mark} "), mark_style));
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        spots.push((x, width, Action::ShowTabAgent(agent)));
        x += width + 1;
    }
    let new = " + agent ";
    spans.push(Span::styled(new, t.faint()));
    spots.push((x, text::width(new) as u16, Action::NewAgent));
    let line = text::truncate_line(Line::from(spans), area.width as usize);
    frame.render_widget(Paragraph::new(line), area);
    for (x, width, action) in spots {
        if x < area.right() {
            app.hits.push(
                Rect::new(x, area.y, width.min(area.right() - x), 1),
                Target::Do(action),
            );
        }
    }
}

/// A calm line offering the repository the tab's folder is part of.
fn draw_suggestion(frame: &mut Frame, app: &mut App, area: Rect, root: &Path) {
    let t = theme();
    let lead = format!(" This folder is part of {}. ", short_path(root));
    let open = "Open the whole repository?";
    let keys = "   /root · ";
    let hide = "Esc hides";
    let line = Line::from(vec![
        Span::styled(lead.clone(), t.dim()),
        Span::styled(open, t.fg(t.accent)),
        Span::styled(keys, t.faint()),
        Span::styled(hide, t.faint()),
    ]);
    let line = text::truncate_line(line, area.width as usize);
    frame.render_widget(Paragraph::new(line), area);
    let open_x = area.x + text::width(&lead) as u16;
    let hide_x = open_x + text::width(open) as u16 + text::width(keys) as u16;
    for (x, width, action) in [
        (open_x, open, Action::OpenFolder(root.to_owned())),
        (hide_x, hide, Action::HideSuggestion),
    ] {
        if x < area.right() {
            let width = (text::width(width) as u16).min(area.right() - x);
            app.hits
                .push(Rect::new(x, area.y, width, 1), Target::Do(action));
        }
    }
}

fn draw_transcript(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let mut area = area;
    let shown = app.tab().shown().map(str::to_owned).unwrap_or_default();
    if let Some(root) = app.suggested_root().map(Path::to_path_buf) {
        draw_suggestion(frame, app, Rect { height: 1, ..area }, &root);
        area.y += 1;
        area.height = area.height.saturating_sub(1);
    }
    if app.tab().agents.len() > 1 {
        draw_agent_strip(frame, app, Rect { height: 1, ..area });
        area.y += 1;
        area.height = area.height.saturating_sub(1);
    }
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
    let mut ids: Vec<String> = tab.root().map(str::to_owned).into_iter().collect();
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
    let model = app.current_model();
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
    lines.push((
        field("space", app.workspace_name.clone(), Style::new()),
        Some(Action::Workspaces),
    ));
    lines.push((field("model", model, Style::new()), Some(Action::PickModel)));
    let mode = app.tab().mode;
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
    if let Some(url) = app.backend.web_urls().into_iter().next() {
        lines.push((
            field(
                "web",
                text::hide_token(&url),
                t.fg(t.accent).add_modifier(Modifier::UNDERLINED),
            ),
            Some(Action::CopyWebUrl),
        ));
    }
    if let Some(notice) = app.daemon_segment() {
        lines.push((
            Line::styled(notice, t.fg(t.warning)),
            Some(Action::RestartDaemon),
        ));
    } else if let Some(notice) = update_notice(&app.update) {
        lines.push((
            Line::styled(notice, t.fg(t.accent)),
            Some(Action::UpdateDialog),
        ));
    }
    lines.push((Line::default(), None));
    for tip in [
        "Describe a task and press Enter; Shift+Enter or Ctrl+J adds a line.",
        "F4 picks a model, F5 cycles the mode, F9 opens the menus.",
        "/ lists commands, F1 shows every key, /cd opens a folder in a tab.",
        "@ attaches a file, Ctrl+V an image; drag over text to copy it.",
        "Agents keep working in the daemon when you quit; `malac` comes back here.",
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
    let shown = app.tab().shown().map(str::to_owned).unwrap_or_default();
    let roots = app.tab().agents.clone();
    for (depth, view) in roots.iter().flat_map(|r| app.agents.tree(r)) {
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
    for (_, view) in roots.iter().flat_map(|r| app.agents.tree(r)) {
        total.add(&app.snapshots.get(&view.id).map_or(view.total, |s| s.usage));
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

    lines.push((Line::default(), None));
    lines.push(heading("Workspace"));
    lines.push((
        Line::raw(app.workspace_name.clone()),
        Some(Action::Workspaces),
    ));
    let tabs = match app.tabs.len() {
        1 => "1 tab".to_owned(),
        n => format!("{n} tabs"),
    };
    lines.push((
        Line::styled(format!("{tabs} · daemon {}", app.daemon.version), t.dim()),
        Some(Action::Workspaces),
    ));
    if let Some(url) = app.backend.web_urls().first() {
        lines.push((Line::default(), None));
        lines.push(heading("Web UI"));
        lines.push((
            Line::styled(
                text::truncate(&text::hide_token(url), width).into_owned(),
                t.fg(t.accent),
            ),
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
    let mut border = if focused { t.fg(t.accent) } else { t.faint() };
    let dictate = match crate::voice::ready(&app.voice.dictation) && app.talk.is_none() {
        true => "· ^X dictate ",
        false => "",
    };
    let hint = match app.dictation_hint() {
        Some((recording, text)) => {
            let mut spans = vec![Span::raw(" ")];
            if recording {
                border = t.fg(t.error);
                spans.push(Span::styled("● ", t.fg(t.error)));
            }
            spans.push(Span::styled(format!("{text} "), t.dim()));
            Line::from(spans)
        }
        None if app.busy() => Line::styled(" enter queues · esc stops ", t.faint()),
        None if !app.attachments.is_empty() => Line::styled(
            format!(" enter send · ⌫ at the start drops the last file {dictate}"),
            t.faint(),
        ),
        None => Line::styled(
            format!(" enter send · @ attach a file {dictate}"),
            t.faint(),
        ),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title_bottom(hint.right_aligned());
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
    for (row, (index, completion)) in completions
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
        let quiet = |faint: bool| match (index == selected, faint) {
            (true, _) => style,
            (false, true) => t.faint(),
            (false, false) => t.dim(),
        };
        let name = format!(" /{}", completion.name);
        let args = match completion.args.as_str() {
            "" => String::new(),
            args => format!(" {args}"),
        };
        let gap = 26usize.saturating_sub(text::width(&name) + text::width(&args));
        let mut spans = vec![
            Span::styled(name, style),
            Span::styled(args, quiet(completion.source.is_some())),
            Span::styled(" ".repeat(gap), style),
            Span::styled(format!("{} ", completion.help), quiet(false)),
        ];
        if let Some(source) = &completion.source {
            spans.push(Span::styled(format!("· {source} "), quiet(true)));
        }
        let line = Line::from(spans);
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
    let jobs = app.workspace_jobs();
    let hits = &mut app.hits;
    match &mut app.overlay {
        Some(Overlay::Menu(menu)) => menu::draw(frame, menu, anchor, hits),
        Some(
            Overlay::Models(p) | Overlay::Sessions(p) | Overlay::Agents(p) | Overlay::Workspaces(p),
        ) => p.draw(frame, hits),
        Some(Overlay::AgentMap(map)) => map.draw(frame, hits),
        Some(Overlay::Folder(browser)) => browser.draw(frame, hits),
        Some(Overlay::Jobs(view)) => view.draw(frame, &jobs, hits),
        Some(Overlay::Help(help)) => help.draw(frame, hits),
        Some(Overlay::Rename(rename)) => rename.draw(frame, hits),
        Some(Overlay::Confirm(confirm)) => confirm.draw(frame, hits),
        Some(Overlay::Update(dialog)) => dialog.draw(frame, &app.update, hits),
        Some(Overlay::Remote(dialog)) => {
            let shown = super::remote::Shown {
                connection: app.backend.endpoint().label(),
                url: app
                    .backend
                    .file()
                    .map(|f| f.url.clone())
                    .unwrap_or_default(),
                version: &app.daemon.version,
                remote: &app.remote,
                awake: &app.awake,
            };
            dialog.draw(frame, &shown, hits);
        }
        None => {}
    }
}
