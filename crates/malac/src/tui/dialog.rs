//! Modal dialogs: the permission prompt, list pickers (models, sessions,
//! agents), help, renaming a tab and installing an update.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use malac_core::permission::{Decision, PermissionRequest};
use malac_core::update::{Method, Status};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use serde_json::Value;

use super::app::{Action, HitMap, Outcome, TUI_COMMANDS, Target};
use super::text::{self, sanitize};
use super::theme::theme;
use crate::repl::{COMMANDS, TAGLINE};

/// A centered box of at most `w` x `h`, drawn with a title; returns the inside.
pub(super) fn frame_box(
    frame: &mut Frame,
    w: u16,
    h: u16,
    title: &str,
    hint: &str,
    hits: &mut HitMap,
) -> Rect {
    let t = theme();
    let screen = frame.area();
    let w = w.min(screen.width.saturating_sub(2)).max(10);
    let h = h.min(screen.height.saturating_sub(2)).max(4);
    let area = Rect::new(
        screen.x + (screen.width - w) / 2,
        screen.y + (screen.height - h) / 2,
        w,
        h,
    );
    frame.render_widget(Clear, area);
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.faint())
        .title(Line::styled(format!(" {title} "), t.bold()));
    if !hint.is_empty() {
        block = block.title_bottom(Line::styled(format!(" {hint} "), t.dim()).right_aligned());
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    hits.push(area, Target::Overlay);
    Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    }
}

pub(super) fn put(frame: &mut Frame, area: Rect, row: u16, line: Line<'static>) {
    if row < area.height {
        let line = text::truncate_line(line, area.width as usize);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x, area.y + row, area.width, 1),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    Model,
    Session,
    Agent,
    Workspace,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub group: String,
    pub label: String,
    pub detail: String,
    pub value: String,
    pub current: bool,
}

enum Shown {
    Header(usize),
    Item(usize),
}

/// A filterable list, grouped under headers.
pub struct Picker {
    pub kind: PickKind,
    pub title: String,
    pub hint: String,
    pub note: String,
    pub filter: String,
    rows: Vec<Row>,
    /// Index into the matching rows.
    selected: usize,
    scroll: usize,
    height: usize,
}

impl Picker {
    pub fn new(kind: PickKind, title: &str, hint: &str, rows: Vec<Row>) -> Self {
        let mut picker = Self {
            kind,
            title: title.to_owned(),
            hint: hint.to_owned(),
            note: String::new(),
            filter: String::new(),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            height: 10,
        };
        picker.set_rows(rows);
        if let Some(current) = picker
            .matching()
            .iter()
            .position(|&i| picker.rows[i].current)
        {
            picker.selected = current;
        }
        picker
    }

