//! The application: tabs over agents, the prompt editor, menus and dialogs,
//! and what every key, click and action does.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use malac_core::config;
use malac_core::event::{AgentId, Envelope, Event, Level, TurnStatus};
use malac_core::permission::{Decision, Mode};
use malac_core::provider::ProviderModels;
use malac_core::session::{self, SessionSummary};
use malac_core::update::{self, Status};
use malac_core::{AgentHandle, AgentSnapshot, AgentSpec, AgentStatus, Block};
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use tokio::sync::mpsc;

use super::Pending;
use super::agentmap::{AgentMap, Node, State};
use super::attach::{self, Attachment};
use super::bars::{self, Mark};
use super::dialog::{self, Approval, Help, PickKind, Picker, Rename, Row, UpdateDialog};
use super::editor::{self, Editor};
use super::files;
use super::folder::FolderBrowser;
use super::jobs::{self, Job, JobStatus, JobsView};
use super::menu::{Item, Menu, Place};
use super::select::{self, Point, Selection};
use super::term;
use super::text;
use super::theme::theme;
use super::transcript::{AgentView, Agents, Entry, SPINNER, duration};
use crate::render::tokens;
use crate::repl::{COMMANDS, Session, Slash};

mod actions;
mod backend;
mod input;
mod mouse;

pub use actions::Action;

/// Commands only the full-screen interface has, completed with the rest.
pub const TUI_COMMANDS: &[(&str, &str, &str)] = &[
    ("cd", "[dir]", "work in another folder, in a new tab"),
    ("jobs", "", "background processes the agents started"),
];

/// What a menu or dialog wants after a key or click.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Stay,
    Close,
    Run(Action),
    /// Show another menu bar title.
    Switch(usize),
}

/// What sits under a screen position.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Do(Action),
    MenuTitle(usize),
    MenuItem(usize),
    Tab(usize),
    CloseTab(usize),
    /// A transcript entry and the row within it.
    Entry {
        index: usize,
        line: usize,
    },
    /// The transcript pane, under its entries.
    Transcript,
    Completion(usize),
    /// A row of the `@` file picker.
    FileMatch(usize),
    /// An attachment chip above the prompt; clicking removes it.
    Chip(usize),
    Input,
    Pick(usize),
    /// Inside a menu or dialog, on nothing in particular.
    Overlay,
}

/// Clickable areas of the last frame; later entries are on top.
#[derive(Default)]
pub struct HitMap(Vec<(Rect, Target)>);

impl HitMap {
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn push(&mut self, area: Rect, target: Target) {
        self.0.push((area, target));
    }

    pub fn at(&self, x: u16, y: u16) -> Option<&Target> {
        self.0
            .iter()
            .rev()
            .find(|(area, _)| area.contains(Position { x, y }))
            .map(|(_, target)| target)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Transcript,
}

/// One tab: a session, or a subagent pinned to its own tab.
pub struct Tab {
    pub handle: AgentHandle,
    pub title: Option<String>,
    /// Subagents opened inside this tab, innermost last.
    pub view: Vec<AgentId>,
    /// Top row while scrolled up; `None` follows new output.
    pub scroll: Option<usize>,
    /// Transcript height when the user scrolled away, to notice new output.
    pub seen: usize,
    pub selected: Option<usize>,
    pub draft: String,
    pub draft_attachments: Vec<Attachment>,
    /// Prompts typed while the agent was busy, sent one by one as it frees up.
    pub queued: VecDeque<Prompt>,
}

/// A prompt on its way: what was typed and what was attached.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prompt {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

impl From<String> for Prompt {
    fn from(text: String) -> Self {
        Self {
            text,
            attachments: Vec::new(),
        }
    }
}

impl From<&str> for Prompt {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

impl Tab {
    fn new(handle: AgentHandle) -> Self {
        Self {
            handle,
            title: None,
            view: Vec::new(),
            scroll: None,
            seen: 0,
            selected: None,
            draft: String::new(),
            draft_attachments: Vec::new(),
            queued: VecDeque::new(),
        }
    }