    /// Replaces the rows, keeping the selection on the same value.
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        let value = self.chosen().map(|r| r.value.clone());
        self.rows = rows;
        if let Some(value) = value {
            if let Some(i) = self
                .matching()
                .iter()
                .position(|&i| self.rows[i].value == value)
            {
                self.selected = i;
            }
        }
        self.clamp();
    }

    fn matching(&self) -> Vec<usize> {
        let words: Vec<String> = self
            .filter
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                // Session details hold the directory, which is worth searching.
                let detail = if self.kind == PickKind::Session {
                    row.detail.as_str()
                } else {
                    ""
                };
                let haystack =
                    format!("{} {} {detail} {}", row.group, row.label, row.value).to_lowercase();
                words.iter().all(|w| haystack.contains(w.as_str()))
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn shown(&self) -> Vec<Shown> {
        let mut out = Vec::new();
        let mut group = None;
        for (n, &i) in self.matching().iter().enumerate() {
            let row = &self.rows[i];
            if !row.group.is_empty() && group != Some(&row.group) {
                out.push(Shown::Header(i));
                group = Some(&row.group);
            }
            out.push(Shown::Item(n));
        }
        out
    }

    pub fn chosen(&self) -> Option<&Row> {
        self.matching().get(self.selected).map(|&i| &self.rows[i])
    }

    fn clamp(&mut self) {
        let len = self.matching().len();
        self.selected = self.selected.min(len.saturating_sub(1));
    }

    fn select(&mut self, index: usize) {
        self.selected = index;
        self.clamp();
        let shown = self.shown();
        let line = shown
            .iter()
            .position(|s| matches!(s, Shown::Item(n) if *n == self.selected))
            .unwrap_or(0);
        // Keep the group header visible above the first item.
        let top = if line > 0 && matches!(shown[line - 1], Shown::Header(_)) {
            line - 1
        } else {
            line
        };
        if top < self.scroll {
            self.scroll = top;
        } else if line >= self.scroll + self.height {
            self.scroll = line + 1 - self.height;
        }
    }

    fn outcome(&self, action: fn(String) -> Action) -> Outcome {
        match self.chosen() {
            Some(row) => Outcome::Run(action(row.value.clone())),
            None => Outcome::Stay,
        }
    }

    fn choose(&self) -> Outcome {
        self.outcome(match self.kind {
            PickKind::Model => Action::SetModel,
            PickKind::Session => Action::Resume,
            PickKind::Agent => Action::OpenAgent,
            PickKind::Workspace => Action::OpenWorkspace,
        })
    }

    pub fn click(&mut self, index: usize) -> Outcome {
        self.select(index);
        self.choose()
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let len = self.matching().len();
        let index = (self.selected as isize + delta).clamp(0, len.saturating_sub(1) as isize);
        self.select(index as usize);
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Char('c') if ctrl => return Outcome::Close,
            KeyCode::Enter => return self.choose(),
            KeyCode::Char('x') if ctrl && self.kind == PickKind::Agent => {
                return self.outcome(Action::StopAgent);
            }
            KeyCode::Up => self.scroll_by(-1),
            KeyCode::Down => self.scroll_by(1),
            KeyCode::PageUp => self.scroll_by(-(self.height as isize)),
            KeyCode::PageDown => self.scroll_by(self.height as isize),
            KeyCode::Home => self.select(0),
            KeyCode::End => self.select(usize::MAX),
            KeyCode::Backspace => {
                self.filter.pop();
                self.select(0);
            }
            KeyCode::Char('u') if ctrl => {
                self.filter.clear();
                self.select(0);
            }
            KeyCode::Char(c) if !ctrl => {
                self.filter.push(c);
                self.select(0);
            }
            _ => {}
        }
        Outcome::Stay
    }

    pub fn draw(&mut self, frame: &mut Frame, hits: &mut HitMap) {
        let t = theme();
        let wanted = self.shown().len() as u16 + 5;
        let inner = frame_box(frame, 88, wanted.max(12), &self.title, &self.hint, hits);
        let mut filter = vec![Span::styled("› ", t.dim()), Span::raw(self.filter.clone())];
        filter.push(Span::styled("▏", t.fg(t.accent)));
        if !self.note.is_empty() {
            filter.push(Span::styled(format!("   {}", self.note), t.dim()));
        }
        put(frame, inner, 0, Line::from(filter));
        put(
            frame,
            inner,
            1,
            Line::styled("─".repeat(inner.width as usize), t.faint()),
        );
        self.height = inner.height.saturating_sub(2) as usize;
        let matching = self.matching();
        let shown = self.shown();
        if matching.is_empty() {
            put(frame, inner, 2, Line::styled("  nothing matches", t.dim()));
        }
        self.scroll = self.scroll.min(shown.len().saturating_sub(self.height));
        let width = inner.width as usize;
        for (line, item) in shown.iter().skip(self.scroll).take(self.height).enumerate() {
            let y = 2 + line as u16;
            match item {
                Shown::Header(i) => {
                    put(
                        frame,
                        inner,
                        y,
                        Line::styled(self.rows[*i].group.clone(), t.bold().fg(t.dim)),
                    );
                }
                Shown::Item(n) => {
                    let row = &self.rows[matching[*n]];
                    let selected = *n == self.selected;
                    let style = if selected { t.selected() } else { Style::new() };
                    let detail_style = if selected { style } else { t.dim() };
                    let mark = if row.current { "● " } else { "  " };
                    let mark_style = if selected { style } else { t.fg(t.accent) };
                    let label = format!("{mark}{}", row.label);
                    let room = width.saturating_sub(text::width(&label) + 2);
                    let detail = text::truncate(&row.detail, room).into_owned();
                    let gap = width.saturating_sub(text::width(&label) + text::width(&detail) + 1);
                    let line = Line::from(vec![
                        Span::styled(mark, mark_style),
                        Span::styled(row.label.clone(), style),
                        Span::styled(" ".repeat(gap), style),
                        Span::styled(format!("{detail} "), detail_style),
                    ]);
                    put(frame, inner, y, line);
                    hits.push(
                        Rect::new(inner.x, inner.y + y, inner.width, 1),
                        Target::Pick(*n),
                    );
                }
            }
        }
    }
}

/// Keys, mouse and commands.
pub struct Help {
    pub scroll: usize,
}

const KEYS: &[(&str, &str)] = &[
    ("F1  ?", "help (? on an empty prompt)"),
    ("F2  Ctrl-Y", "copy the last answer"),
    (
        "F3",
        "agents as a list or a map (F3 again switches); open, stop",
    ),
    ("F4  Ctrl-P", "pick a model"),
    ("F5  Shift-Tab", "cycle the mode: ask, edits, autopilot"),
    ("F6", "tab menu"),
    ("F7", "open a saved session"),
    ("F8  Esc  Ctrl-C", "stop the running turn and its subagents"),
    (
        "Esc",
        "clear a selection, close the @ picker, leave a subagent",
    ),
    ("F9  Ctrl-O  Alt-M", "menu bar"),
    (
        "F10  Ctrl-D",
        "quit; agents keep working in the daemon (Ctrl-C twice also quits)",
    ),
    (
        "Ctrl-T  Ctrl-W",
        "new tab for this folder, close tab (its sessions stay saved)",
    ),
    ("Alt-1…9", "go to tab"),
    ("Alt-←/→  Ctrl-PgUp/PgDn", "previous or next tab"),
    ("Ctrl-Shift-PgUp/PgDn", "move the tab left or right"),
    ("Alt-↑/↓", "previous or next agent in the tab"),
    (
        "Ctrl-N",
        "start another agent in the tab (on an empty prompt)",
    ),
    ("Ctrl-R", "reconnect to the daemon, starting it if needed"),
    ("Enter", "send; runs the highlighted /command"),
    (
        "@",
        "attach a file from the tab's folder (fuzzy, .gitignore honored)",
    ),
    ("Ctrl-V", "attach the image on the clipboard (macOS)"),
    (
        "Backspace",
        "at the start of the prompt, drop the last attachment",
    ),
    ("Shift-Enter  Alt-Enter  Ctrl-J", "new line"),
    ("↑ ↓", "move between lines, then through history"),
    ("Ctrl-←/→  Alt-B/F", "word left or right"),
    ("Ctrl-A/E  Home/End", "start or end of line"),
    (
        "Ctrl-U/K  Alt-Backspace",
        "delete to line start, line end, word",
    ),
    (
        "Tab",
        "move to the transcript: ↑↓ select, Enter expand or open, y copy",
    ),
    ("[  ]", "in the transcript: previous or next answer"),
    ("PgUp/PgDn", "scroll; Home/End on an empty prompt jump"),
    ("Ctrl-G", "toggle the sidebar"),
    (
        "Ctrl-S",
        "mouse off, to select text with the terminal (again to restore)",
    ),
    ("Ctrl-L", "redraw"),
];

const MOUSE: &[(&str, &str)] = &[
    (
        "click",
        "expand a tool call, open a subagent, switch tabs, use menus and the bars",
    ),
    (
        "drag",
        "select text in the transcript or the prompt; release copies it",
    ),
    ("double / triple click", "select and copy a word / a line"),
    ("right click", "copy, copy code, expand, open, stop"),
    ("middle click", "close a tab"),
    ("wheel", "scroll the transcript, a list or the agent map"),
    (
        "drop a file",
        "attach it: terminals paste the path of a dropped file",
    ),
    (
        "Option-drag  Shift-drag",
        "the terminal's own selection: Option in iTerm2 and Ghostty, Shift in kitty",
    ),
];