    pub fn root(&self) -> &str {
        &self.handle.id
    }

    pub fn shown(&self) -> &str {
        self.view.last().map_or(self.root(), String::as_str)
    }

    /// Pinned subagents are watched, not prompted.
    pub fn read_only(&self) -> bool {
        self.handle.info().parent.is_some()
    }

    fn reset_view(&mut self) {
        self.view.clear();
        self.scroll = None;
        self.selected = None;
    }
}

pub enum Overlay {
    Menu(Menu),
    Models(Picker),
    Sessions(Picker),
    Agents(Picker),
    AgentMap(AgentMap),
    Folder(FolderBrowser),
    Jobs(JobsView),
    Help(Help),
    Rename(Rename),
    Update(UpdateDialog),
}

/// Results of work done in the background.
pub enum Update {
    Models(Vec<ProviderModels>),
    Checked(Result<Status, String>),
    Installed(Result<Status, String>),
    Files {
        root: PathBuf,
        list: Vec<String>,
    },
    /// The clipboard's image, saved to a file.
    Pasted(Result<PathBuf, String>),
}

/// Where things were drawn in the last frame, for keys that depend on layout.
#[derive(Default)]
pub struct Geometry {
    pub transcript: Rect,
    /// Where the prompt's text starts, below the attachment chips.
    pub input: Rect,
    /// The first wrapped row of the prompt shown, when it scrolls.
    pub input_first: usize,
    pub titles: Vec<u16>,
    pub tabs_x: u16,
    pub sidebar: bool,
}

/// A message in the status line for a few seconds.
pub struct Flash {
    pub text: String,
    /// Confirms something worked, rather than warning.
    pub ok: bool,
    pub at: Instant,
}

/// What the left button is doing between press and release.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drag {
    /// Pressed on the transcript; a click unless it moves.
    Transcript {
        start: Point,
        entry: Option<usize>,
        moved: bool,
    },
    /// Selecting in the prompt from a byte offset.
    Input { anchor: usize },
}

/// Recent presses, to tell double and triple clicks.
#[derive(Debug, Clone, Copy)]
struct Clicks {
    at: Instant,
    x: u16,
    y: u16,
    count: u8,
}

/// Files under the tab's folder for the `@` picker; `None` while indexing.
#[derive(Default)]
pub struct FileIndex {
    pub root: PathBuf,
    pub list: Option<Vec<String>>,
    pub loading: bool,
}

/// Which agents view F3 opens: the last one used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentsView {
    List,
    Map,
}

pub struct App {
    pub session: Session,
    pub agents: Agents,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub editor: Editor,
    pub focus: Focus,
    pub completion: usize,
    pub completion_dismissed: bool,
    pub approvals: VecDeque<Approval>,
    pub approvals_hidden: bool,
    pub overlay: Option<Overlay>,
    /// The user's choice; `None` shows the sidebar on wide terminals.
    pub sidebar: Option<bool>,
    pub mouse_capture: bool,
    pub models: Vec<ProviderModels>,
    pub discovering: bool,
    pub update: Status,
    pub hits: HitMap,
    pub geometry: Geometry,
    pub snapshots: HashMap<AgentId, AgentSnapshot>,
    pub branch: Option<String>,
    pub recent: Vec<SessionSummary>,
    pub flash: Option<Flash>,
    /// Text selected in the transcript with the mouse.
    pub selection: Option<Selection>,
    pub drag: Option<Drag>,
    /// The pointer during a drag, for scrolling while it is past an edge.
    pointer: (u16, u16),
    clicks: Option<Clicks>,
    /// Files and images attached to the prompt being written.
    pub attachments: Vec<Attachment>,
    pub files: FileIndex,
    /// The highlighted row of the `@` picker.
    pub file_choice: usize,
    /// Where the `@` was when the picker was dismissed with Esc.
    file_dismissed: Option<usize>,
    pub jobs: Vec<Job>,
    jobs_polled: Instant,
    pub agents_view: AgentsView,
    pub started: Instant,
    pub dirty: bool,
    pub clear_screen: bool,
    pub quit: bool,
    quit_armed: Option<Instant>,
    clock: String,
    branch_read: Instant,
    history_path: PathBuf,
    tx: mpsc::UnboundedSender<Update>,
}