impl Help {
    fn lines(width: usize) -> Vec<Line<'static>> {
        let t = theme();
        let mut lines = vec![
            Line::from(vec![
                Span::styled(format!("malac {}", env!("CARGO_PKG_VERSION")), t.bold()),
                Span::styled(format!(" · {TAGLINE}"), t.dim()),
            ]),
            Line::default(),
        ];
        let section = |title: &str| Line::styled(title.to_owned(), t.bold());
        let row = |key: &str, what: &str| {
            let key = format!("  {key:<30} ");
            let room = width.saturating_sub(text::width(&key));
            Line::from(vec![
                Span::styled(key, Style::new()),
                Span::styled(text::truncate(what, room).into_owned(), t.dim()),
            ])
        };
        lines.push(section("Keys"));
        lines.extend(KEYS.iter().map(|(k, w)| row(k, w)));
        lines.push(Line::default());
        lines.push(section("Mouse"));
        lines.extend(MOUSE.iter().map(|(k, w)| row(k, w)));
        lines.push(Line::default());
        lines.push(section("Commands"));
        for (name, args, help) in COMMANDS.iter().chain(TUI_COMMANDS) {
            lines.push(row(format!("/{name} {args}").trim_end(), help));
        }
        lines.push(Line::default());
        lines.push(section("Workspaces"));
        for line in [
            "Tabs are projects: each works in its own folder, and holds one or more agents.",
            "A workspace is a set of tabs. Agents run in the malac daemon, so closing the",
            "terminal leaves them working; `malac` reopens the last workspace where you left",
            "off, `malac -n` starts a new one, `malac -w <name>` opens one, `malac <prompt>`",
            "adds a tab. Several terminals and the web UI can work on one workspace at once.",
        ] {
            lines.push(Line::styled(format!("  {line}"), t.dim()));
        }
        lines.push(Line::default());
        lines.push(Line::styled(
            "In tmux or byobu, `set -g mouse on` passes clicks through.",
            t.dim(),
        ));
        lines
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll += 1,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll += 10,
            KeyCode::Home => self.scroll = 0,
            _ => return Outcome::Close,
        }
        Outcome::Stay
    }

    pub fn draw(&mut self, frame: &mut Frame, hits: &mut HitMap) {
        let inner = frame_box(
            frame,
            96,
            46,
            "Help",
            "↑↓ scroll · any other key closes",
            hits,
        );
        let lines = Self::lines(inner.width as usize);
        self.scroll = self
            .scroll
            .min(lines.len().saturating_sub(inner.height as usize));
        for (y, line) in lines.into_iter().skip(self.scroll).enumerate() {
            put(frame, inner, y as u16, line);
        }
    }
}

/// What a one-line prompt asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    TabName,
    WorkspaceName,
    NewWorkspace,
}

/// A one-line prompt for a name.
pub struct Rename {
    pub ask: Ask,
    pub text: String,
}

impl Rename {
    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => Outcome::Close,
            KeyCode::Enter => Outcome::Run(Action::Named(self.ask, self.text.trim().to_owned())),
            KeyCode::Backspace => {
                self.text.pop();
                Outcome::Stay
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.clear();
                Outcome::Stay
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.push(c);
                Outcome::Stay
            }
            _ => Outcome::Stay,
        }
    }

    pub fn draw(&self, frame: &mut Frame, hits: &mut HitMap) {
        let t = theme();
        let (title, hint) = match self.ask {
            Ask::TabName => ("Rename tab", "enter save · esc cancel"),
            Ask::WorkspaceName => ("Rename workspace", "enter save · esc cancel"),
            Ask::NewWorkspace => ("New workspace", "enter create · esc cancel"),
        };
        let inner = frame_box(frame, 48, 3, title, hint, hits);
        put(
            frame,
            inner,
            0,
            Line::from(vec![
                Span::raw(self.text.clone()),
                Span::styled("▏", t.fg(t.accent)),
            ]),
        );
    }
}

/// Shows what a newer malac would bring and installs it on request.
pub struct UpdateDialog {
    pub installing: bool,
    pub result: Option<Result<String, String>>,
}

impl UpdateDialog {
    pub fn key(&mut self, key: KeyEvent, status: &Status) -> Outcome {
        match key.code {
            KeyCode::Char('i') | KeyCode::Enter if can_install(status) && !self.installing => {
                Outcome::Run(Action::InstallUpdate)
            }
            KeyCode::Char('c') if !self.installing => Outcome::Run(Action::CheckUpdate),
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('l') => Outcome::Close,
            _ => Outcome::Stay,
        }
    }

    pub fn draw(&self, frame: &mut Frame, status: &Status, hits: &mut HitMap) {
        let t = theme();
        let inner = frame_box(frame, 72, 11, "Update", "", hits);
        let latest = status.latest.as_deref().unwrap_or("unknown");
        put(
            frame,
            inner,
            0,
            Line::from(vec![
                Span::styled("installed  ", t.dim()),
                Span::raw(status.current.clone()),
            ]),
        );
        put(
            frame,
            inner,
            1,
            Line::from(vec![
                Span::styled("latest     ", t.dim()),
                Span::raw(latest.to_owned()),
            ]),
        );
        put(
            frame,
            inner,
            2,
            Line::from(vec![
                Span::styled("notes      ", t.dim()),
                Span::styled(
                    status.release_url.clone(),
                    t.fg(t.accent).add_modifier(Modifier::UNDERLINED),
                ),
            ]),
        );
        let message = match (&self.result, self.installing) {
            (_, true) => Line::styled("installing…", t.dim()),
            (Some(Ok(message)), _) => Line::styled(message.clone(), t.fg(t.accent)),
            (Some(Err(err)), _) => Line::styled(err.clone(), t.fg(t.error)),
            (None, _) => match (&status.installed, status.available, &status.hint) {
                (Some(version), _, _) => Line::styled(
                    format!("{version} is installed; restart malac to use it"),
                    t.fg(t.accent),
                ),
                (None, true, Some(hint)) if status.method != Method::SelfUpdate => {
                    Line::styled(format!("update with: {hint}"), t.fg(t.warning))
                }
                (None, true, _) => Line::styled("running agents keep the current version", t.dim()),
                (None, false, _) => Line::styled("malac is up to date", t.dim()),
            },
        };
        put(frame, inner, 4, message);
        let mut buttons = Vec::new();
        if can_install(status) && !self.installing {
            buttons.push((" Install now  i ", Action::InstallUpdate));
        }
        buttons.push((" Check again  c ", Action::CheckUpdate));
        buttons.push((" Later  esc ", Action::CloseDialog));
        draw_buttons(
            frame,
            inner,
            inner.height.saturating_sub(1),
            &buttons,
            0,
            hits,
        );
    }
}

pub fn can_install(status: &Status) -> bool {
    status.available && status.installed.is_none() && status.method == Method::SelfUpdate
}

pub(super) fn draw_buttons(
    frame: &mut Frame,
    inner: Rect,
    row: u16,
    buttons: &[(&str, Action)],
    focus: usize,
    hits: &mut HitMap,
) {
    let t = theme();
    let mut x = inner.x;
    for (index, (label, action)) in buttons.iter().enumerate() {
        let width = text::width(label) as u16;
        if x + width > inner.right() || row >= inner.height {
            break;
        }
        let style = if index == focus {
            t.selected()
        } else {
            Style::new().bg(t.selection_bg)
        };
        let area = Rect::new(x, inner.y + row, width, 1);
        frame.render_widget(Paragraph::new(Line::styled(label.to_string(), style)), area);
        hits.push(area, Target::Do(action.clone()));
        x += width + 2;
    }
}

/// Asks before something that cannot be undone or interrupts work.
pub struct Confirm {
    pub title: String,
    pub message: String,
    /// The button that goes ahead, such as " Close it  y ".
    pub label: &'static str,
    pub yes: Action,
}

impl Confirm {
    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => Outcome::Run(self.yes.clone()),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Outcome::Close,
            _ => Outcome::Stay,
        }
    }

    pub fn draw(&self, frame: &mut Frame, hits: &mut HitMap) {
        let width = (text::width(&self.message) as u16 + 6).clamp(40, 80);
        let inner = frame_box(frame, width, 6, &self.title, "", hits);
        put(frame, inner, 0, Line::raw(self.message.clone()));
        let buttons = [
            (self.label, self.yes.clone()),
            (" Cancel  n ", Action::CloseDialog),
        ];
        draw_buttons(frame, inner, inner.height - 1, &buttons, 0, hits);
    }
}

/// A permission request waiting for an answer, from this client or another.
pub struct Approval {
    pub request: PermissionRequest,
    pub arrived: Instant,
    pub details: bool,
    pub focus: usize,
}

pub const ANSWERS: [(&str, Decision); 3] = [
    (" Allow  y ", Decision::Allow),
    (" Always  a ", Decision::AllowSession),
    (" Deny  n ", Decision::Deny),
];

impl Approval {
    pub fn new(request: PermissionRequest) -> Self {
        Self {
            request,
            arrived: Instant::now(),
            details: false,
            focus: 0,
        }
    }