/// Approval keys are ignored this long after a prompt appears, so typing
/// that was meant for the editor cannot answer it.
const ANSWER_GUARD: Duration = Duration::from_millis(300);
const DOUBLE_PRESS: Duration = Duration::from_millis(1200);

impl App {
    pub fn new(session: Session, tx: mpsc::UnboundedSender<Update>) -> Self {
        let history_path = config::home_dir().join("history.txt");
        let runtime = session.runtime.clone();
        let mut agents = Agents::default();
        agents.show_thinking = runtime.config.ui.show_thinking;
        let first = session.handle.clone();
        let mut app = Self {
            agents,
            tabs: Vec::new(),
            active: 0,
            editor: Editor::new(editor::load_history(&history_path)),
            focus: Focus::Input,
            completion: 0,
            completion_dismissed: false,
            approvals: VecDeque::new(),
            approvals_hidden: false,
            overlay: None,
            sidebar: None,
            mouse_capture: true,
            models: Vec::new(),
            discovering: false,
            update: update::status(),
            hits: HitMap::default(),
            geometry: Geometry::default(),
            snapshots: HashMap::new(),
            branch: git_branch(&runtime.cwd),
            recent: Vec::new(),
            flash: None,
            selection: None,
            drag: None,
            pointer: (0, 0),
            clicks: None,
            attachments: Vec::new(),
            files: FileIndex::default(),
            file_choice: 0,
            file_dismissed: None,
            jobs: Vec::new(),
            jobs_polled: Instant::now(),
            agents_view: AgentsView::List,
            started: Instant::now(),
            dirty: true,
            clear_screen: false,
            quit: false,
            quit_armed: None,
            clock: String::new(),
            branch_read: Instant::now(),
            history_path,
            tx,
            session,
        };
        app.open_tab(first);
        app.recent = session::list(false, 40)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.meta.cwd == runtime.cwd && s.meta.id != app.tabs[0].root())
            .filter(|s| !s.title.is_empty())
            .take(5)
            .collect();
        for server in runtime.servers() {
            if let Some(err) = server.error {
                let first = err.lines().next().unwrap_or_default().to_owned();
                app.notice(
                    Level::Warning,
                    format!("MCP server `{}` unavailable: {first}", server.name),
                );
            }
        }
        app.refresh_snapshots();
        app
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    pub fn shown(&self) -> Option<&AgentView> {
        self.agents.get(self.tab().shown())
    }

    pub fn refresh_snapshots(&mut self) {
        self.snapshots = self
            .session
            .runtime
            .agents()
            .into_iter()
            .map(|s| (s.info.id.clone(), s))
            .collect();
    }