    fn input_lines(&self, width: usize) -> Vec<Line<'static>> {
        let t = theme();
        let mut lines = Vec::new();
        let input = &self.request.input;
        let entries: Vec<(String, String)> = match input.as_object() {
            Some(map) => map
                .iter()
                .map(|(k, v)| {
                    let value = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), value)
                })
                .collect(),
            None => vec![(String::new(), input.to_string())],
        };
        for (key, value) in entries {
            let value = sanitize(&value).into_owned();
            let prefix = if key.is_empty() {
                String::new()
            } else {
                format!("{key}: ")
            };
            for (i, source) in value.split('\n').enumerate() {
                let lead = if i == 0 {
                    prefix.clone()
                } else {
                    " ".repeat(prefix.len())
                };
                for row in text::hard_wrap(&format!("{lead}{source}"), width) {
                    lines.push(Line::styled(row, t.dim()));
                }
            }
        }
        lines
    }

    pub fn draw(&self, frame: &mut Frame, position: (usize, usize), hits: &mut HitMap) {
        let t = theme();
        let screen = frame.area();
        let width = screen.width.saturating_sub(4).min(100);
        let body = width.saturating_sub(4) as usize;
        let input = self.input_lines(body);
        let shown = if self.details {
            input.len().min(screen.height.saturating_sub(12) as usize)
        } else {
            input.len().min(6)
        };
        let title = if position.1 > 1 {
            format!("Permission {} of {}", position.0 + 1, position.1)
        } else {
            "Permission".to_owned()
        };
        let hint = if input.len() > 6 {
            "d details · h hide"
        } else {
            "h hide"
        };
        let height = shown as u16 + 7;
        let inner = frame_box(frame, width, height, &title, hint, hits);
        let who = Line::from(vec![
            Span::styled(self.request.agent_label.clone(), t.bold()),
            Span::styled(" wants to run ", t.dim()),
            Span::styled(self.request.tool.clone(), t.bold().fg(t.warning)),
        ]);
        put(frame, inner, 0, who);
        let summary = sanitize(&self.request.summary).into_owned();
        put(frame, inner, 1, Line::raw(summary));
        for (i, line) in input.iter().take(shown).enumerate() {
            put(frame, inner, 3 + i as u16, line.clone());
        }
        if input.len() > shown {
            let more = format!("… {} more lines", input.len() - shown);
            put(
                frame,
                inner,
                3 + shown as u16,
                Line::styled(more, t.faint()),
            );
        }
        let buttons: Vec<(&str, Action)> = ANSWERS
            .iter()
            .map(|(label, decision)| (*label, Action::Answer(*decision)))
            .collect();
        draw_buttons(frame, inner, inner.height - 1, &buttons, self.focus, hits);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(group: &str, label: &str, current: bool) -> Row {
        Row {
            group: group.into(),
            label: label.into(),
            detail: String::new(),
            value: format!("{group}/{label}"),
            current,
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn picker_starts_on_the_current_row_and_filters() {
        let rows = vec![
            row("lora", "smart", false),
            row("lora", "fast", true),
            row("openai", "gpt-5", false),
        ];
        let mut picker = Picker::new(PickKind::Model, "Model", "", rows);
        assert_eq!(picker.chosen().unwrap().value, "lora/fast");
        for c in "gpt".chars() {
            picker.key(press(KeyCode::Char(c)));
        }
        assert_eq!(picker.chosen().unwrap().value, "openai/gpt-5");
        assert_eq!(
            picker.key(press(KeyCode::Enter)),
            Outcome::Run(Action::SetModel("openai/gpt-5".into()))
        );
        picker.key(press(KeyCode::Backspace));
        picker.key(press(KeyCode::Backspace));
        picker.key(press(KeyCode::Backspace));
        picker.key(press(KeyCode::Down));
        assert_eq!(picker.chosen().unwrap().value, "lora/fast");
    }

    #[test]
    fn new_rows_keep_the_selection() {
        let mut picker = Picker::new(
            PickKind::Model,
            "Model",
            "",
            vec![row("lora", "smart", false), row("lora", "fast", true)],
        );
        picker.set_rows(vec![
            row("lora", "smart", false),
            row("lora", "big", false),
            row("lora", "fast", true),
        ]);
        assert_eq!(picker.chosen().unwrap().value, "lora/fast");
    }
}