    pub fn spinner(&self) -> &'static str {
        let tick = self.started.elapsed().as_millis() as usize / 80;
        SPINNER[tick % SPINNER.len()]
    }

    fn is_running(&self, id: &str) -> bool {
        self.snapshots
            .get(id)
            .is_some_and(|s| s.status == AgentStatus::Running)
    }

    /// The active tab's agents are working, or have work queued.
    pub fn busy(&self) -> bool {
        let tab = self.tab();
        tab.handle.is_busy()
            || self
                .agents
                .tree(tab.root())
                .iter()
                .any(|(_, v)| self.is_running(&v.id))
    }

    pub fn running_count(&self) -> usize {
        self.snapshots
            .values()
            .filter(|s| s.status == AgentStatus::Running)
            .count()
    }

    pub fn approval_visible(&self) -> bool {
        !self.approvals.is_empty() && !self.approvals_hidden
    }

    pub fn flash(&mut self, message: impl Into<String>) {
        self.flash = Some(Flash {
            text: message.into(),
            ok: false,
            at: Instant::now(),
        });
    }

    /// A flash confirming something worked.
    pub fn flash_ok(&mut self, message: impl Into<String>) {
        self.flash = Some(Flash {
            text: message.into(),
            ok: true,
            at: Instant::now(),
        });
    }

    /// Adds a line to the transcript on screen.
    pub fn notice(&mut self, level: Level, message: impl Into<String>) {
        self.push_entry(Entry::Notice(level, message.into()));
    }

    fn info(&mut self, lines: Vec<Line<'static>>) {
        self.push_entry(Entry::Info(lines));
        self.tab_mut().scroll = None;
    }

    fn push_entry(&mut self, entry: Entry) {
        let id = self.tab().shown().to_owned();
        if let Some(view) = self.agents.get_mut(&id) {
            view.transcript.push(entry);
        }
    }

    /// Makes sure the agent has a view, filled from its saved session.
    fn ensure_view(&mut self, handle: &AgentHandle) {
        if self.agents.get(&handle.id).is_some() {
            return;
        }
        let snapshot = handle.snapshot();
        let mut view = AgentView::new(
            &handle.id,
            &snapshot.info.label,
            snapshot.info.parent.clone(),
            &snapshot.model,
            &snapshot.info.cwd.display().to_string(),
        );
        if let Ok(saved) = session::load(&handle.id) {
            let tools = self.session.runtime.tools();
            view.transcript.load(&saved.history, |name, input| {
                tools
                    .iter()
                    .find(|t| t.name() == name)
                    .map(|t| t.summary(input))
                    .unwrap_or_else(|| malac_core::tools::summarize_arguments(input))
            });
            view.total = saved.usage;
        }
        self.agents.insert(view);
    }

    fn open_tab(&mut self, handle: AgentHandle) -> usize {
        self.ensure_view(&handle);
        self.tabs.push(Tab::new(handle));
        self.tabs.len() - 1
    }

    fn switch_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if index != self.active {
            self.stash_draft();
            self.active = index;
            self.restore_draft();
        }
        self.focus = Focus::Input;
        self.selection = None;
        self.branch = git_branch(&self.tab_cwd());
        self.session.switch_to(self.tabs[index].handle.clone());
    }

    /// Keeps what is being written with the active tab.
    fn stash_draft(&mut self) {
        let draft = self.editor.text().to_owned();
        let attachments = std::mem::take(&mut self.attachments);
        let tab = &mut self.tabs[self.active];
        tab.draft = draft;
        tab.draft_attachments = attachments;
    }

    fn restore_draft(&mut self) {
        let tab = &mut self.tabs[self.active];
        let draft = std::mem::take(&mut tab.draft);
        self.attachments = std::mem::take(&mut tab.draft_attachments);
        self.editor.set_text(&draft);
    }

    pub fn tab_label(&self, index: usize) -> String {
        let tab = &self.tabs[index];
        if let Some(title) = &tab.title {
            return title.clone();
        }
        let view = self.agents.get(tab.root());
        if tab.read_only() {
            return view.map_or_else(|| "agent".to_owned(), |v| v.label.clone());
        }
        view.and_then(|v| {
            v.transcript.items().iter().find_map(|i| match &i.entry {
                Entry::User(text) => {
                    let (prompt, mentions) = attach::split_user_text(text);
                    let first = text::first_line(&prompt).to_owned();
                    match mentions.first() {
                        Some(mention) if first.is_empty() => Some(mention.name.clone()),
                        _ => Some(first),
                    }
                }
                _ => None,
            })
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "new".to_owned())
    }

    pub fn tab_mark(&self, index: usize) -> Mark {
        let tab = &self.tabs[index];
        let tree = self.agents.tree(tab.root());
        let ids: Vec<&str> = tree.iter().map(|(_, v)| v.id.as_str()).collect();
        if self
            .approvals
            .iter()
            .any(|a| ids.contains(&a.request.agent.as_str()))
        {
            return Mark::Waiting;
        }
        if tab.handle.is_busy() || ids.iter().any(|id| self.is_running(id)) {
            return Mark::Running(self.spinner());
        }
        match self.agents.get(tab.root()).and_then(|v| v.last_status) {
            Some(TurnStatus::Completed) => Mark::Done,
            Some(TurnStatus::Failed) => Mark::Failed,
            _ => Mark::Idle,
        }
    }

    /// Slash commands matching what is typed, while the command name is typed.
    pub fn completions(&self) -> Vec<&'static (&'static str, &'static str, &'static str)> {
        if self.completion_dismissed || self.focus != Focus::Input {
            return Vec::new();
        }
        completions(self.editor.text())
    }

    /// The `@` being typed before the cursor and the query after it.
    pub fn file_query(&self) -> Option<(usize, &str)> {
        if self.focus != Focus::Input || self.tab().read_only() {
            return None;
        }
        let text = &self.editor.text()[..self.editor.cursor_offset()];
        let at = text.rfind('@')?;
        let query = &text[at + 1..];
        let starts_word = text[..at]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        let open = starts_word && !query.contains(char::is_whitespace);
        (open && self.file_dismissed != Some(at)).then_some((at, query))
    }

    /// Files matching the `@` query, best first.
    pub fn file_matches(&self) -> Vec<files::Match> {
        match (self.file_query(), &self.files.list) {
            (Some((_, query)), Some(list)) => files::best(list, query, 50),
            _ => Vec::new(),
        }
    }

    /// Attaches a file; refusals are flashed.
    pub fn attach(&mut self, path: &Path) -> bool {
        match Attachment::open(path, &self.tab_cwd()) {
            Ok(attachment) => {
                if !self.attachments.iter().any(|a| a.path == attachment.path) {
                    self.attachments.push(attachment);
                }
                true
            }
            Err(err) => {
                self.flash(err);
                false
            }
        }
    }

    pub fn on_bus(&mut self, envelope: &Envelope) {
        match &envelope.event {
            Event::Update { status } => {
                self.update = status.clone();
                return;
            }
            Event::PermissionResolved { request_id, .. } => {
                self.approvals.retain(|a| a.request.id != *request_id);
            }
            _ => {}
        }
        self.agents.apply(envelope);
        match &envelope.event {
            Event::TurnFinished { .. } => self.send_queued(),
            Event::ToolFinished { call_id, tool, .. } if tool == "bash" => {
                self.track_job(&envelope.agent, call_id);
            }
            _ => {}
        }
    }

    /// Sends each tab its next queued prompt once its agent is free.
    fn send_queued(&mut self) {
        for tab in &mut self.tabs {
            if !tab.handle.is_busy() {
                if let Some(prompt) = tab.queued.pop_front() {
                    tab.handle.send(Self::prompt_blocks(&prompt));
                }
            }
        }
    }

    pub fn on_approval(&mut self, pending: Pending) {
        self.approvals.push_back(Approval {
            request: pending.request,
            reply: Some(pending.reply),
            arrived: Instant::now(),
            details: false,
            focus: 0,
        });
        self.approvals_hidden = false;
        self.dirty = true;
    }

    pub fn on_update(&mut self, update: Update) {
        match update {
            Update::Models(models) => {
                self.models = models;
                self.discovering = false;
                let rows = self.model_rows();
                if let Some(Overlay::Models(picker)) = &mut self.overlay {
                    picker.set_rows(rows);
                    picker.note.clear();
                }
            }
            Update::Files { root, list } => {
                if root == self.files.root {
                    self.files.list = Some(list);
                    self.files.loading = false;
                }
            }
            Update::Pasted(Ok(path)) => {
                if self.attach(&path) {
                    self.flash_ok("attached the clipboard image");
                }
            }
            Update::Pasted(Err(err)) => self.flash(err),
            Update::Checked(result) | Update::Installed(result) => {
                let message = match &result {
                    Ok(status) => {
                        self.update = status.clone();
                        Ok(
                            match (&status.installed, status.available, &status.latest) {
                                (Some(version), _, _) => {
                                    format!("installed {version}; restart malac to use it")
                                }
                                (None, true, Some(latest)) => format!("{latest} is available"),
                                _ => "malac is up to date".to_owned(),
                            },
                        )
                    }
                    Err(err) => Err(err.clone()),
                };
                match &mut self.overlay {
                    Some(Overlay::Update(dialog)) => {
                        dialog.installing = false;
                        dialog.result = Some(message);
                    }
                    _ => match message {
                        Ok(message) => self.flash(message),
                        Err(err) => self.notice(Level::Error, format!("update failed: {err}")),
                    },
                }
            }
        }
        self.dirty = true;
    }

    /// Housekeeping between events; returns whether the screen changes.
    pub fn tick(&mut self) -> bool {
        let before = self.approvals.len();
        self.approvals.retain(|a| !a.is_closed());
        let mut changed = before != self.approvals.len();
        if self.approvals.is_empty() {
            self.approvals_hidden = false;
        }
        if self
            .flash
            .as_ref()
            .is_some_and(|f| f.at.elapsed() > Duration::from_secs(4))
        {
            self.flash = None;
            changed = true;
        }
        if self.branch_read.elapsed() > Duration::from_secs(5) {
            self.branch_read = Instant::now();
            let branch = git_branch(&self.tab_cwd());
            changed |= branch != self.branch;
            self.branch = branch;
        }
        if self.jobs_polled.elapsed() > Duration::from_secs(1) {
            self.jobs_polled = Instant::now();
            changed |= self.poll_jobs();
        }
        changed |= self.drag_scroll();
        let clock = chrono::Local::now().format("%H:%M").to_string();
        if clock != self.clock {
            self.clock = clock;
            changed = true;
        }
        self.send_queued();
        // Animate while anything works, and once more when it stops.
        let was_working = self.working();
        self.refresh_snapshots();
        changed || was_working || self.working()
    }

    fn working(&self) -> bool {
        self.running_count() > 0 || self.tabs.iter().any(|t| t.handle.is_busy())
    }

    pub fn clock(&self) -> &str {
        &self.clock
    }

    /// Pasted text goes to the prompt or the open dialog's filter. Paths
    /// of files dropped on the terminal attach those files instead.
    pub fn on_paste(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or_default();
        match &mut self.overlay {
            Some(Overlay::Models(p) | Overlay::Sessions(p) | Overlay::Agents(p)) => {
                p.filter.push_str(line);
            }
            Some(Overlay::Folder(f)) => f.filter.push_str(line),
            Some(Overlay::Rename(r)) => r.text.push_str(line),
            Some(_) => {}
            None => {
                self.focus = Focus::Input;
                let dropped = attach::dropped_paths(text).filter(|_| !self.tab().read_only());
                match dropped {
                    Some(paths) => {
                        let attached = paths.iter().filter(|p| self.attach(p)).count();
                        if attached > 0 {
                            let noun = if attached == 1 { "file" } else { "files" };
                            self.flash_ok(format!("attached {attached} {noun}"));
                        }
                    }
                    None => {
                        self.editor.insert(text);
                        self.completion_dismissed = false;
                    }
                }
            }
        }
    }

    /// The status line's left side: a message, what the agent does, or the directory.
    pub fn activity(&self) -> Vec<Span<'static>> {
        let t = theme();
        if let Some(flash) = &self.flash {
            return match flash.ok {
                true => vec![
                    Span::styled("✓ ", t.fg(t.accent)),
                    Span::raw(flash.text.clone()),
                ],
                false => vec![Span::styled(flash.text.clone(), t.fg(t.warning))],
            };
        }
        let tab = self.tab();
        if self.busy() {
            let view = self.agents.get(tab.root());
            let snapshot = self.snapshots.get(tab.root());
            let mut doing = snapshot.map(|s| s.activity.clone()).unwrap_or_default();
            if let Some(tool) = view.and_then(|v| v.writing.as_ref()) {
                doing = format!("writing {tool} call");
            }
            if doing.is_empty() {
                doing = "working".to_owned();
            }
            let elapsed = view
                .and_then(|v| v.turn_started.filter(|_| v.running))
                .map(|s| format!(" · {}", duration(s.elapsed())))
                .unwrap_or_default();
            let queued = match tab.queued.len() {
                0 => String::new(),
                n => format!(" · {n} queued"),
            };
            return vec![
                Span::styled(format!("{} ", self.spinner()), t.fg(t.accent)),
                Span::raw(text::first_line(&doing).to_owned()),
                Span::styled(format!("{elapsed}{queued} · F8 stops"), t.dim()),
            ];
        }
        vec![Span::styled(short_path(&self.tab_cwd()), t.dim())]
    }

    pub fn update_segment(&self) -> Option<String> {
        match (
            &self.update.installed,
            self.update.available,
            &self.update.latest,
        ) {
            (Some(version), _, _) => Some(format!("restart for {version}")),
            (None, true, Some(latest)) => Some(format!("update {latest}")),
            _ => None,
        }
    }
}

pub fn completions(input: &str) -> Vec<&'static (&'static str, &'static str, &'static str)> {
    let Some(prefix) = input.strip_prefix('/') else {
        return Vec::new();
    };
    if prefix.contains(char::is_whitespace) {
        return Vec::new();
    }
    COMMANDS
        .iter()
        .chain(TUI_COMMANDS)
        .filter(|(name, ..)| name.starts_with(prefix))
        .collect()
}

fn dirs_home() -> Option<String> {
    std::env::var("HOME").ok().filter(|h| !h.is_empty())
}

/// The directory with the home directory shown as `~`.
pub fn short_path(path: &Path) -> String {
    let path = path.display().to_string();
    match dirs_home() {
        Some(home) if path.starts_with(&home) => path.replacen(&home, "~", 1),
        _ => path,
    }
}

/// The checked-out branch, read from `.git/HEAD` without running git.
pub fn git_branch(dir: &Path) -> Option<String> {
    for dir in dir.ancestors() {
        let git = dir.join(".git");
        let head = if git.is_dir() {
            git.join("HEAD")
        } else if git.is_file() {
            let link = std::fs::read_to_string(&git).ok()?;
            dir.join(link.strip_prefix("gitdir:")?.trim()).join("HEAD")
        } else {
            continue;
        };
        let head = std::fs::read_to_string(head).ok()?;
        let head = head.trim();
        return Some(match head.strip_prefix("ref: refs/heads/") {
            Some(branch) => branch.to_owned(),
            None => head.chars().take(7).collect(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_command_names_only() {
        let names: Vec<&str> = completions("/m").iter().map(|c| c.0).collect();
        assert_eq!(names, vec!["model", "mode", "mcp", "memory"]);
        assert!(completions("/model lora").is_empty());
        assert!(completions("hello").is_empty());
        assert_eq!(completions("/").len(), COMMANDS.len() + TUI_COMMANDS.len());
        assert_eq!(completions("/c")[0].0, "compact");
        assert!(completions("/c").iter().any(|c| c.0 == "cd"));
    }

    #[test]
    fn hit_map_prefers_the_top_layer() {
        let mut hits = HitMap::default();
        hits.push(Rect::new(0, 0, 10, 10), Target::Input);
        hits.push(Rect::new(2, 2, 3, 1), Target::Tab(1));
        assert_eq!(hits.at(3, 2), Some(&Target::Tab(1)));
        assert_eq!(hits.at(3, 3), Some(&Target::Input));
        assert_eq!(hits.at(10, 0), None);
    }

    #[test]
    fn reads_the_git_branch() {
        let dir = std::env::temp_dir().join(format!("malac-git-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(git_branch(&dir.join("sub")).as_deref(), Some("feature/x"));
        std::fs::write(dir.join(".git/HEAD"), "0123456789abcdef\n").unwrap();
        assert_eq!(git_branch(&dir).as_deref(), Some("0123456"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
