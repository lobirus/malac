//! The application: a workspace's tabs as the daemon holds them, the agents
//! working in each, the prompt editor, menus and dialogs, and what every
//! key, click, action and daemon message does. Everything shown is built
//! from the daemon's snapshot and the changes it broadcasts; everything
//! the user asks for goes through [`Backend`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use malac_core::agent::AgentInfo;
use malac_core::commands::CommandInfo;
use malac_core::config::{self, Config};
use malac_core::context::Summary;
use malac_core::event::{AgentId, Envelope, Event, Level, TurnStatus};
use malac_core::permission::{Decision, Mode, PermissionRequest};
use malac_core::session::SessionSummary;
use malac_core::tools::Tool;
use malac_core::update::Status;
use malac_core::{AgentSnapshot, AgentStatus};
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use serde::Deserialize;

use super::agentmap::{AgentMap, Node, State};
use super::attach::{self, Attachment};
use super::bars::{self, Mark};
use super::dialog::{
    self, Approval, Ask, Confirm, Help, PickKind, Picker, Rename, Row, UpdateDialog,
};
use super::editor::{self, Editor};
use super::files;
use super::folder::FolderBrowser;
use super::jobs::JobsView;
use super::menu::{Item, Menu, Place};
use super::remote::{Page as RemotePage, RemoteDialog};
use super::select::{self, Point, Selection};
use super::term;
use super::text;
use super::theme::theme;
use super::transcript::{AgentView, Agents, Entry, SPINNER, duration};
use super::voice::{Dictation, Local, Openable, TalkView};
use crate::client::{Client, Events, Launched};
use crate::connections::Endpoint;
use crate::daemon::uploads::{MAX_PROMPT_UPLOAD_BYTES, MAX_UPLOAD_BYTES};
use crate::protocol::{
    AwakeStatus, ControlStatus, DaemonStatus, JobView, RemoteStatus, ServerMessage, Snapshot,
    TabId, TabView, VoiceStatus, WorkspaceSummary, WorkspaceView,
};
use crate::render::tokens;
use crate::repl::{COMMANDS, Slash};

mod actions;
mod backend;
mod input;
mod mouse;
mod voice;

pub use actions::Action;
pub use backend::{Backend, Connection, Reply, Rest};

/// Commands only the full-screen interface has, completed with the rest.
pub const TUI_COMMANDS: &[(&str, &str, &str)] = &[
    ("cd", "[dir]", "work in another folder, in a new tab"),
    ("jobs", "", "background processes the agents started"),
    (
        "root",
        "",
        "open the repository this folder is part of, in a tab",
    ),
    (
        "open",
        "[n]",
        "open the newest file or image malac sent (n: the nth newest)",
    ),
    ("talk", "", "talk mode: speak with malac (Esc ends)"),
    (
        "speak",
        "[on|off]",
        "whether this terminal speaks when malac does",
    ),
    (
        "voice",
        "[setting value]",
        "dictation, speech and talk mode: status and settings",
    ),
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

/// One tab: a project folder in the workspace and the agents working in
/// it, plus what this terminal shows of it.
pub struct Tab {
    /// `None` until the daemon has created the tab.
    pub id: Option<TabId>,
    pub name: String,
    pub cwd: PathBuf,
    /// Top-level agents, in the daemon's order.
    pub agents: Vec<AgentId>,
    /// The agent this terminal shows: the daemon's active one at first.
    pub agent: Option<AgentId>,
    pub mode: Mode,
    /// The model for the tab's next new agent.
    pub model: Option<String>,
    /// Subagents opened inside the shown agent, innermost last.
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
    /// A prompt is on its way and its turn has not started yet.
    pub sending: bool,
    /// This terminal asked the daemon to create the tab.
    pub creating: bool,
    /// The folder's layout and custom commands, once the daemon loaded it.
    pub project: Option<Summary>,
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
    /// A tab this terminal shows before the daemon has one, for an empty workspace.
    fn placeholder(cwd: PathBuf) -> Self {
        Self {
            id: None,
            name: text::truncate(&short_path(&cwd), 18).into_owned(),
            cwd,
            agents: Vec::new(),
            agent: None,
            mode: Mode::Ask,
            model: None,
            view: Vec::new(),
            scroll: None,
            seen: 0,
            selected: None,
            draft: String::new(),
            draft_attachments: Vec::new(),
            queued: VecDeque::new(),
            sending: false,
            creating: false,
            project: None,
        }
    }

    fn from_view(view: &TabView) -> Self {
        let mut tab = Self::placeholder(view.cwd.clone());
        tab.id = Some(view.id.clone());
        tab.update_from(view);
        tab
    }

    /// Takes the daemon's view of the tab, keeping what this terminal shows
    /// unless its agent left the tab.
    fn update_from(&mut self, view: &TabView) {
        self.name = view.name.clone();
        self.cwd = view.cwd.clone();
        self.agents = view.agents.clone();
        self.mode = view.mode;
        // Only loaded tabs report it; what an unloaded one said still holds.
        if view.project.is_some() {
            self.project = view.project.clone();
        }
        if self.agent.as_ref().is_none_or(|a| !self.agents.contains(a)) {
            self.agent = view.active_agent.clone();
            self.reset_view();
        }
    }

    /// The agent shown when no subagent is opened.
    pub fn root(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    /// The tab's custom slash commands.
    pub fn commands(&self) -> &[CommandInfo] {
        self.project.as_ref().map_or(&[], |p| p.commands.as_slice())
    }

    pub fn shown(&self) -> Option<&str> {
        self.view.last().map(String::as_str).or(self.root())
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
    Workspaces(Picker),
    AgentMap(AgentMap),
    Folder(FolderBrowser),
    Jobs(JobsView),
    Help(Help),
    Rename(Rename),
    Confirm(Confirm),
    Update(UpdateDialog),
    Remote(RemoteDialog),
}

/// Results of work done in the background.
pub enum Update {
    /// The daemon's reply to a command, and what it was for.
    Reply(Reply, Result<ServerMessage, String>),
    /// A REST answer.
    Rest(Rest, Result<serde_json::Value, String>),
    Files {
        root: PathBuf,
        list: Vec<String>,
    },
    /// The clipboard's image, saved to a file.
    Pasted(Result<Attachment, String>),
    /// Reconnecting gave up, with why.
    Unreachable(String),
    /// Another daemon's workspaces, for the switcher.
    OtherWorkspaces(Endpoint, Result<Vec<WorkspaceSummary>, String>),
    /// Switching to another daemon failed.
    SwitchFailed(String),
    /// What dictation heard.
    Dictated(Result<String, String>),
    /// What a talk session (this terminal's nth) reported.
    Talk(u64, crate::voice::TalkEvent),
    /// Something malac sent opened in the viewer: its name, or why not.
    Opened(Result<String, String>),
    /// Speech could not play, and why.
    Speech(String),
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

/// A provider and the models it offers, as `GET /api/models` reports.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Discovered {
    pub provider: String,
    #[serde(default)]
    pub configured: Vec<String>,
    #[serde(default)]
    pub available: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

impl Discovered {
    /// Configured models first, then the rest the provider reports.
    pub fn all(&self) -> Vec<String> {
        let mut models = self.configured.clone();
        for model in &self.available {
            if !models.contains(model) {
                models.push(model.clone());
            }
        }
        models
    }
}

/// The connection to the daemon, as the interface sees it.
pub struct Link {
    pub connected: bool,
    /// Trying to connect again, in the background.
    pub reconnecting: bool,
    /// Why the daemon said it stopped.
    pub stopped: Option<String>,
    /// This terminal asked the daemon to stop, to start a fresh one.
    pub restarting: bool,
}

pub struct App {
    pub backend: Backend,
    /// The user's settings, for display only: thinking and context windows.
    pub config: Config,
    pub daemon: DaemonStatus,
    pub link: Link,
    pub workspace_id: Option<String>,
    pub workspace_name: String,
    pub workspaces: Vec<WorkspaceSummary>,
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
    /// Every configured `provider/model`.
    pub models: Vec<String>,
    pub default_model: Option<String>,
    /// What the providers report, once asked.
    pub discovered: Vec<Discovered>,
    pub discovering: bool,
    pub update: Status,
    pub control: ControlStatus,
    pub remote: RemoteStatus,
    pub awake: AwakeStatus,
    /// What dictation, speech and talk mode offer.
    pub voice: VoiceStatus,
    /// "Speak on this terminal", remembered in `~/.malac/terminal.json`.
    pub speak: bool,
    /// This connection's listener id while it plays speech.
    pub listener: Option<u64>,
    /// `voice_listen` is on its way.
    listen_asked: bool,
    pub dictation: Option<Dictation>,
    pub talk: Option<TalkView>,
    /// Talk sessions started here so far.
    talks: u64,
    /// Workspaces of the other daemons, as they answered.
    pub other_workspaces: Vec<(Endpoint, Result<Vec<WorkspaceSummary>, String>)>,
    pub hits: HitMap,
    pub geometry: Geometry,
    /// Every agent of the workspace as last reported, kept current from events.
    pub snapshots: HashMap<AgentId, AgentSnapshot>,
    pub branch: Option<String>,
    /// Saved top-level sessions, newest first, as the daemon lists them.
    pub sessions: Vec<SessionSummary>,
    /// Those started in the active tab's folder that no tab holds.
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
    pub jobs: Vec<JobView>,
    jobs_polled: Instant,
    pub agents_view: AgentsView,
    /// Folders whose "open the whole repository" notice was put away.
    pub hidden_suggestions: HashSet<PathBuf>,
    /// The tab this terminal created and shows once the daemon announces it.
    focus_tab: Option<TabId>,
    /// Where `malac` started, for a workspace's first tab.
    pub home_cwd: PathBuf,
    tools: Vec<Arc<dyn Tool>>,
    pub started: Instant,
    pub dirty: bool,
    pub clear_screen: bool,
    pub quit: bool,
    quit_armed: Option<Instant>,
    clock: String,
    branch_read: Instant,
    history_path: PathBuf,
}

/// Approval keys are ignored this long after a prompt appears, so typing
/// that was meant for the editor cannot answer it.
const ANSWER_GUARD: Duration = Duration::from_millis(300);
const DOUBLE_PRESS: Duration = Duration::from_millis(1200);
/// How often an open jobs view refreshes the log it shows.
const LOG_REFRESH: Duration = Duration::from_secs(1);

impl App {
    /// The interface for the workspace `launched` opened, on a connection
    /// whose first message was `snapshot`.
    pub fn new(
        backend: Backend,
        config: Config,
        snapshot: Snapshot,
        workspace: &WorkspaceView,
        tab: Option<&str>,
        cwd: PathBuf,
    ) -> Self {
        let history_path = config::home_dir().join("history.txt");
        let mut agents = Agents::default();
        agents.show_thinking = config.ui.show_thinking;
        let mut app = Self {
            backend,
            config,
            daemon: snapshot.daemon.clone(),
            link: Link {
                connected: true,
                reconnecting: false,
                stopped: None,
                restarting: false,
            },
            workspace_id: None,
            workspace_name: String::new(),
            workspaces: Vec::new(),
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
            default_model: None,
            discovered: Vec::new(),
            discovering: false,
            update: snapshot.update.clone(),
            control: ControlStatus::default(),
            remote: RemoteStatus::default(),
            awake: AwakeStatus::default(),
            voice: VoiceStatus::default(),
            speak: Local::load().speak,
            listener: None,
            listen_asked: false,
            dictation: None,
            talk: None,
            talks: 0,
            other_workspaces: Vec::new(),
            hits: HitMap::default(),
            geometry: Geometry::default(),
            snapshots: HashMap::new(),
            branch: None,
            sessions: Vec::new(),
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
            hidden_suggestions: HashSet::new(),
            focus_tab: None,
            home_cwd: cwd,
            tools: malac_core::tools::builtin(),
            started: Instant::now(),
            dirty: true,
            clear_screen: false,
            quit: false,
            quit_armed: None,
            clock: String::new(),
            branch_read: Instant::now(),
            history_path,
        };
        app.apply_snapshot(Snapshot {
            workspace: Some(workspace.clone()),
            ..snapshot
        });
        if let Some(index) = tab.and_then(|id| app.tab_index(id)) {
            app.active = index;
        }
        app.branch = git_branch(&app.tab_cwd());
        app.recent = recent_sessions(&app.tab_cwd(), &app.tabs, &app.sessions);
        app
    }

    /// The interface on another daemon, keeping how this one was set up:
    /// the editor and its history, the sidebar, the mouse and the views.
    pub fn switched(
        self,
        client: Client,
        endpoint: Endpoint,
        snapshot: Snapshot,
        launched: Launched,
    ) -> Self {
        let mut backend = self.backend;
        let label = endpoint.label();
        backend.switch_to(client, endpoint);
        let home_cwd = match backend.endpoint() {
            Endpoint::Local => std::env::current_dir().unwrap_or_else(|_| self.home_cwd.clone()),
            Endpoint::Saved(_) => PathBuf::from("~"),
        };
        let mut app = Self::new(
            backend,
            self.config,
            snapshot,
            &launched.workspace,
            launched.tab.as_deref(),
            home_cwd,
        );
        app.editor = self.editor;
        app.attachments = self.attachments;
        app.sidebar = self.sidebar;
        app.mouse_capture = self.mouse_capture;
        app.agents.show_thinking = self.agents.show_thinking;
        app.agents_view = self.agents_view;
        app.clear_screen = true;
        crate::connections::remember(app.backend.endpoint().name());
        app.flash_ok(format!("connected to {label}"));
        app
    }

    /// The daemon's name when it is not this machine's.
    pub fn connection_name(&self) -> Option<&str> {
        self.backend.endpoint().name()
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    fn tab_index(&self, id: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.id.as_deref() == Some(id))
    }

    /// The tab a top-level agent or one of its subagents belongs to.
    fn tab_of_agent(&self, id: &str) -> Option<usize> {
        let root = self.agents.root_of(id);
        self.tabs.iter().position(|t| {
            t.agent.as_deref() == Some(root) || t.agents.iter().any(|a| a == root || a == id)
        })
    }

    pub fn shown(&self) -> Option<&AgentView> {
        self.agents.get(self.tab().shown()?)
    }

    /// The shown agent and its subagents, with their depth.
    pub fn tree(&self, tab: &Tab) -> Vec<(usize, &AgentView)> {
        tab.root()
            .map_or_else(Vec::new, |root| self.agents.tree(root))
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

    fn tab_busy(&self, tab: &Tab) -> bool {
        tab.sending || self.tree(tab).iter().any(|(_, v)| self.is_running(&v.id))
    }

    /// The shown agent of the active tab works, or a prompt is on its way.
    pub fn busy(&self) -> bool {
        self.tab_busy(self.tab())
    }

    /// Whether any agent of a tab works, the one shown or another.
    fn tab_working(&self, tab: &Tab) -> bool {
        tab.sending
            || tab
                .agents
                .iter()
                .flat_map(|root| self.agents.tree(root))
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

    /// Adds a line to the transcript on screen, or flashes it when no
    /// agent is shown yet.
    pub fn notice(&mut self, level: Level, message: impl Into<String>) {
        let message = message.into();
        if !self.push_entry(Entry::Notice(level, message.clone())) {
            self.flash(message);
        }
    }

    fn info(&mut self, lines: Vec<Line<'static>>) {
        if self.push_entry(Entry::Info(lines)) {
            self.tab_mut().scroll = None;
        }
    }

    fn push_entry(&mut self, entry: Entry) -> bool {
        let Some(id) = self.tab().shown().map(str::to_owned) else {
            return false;
        };
        match self.agents.get_mut(&id) {
            Some(view) => {
                view.transcript.push(entry);
                true
            }
            None => false,
        }
    }

    fn switch_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let changed = index != self.active;
        if changed {
            self.stash_draft();
            self.active = index;
            self.restore_draft();
        }
        self.focus = Focus::Input;
        self.selection = None;
        self.branch = git_branch(&self.tab_cwd());
        // The daemon remembers the active tab, which `malac` returns to.
        if let Some(id) = self.tab().id.clone().filter(|_| changed) {
            self.backend
                .command(crate::protocol::Command::TabActivate { tab: id });
        }
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
        if tab.name.is_empty() {
            "new".to_owned()
        } else {
            tab.name.clone()
        }
    }

    pub fn tab_mark(&self, index: usize) -> Mark {
        let tab = &self.tabs[index];
        let ids: Vec<&str> = tab
            .agents
            .iter()
            .flat_map(|root| self.agents.tree(root))
            .map(|(_, v)| v.id.as_str())
            .collect();
        if self
            .approvals
            .iter()
            .any(|a| ids.contains(&a.request.agent.as_str()))
        {
            return Mark::Waiting;
        }
        if self.tab_working(tab) {
            return Mark::Running(self.spinner());
        }
        match tab
            .root()
            .and_then(|r| self.agents.get(r))
            .and_then(|v| v.last_status)
        {
            Some(TurnStatus::Completed) => Mark::Done,
            Some(TurnStatus::Failed) => Mark::Failed,
            _ => Mark::Idle,
        }
    }

    /// Slash commands matching what is typed, while the command name is
    /// typed: the built-in ones, then the tab's own.
    pub fn completions(&self) -> Vec<Completion> {
        if self.completion_dismissed || self.focus != Focus::Input {
            return Vec::new();
        }
        completions(self.editor.text(), self.tab().commands())
    }

    /// The `@` being typed before the cursor and the query after it.
    pub fn file_query(&self) -> Option<(usize, &str)> {
        if self.focus != Focus::Input {
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

    /// Attaches a file or folder; refusals are flashed.
    pub fn attach(&mut self, path: &Path) -> bool {
        match Attachment::open(path, &self.tab_cwd()).and_then(|a| self.sendable(a)) {
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

    /// A file from this machine goes to a daemon elsewhere as its contents,
    /// which a folder has none of and which may be only so large.
    fn sendable(&self, attachment: Attachment) -> Result<Attachment, String> {
        if !self.backend.is_remote() {
            return Ok(attachment);
        }
        let to = self.backend.endpoint().label();
        match (attachment.kind, attachment.size) {
            (attach::Kind::Directory, _) => Err(format!(
                "{}/ stays on this machine: {to} can be sent files, not folders",
                attachment.name
            )),
            (_, Some(bytes)) if bytes > MAX_UPLOAD_BYTES as u64 => Err(format!(
                "{} is {}, more than the {} {to} accepts",
                attachment.name,
                attach::size(bytes),
                attach::size(MAX_UPLOAD_BYTES as u64)
            )),
            (_, Some(bytes)) => {
                let sent: u64 = self.attachments.iter().filter_map(|a| a.size).sum();
                if sent + bytes > MAX_PROMPT_UPLOAD_BYTES as u64 {
                    return Err(format!(
                        "{} would bring this prompt's files to {}, more than the {} {to} \
                         accepts at once: send it with the next prompt",
                        attachment.name,
                        attach::size(sent + bytes),
                        attach::size(MAX_PROMPT_UPLOAD_BYTES as u64)
                    ));
                }
                Ok(attachment)
            }
            _ => Ok(attachment),
        }
    }

    /// The working directory of the active tab.
    pub fn tab_cwd(&self) -> PathBuf {
        self.tab().cwd.clone()
    }

    /// The repository the active tab's folder is part of, when the daemon
    /// suggests opening it instead and the notice was not put away.
    pub fn suggested_root(&self) -> Option<&Path> {
        let tab = self.tab();
        let root = tab.project.as_ref()?.layout.suggest_root.as_deref()?;
        (!self.hidden_suggestions.contains(&tab.cwd)).then_some(root)
    }

    fn is_ours(&self, workspace: &str) -> bool {
        self.workspace_id.as_deref() == Some(workspace)
    }

    /// Applies a message the daemon broadcast or sent unasked.
    pub fn on_message(&mut self, message: ServerMessage) {
        self.dirty = true;
        match message {
            ServerMessage::Event {
                workspace,
                envelope,
                ..
            } => self.on_event(workspace.as_deref(), &envelope),
            ServerMessage::Snapshot(snapshot) => self.apply_snapshot(*snapshot),
            ServerMessage::WorkspaceCreated { workspace }
            | ServerMessage::WorkspaceUpdated { workspace } => {
                if self.is_ours(&workspace.id) {
                    self.workspace_name = workspace.name.clone();
                }
                match self.workspaces.iter_mut().find(|w| w.id == workspace.id) {
                    Some(known) => *known = workspace,
                    None => self.workspaces.push(workspace),
                }
            }
            ServerMessage::WorkspaceDeleted { workspace } => {
                self.workspaces.retain(|w| w.id != workspace);
                if self.is_ours(&workspace) {
                    self.workspace_id = None;
                    self.notice(Level::Warning, "this workspace was deleted");
                    self.backend.request(
                        crate::protocol::Command::Snapshot { workspace: None },
                        Reply::Snapshot,
                    );
                }
            }
            ServerMessage::TabCreated {
                workspace,
                index,
                tab,
            } if self.is_ours(&workspace) => self.tab_created(index, &tab),
            ServerMessage::TabUpdated { workspace, tab } if self.is_ours(&workspace) => {
                self.absorb(&tab);
                if let Some(index) = self.tab_index(&tab.id) {
                    self.tabs[index].update_from(&tab);
                }
            }
            ServerMessage::TabClosed { workspace, tab } if self.is_ours(&workspace) => {
                self.tab_closed(&tab);
            }
            ServerMessage::TabsReordered { workspace, tabs } if self.is_ours(&workspace) => {
                self.reorder(&tabs);
            }
            ServerMessage::JobStarted { job } | ServerMessage::JobFinished { job } => {
                match self.jobs.iter_mut().find(|j| j.job.id == job.job.id) {
                    Some(known) => *known = job,
                    None => self.jobs.push(job),
                }
            }
            ServerMessage::Control { control, .. } => self.control = control,
            ServerMessage::Notice {
                level,
                message,
                tab,
            } => {
                let here = tab.as_deref().is_none_or(|t| self.tab_index(t).is_some());
                if here {
                    self.notice(level, message);
                }
            }
            ServerMessage::Mode {
                mode,
                tab,
                workspace,
            } => {
                // A mode for a whole workspace names it; one for a tab names the tab.
                let here = tab.is_some() || workspace.as_deref().is_none_or(|w| self.is_ours(w));
                for t in self.tabs.iter_mut().filter(|_| here) {
                    if tab.is_none() || t.id == tab {
                        t.mode = mode;
                    }
                }
            }
            ServerMessage::Clients { clients } => self.daemon.clients = clients,
            ServerMessage::Remote { remote } => {
                self.remote = remote;
                if let Some(Overlay::Remote(_)) = &self.overlay {
                    self.backend.devices();
                }
            }
            ServerMessage::Awake { awake } => self.awake = awake,
            ServerMessage::Voice { voice } => self.voice_changed(voice),
            ServerMessage::Models {
                models,
                default_model,
            } => {
                self.models = models;
                self.default_model = default_model;
                // The next picker asks the providers again, new ones included.
                self.discovered.clear();
            }
            ServerMessage::Say {
                id,
                text,
                reason,
                agent,
                listener,
                ..
            } => self.on_say(&id, &text, &reason, agent.as_deref(), listener),
            ServerMessage::SayCancelled { ids } => self.backend.cancel_speech(&ids),
            ServerMessage::Reloaded => self.flash_ok("settings applied"),
            ServerMessage::Lagged { skipped } => {
                self.flash(format!("missed {skipped} updates; catching up"));
                self.resync();
            }
            ServerMessage::Shutdown { reason } => self.link.stopped = Some(reason),
            ServerMessage::Error { message, .. } => self.flash(message),
            _ => {}
        }
    }

    /// Asks for a fresh snapshot of this workspace.
    pub fn resync(&mut self) {
        let workspace = self.workspace_id.clone();
        self.backend.request(
            crate::protocol::Command::Snapshot { workspace },
            Reply::Snapshot,
        );
    }

    fn on_event(&mut self, workspace: Option<&str>, envelope: &Envelope) {
        if let Event::Update { status } = &envelope.event {
            self.update = status.clone();
            return;
        }
        if workspace.is_some_and(|w| !self.is_ours(w)) {
            return;
        }
        let id = &envelope.agent;
        self.track(envelope);
        match &envelope.event {
            Event::AgentStarted {
                label,
                parent,
                model,
                cwd,
            } if self.agents.get(id).is_none() => {
                let view = AgentView::new(id, label, parent.clone(), model, cwd);
                // A resumed session has a history; a new agent finds none.
                let view = match self.backend.history(id) {
                    true => view.loading(),
                    false => view,
                };
                self.agents.insert(view);
            }
            Event::PermissionRequested {
                request_id,
                call_id,
                tool,
                summary,
                input,
            } if !self.approvals.iter().any(|a| a.request.id == *request_id) => {
                let agent_label = self
                    .snapshots
                    .get(id)
                    .map(|s| s.info.label.clone())
                    .unwrap_or_default();
                self.approvals.push_back(Approval::new(PermissionRequest {
                    id: request_id.clone(),
                    call_id: call_id.clone(),
                    agent: id.clone(),
                    agent_label,
                    tool: tool.clone(),
                    summary: summary.clone(),
                    input: input.clone(),
                }));
                self.approvals_hidden = false;
            }
            Event::PermissionResolved { request_id, .. } => {
                // One that waited before the history was read has no card of its own.
                if let Some(waiting) = self.approvals.iter().find(|a| a.request.id == *request_id) {
                    self.agents
                        .claim_request(id, &waiting.request.call_id, request_id);
                }
                self.approvals.retain(|a| a.request.id != *request_id);
            }
            // The turn began, or ended before this terminal saw it begin.
            Event::UserMessage { .. } | Event::TurnFinished { .. } => {
                if let Some(index) = self.tab_of_agent(id) {
                    self.tabs[index].sending = false;
                }
            }
            _ => {}
        }
        self.agents.apply(envelope);
        if matches!(envelope.event, Event::TurnFinished { .. }) {
            self.send_queued();
        }
    }

    /// Keeps an agent's status, activity and usage current from its events.
    fn track(&mut self, envelope: &Envelope) {
        let id = &envelope.agent;
        if let Event::AgentStarted {
            label,
            parent,
            model,
            cwd,
        } = &envelope.event
        {
            let depth = parent
                .as_ref()
                .and_then(|p| self.snapshots.get(p))
                .map_or(0, |p| p.info.depth + 1);
            let snapshot = self
                .snapshots
                .entry(id.clone())
                .or_insert_with(|| AgentSnapshot {
                    info: AgentInfo {
                        id: id.clone(),
                        label: label.clone(),
                        parent: parent.clone(),
                        depth,
                        read_only: false,
                        cwd: cwd.into(),
                        created: chrono::Utc::now(),
                    },
                    model: model.clone(),
                    status: AgentStatus::Idle,
                    activity: String::new(),
                    usage: Default::default(),
                    report: None,
                });
            snapshot.model = model.clone();
            return;
        }
        let Some(snapshot) = self.snapshots.get_mut(id) else {
            return;
        };
        match &envelope.event {
            Event::UserMessage { .. } => {
                snapshot.status = AgentStatus::Running;
                snapshot.activity = "thinking".into();
                snapshot.report = None;
            }
            Event::ToolCallStreaming { tool } => snapshot.activity = format!("writing {tool} call"),
            Event::ToolStarted { tool, summary, .. } => {
                snapshot.activity = text::truncate(&format!("{tool} {summary}"), 120).into_owned();
            }
            Event::PermissionRequested { tool, .. } => {
                snapshot.activity = format!("waiting for approval: {tool}");
            }
            Event::ToolFinished { .. } => snapshot.activity = "thinking".into(),
            Event::Usage { total, .. } => snapshot.usage = *total,
            Event::ModelChanged { model } => snapshot.model = model.clone(),
            Event::TurnFinished { status, .. } | Event::AgentFinished { status, .. } => {
                snapshot.status = match status {
                    TurnStatus::Completed => AgentStatus::Completed,
                    TurnStatus::Failed => AgentStatus::Failed,
                    TurnStatus::Cancelled => AgentStatus::Cancelled,
                };
                snapshot.activity.clear();
                if let Event::AgentFinished { report, .. } = &envelope.event {
                    snapshot.report = Some(report.clone());
                }
            }
            _ => {}
        }
    }

    /// Takes the agents a tab reports: their snapshots, and a view for each
    /// one this terminal has not seen, filled from its saved conversation.
    fn absorb(&mut self, tab: &TabView) {
        for agent in &tab.snapshots {
            let snapshot = &agent.snapshot;
            let id = snapshot.info.id.clone();
            if self.agents.get(&id).is_none() {
                let view = AgentView::from_snapshot(snapshot);
                let view = match self.backend.history(&id) {
                    true => view.loading(),
                    false => view,
                };
                self.agents.insert(view);
                if snapshot.info.parent.is_some() {
                    let finished = match snapshot.status {
                        AgentStatus::Completed => Some(TurnStatus::Completed),
                        AgentStatus::Failed => Some(TurnStatus::Failed),
                        AgentStatus::Cancelled => Some(TurnStatus::Cancelled),
                        AgentStatus::Running | AgentStatus::Idle => None,
                    };
                    self.agents.ensure_card(&id, finished);
                }
            }
            self.snapshots.insert(id, snapshot.clone());
        }
    }

    /// Replaces what the interface knows with a snapshot.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.daemon = snapshot.daemon;
        self.workspaces = snapshot.workspaces;
        self.update = snapshot.update;
        self.control = snapshot.control;
        self.remote = snapshot.remote;
        self.awake = snapshot.awake;
        self.voice_changed(snapshot.voice);
        self.models = snapshot.models;
        self.default_model = snapshot.default_model;
        self.jobs = snapshot.jobs;
        // Saved sessions come from the daemon, which may run elsewhere.
        self.backend.sessions();
        match &snapshot.workspace {
            Some(workspace) => self.apply_workspace(workspace),
            None => {
                // Every workspace is gone: start a fresh one here.
                self.backend.request(
                    crate::protocol::Command::WorkspaceCreate { name: None },
                    Reply::WorkspaceOpened { fresh: true },
                );
            }
        }
        let ours: Vec<TabId> = self.tabs.iter().filter_map(|t| t.id.clone()).collect();
        let pending: Vec<Approval> = snapshot
            .pending
            .into_iter()
            .filter(|p| p.tab.as_ref().is_none_or(|t| ours.contains(t)))
            .map(|p| {
                let known = self
                    .approvals
                    .iter()
                    .position(|a| a.request.id == p.request.id);
                match known.and_then(|i| self.approvals.remove(i)) {
                    Some(approval) => approval,
                    None => Approval::new(p.request),
                }
            })
            .collect();
        self.approvals = pending.into();
        if self.approvals.is_empty() {
            self.approvals_hidden = false;
        }
    }

    /// Shows a workspace: its tabs, keeping what this terminal had of the
    /// ones it already showed.
    fn apply_workspace(&mut self, workspace: &WorkspaceView) {
        let switching = !self.is_ours(&workspace.id);
        let active = match switching {
            true => workspace.active_tab.clone(),
            false => self.tab().id.clone().or(workspace.active_tab.clone()),
        };
        self.workspace_id = Some(workspace.id.clone());
        self.workspace_name = workspace.name.clone();
        if switching {
            // Agents of the workspace left behind send nothing here any more.
            self.snapshots.clear();
        }
        let mut old = std::mem::take(&mut self.tabs);
        let mut tabs = Vec::new();
        for view in &workspace.tabs {
            self.absorb(view);
            let tab = match old.iter().position(|t| t.id.as_ref() == Some(&view.id)) {
                Some(i) => {
                    let mut tab = old.remove(i);
                    tab.update_from(view);
                    tab
                }
                None => Tab::from_view(view),
            };
            tabs.push(tab);
        }
        if tabs.is_empty() {
            let cwd = old
                .first()
                .map_or_else(|| self.home_cwd.clone(), |t| t.cwd.clone());
            tabs.push(Tab::placeholder(cwd));
        }
        self.tabs = tabs;
        self.active = active
            .and_then(|id| self.tab_index(&id))
            .unwrap_or(0)
            .min(self.tabs.len() - 1);
        if switching {
            self.selection = None;
            self.recent = recent_sessions(&self.tab_cwd(), &self.tabs, &self.sessions);
        }
    }

    fn tab_created(&mut self, index: usize, view: &TabView) {
        self.absorb(view);
        if self.tab_index(&view.id).is_some() {
            return;
        }
        // An empty workspace's placeholder becomes the first real tab.
        let placeholder = self.tabs.iter().position(|t| t.id.is_none() && !t.creating);
        match placeholder {
            Some(i) if self.tabs.len() == 1 => {
                self.tabs[i].id = Some(view.id.clone());
                self.tabs[i].update_from(view);
            }
            _ => {
                let index = index.min(self.tabs.len());
                self.tabs.insert(index, Tab::from_view(view));
                if index <= self.active && self.tabs.len() > 1 {
                    self.active += 1;
                }
            }
        }
        if self.focus_tab.as_deref() == Some(&view.id) {
            self.focus_tab = None;
            if let Some(i) = self.tab_index(&view.id) {
                self.switch_tab(i);
            }
        }
    }

    fn tab_closed(&mut self, id: &str) {
        let Some(index) = self.tab_index(id) else {
            return;
        };
        let was_active = index == self.active;
        let closed = self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.tabs.push(Tab::placeholder(closed.cwd));
        }
        if was_active {
            self.active = index.min(self.tabs.len() - 1);
            // What was being typed stays; the next tab's draft fills an empty editor.
            if self.editor.is_empty() && self.attachments.is_empty() {
                self.restore_draft();
            }
            self.selection = None;
        } else if self.active > index {
            self.active -= 1;
        }
    }

    fn reorder(&mut self, order: &[TabId]) {
        let active = self.tab().id.clone();
        self.tabs.sort_by_key(|t| {
            t.id.as_ref()
                .and_then(|id| order.iter().position(|o| o == id))
                .unwrap_or(usize::MAX)
        });
        if let Some(index) = active.and_then(|id| self.tab_index(&id)) {
            self.active = index;
        }
    }

    /// Sends each tab its next queued prompt once the tab exists and its
    /// agent is free.
    fn send_queued(&mut self) {
        for index in 0..self.tabs.len() {
            let tab = &self.tabs[index];
            if tab.queued.is_empty() || self.tab_busy(tab) {
                continue;
            }
            if tab.id.is_none() {
                self.create_tab_for(index);
                continue;
            }
            if let Some(prompt) = self.tabs[index].queued.pop_front() {
                self.dispatch(index, prompt);
            }
        }
    }

    /// Asks the daemon for the placeholder tab at `index`.
    fn create_tab_for(&mut self, index: usize) {
        let tab = &mut self.tabs[index];
        if tab.creating {
            return;
        }
        tab.creating = true;
        let spec = crate::protocol::TabSpec {
            cwd: tab.cwd.clone(),
            workspace: self.workspace_id.clone(),
            ..Default::default()
        };
        self.backend.request(
            crate::protocol::Command::TabCreate(spec),
            Reply::TabCreated { placeholder: true },
        );
    }

    pub fn on_update(&mut self, update: Update) {
        self.dirty = true;
        match update {
            Update::Reply(reply, result) => self.on_reply(reply, result),
            Update::Rest(rest, result) => self.on_rest(rest, result),
            Update::Files { root, list } => {
                if root == self.files.root {
                    self.files.list = Some(list);
                    self.files.loading = false;
                }
            }
            Update::Pasted(Ok(image)) => {
                self.attachments.push(image);
                self.flash_ok("attached the clipboard image");
            }
            Update::Pasted(Err(err)) => self.flash(err),
            Update::Unreachable(err) => {
                self.link.reconnecting = false;
                self.flash(format!(
                    "the daemon is unreachable: {err} · Ctrl-R tries again"
                ));
            }
            Update::OtherWorkspaces(endpoint, result) => {
                self.other_workspaces.retain(|(e, _)| *e != endpoint);
                self.other_workspaces.push((endpoint, result));
                let rows = self.workspace_rows();
                if let Some(Overlay::Workspaces(picker)) = &mut self.overlay {
                    picker.set_rows(rows);
                }
            }
            Update::SwitchFailed(err) => {
                self.flash(format!("cannot switch: {}", text::first_line(&err)));
            }
            Update::Dictated(result) => self.dictated(result),
            Update::Talk(session, event) => self.on_talk(session, event),
            Update::Opened(Ok(name)) => self.flash_ok(format!("opened {name}")),
            Update::Opened(Err(err)) => {
                self.flash(format!("cannot open it: {}", text::first_line(&err)));
            }
            Update::Speech(err) => {
                self.flash(format!("cannot speak: {}", text::first_line(&err)));
            }
        }
    }

    /// Housekeeping between events; returns whether the screen changes.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
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
        if self.jobs_polled.elapsed() > LOG_REFRESH {
            self.jobs_polled = Instant::now();
            self.refresh_job_log();
        }
        changed |= self.drag_scroll();
        let clock = chrono::Local::now().format("%H:%M").to_string();
        if clock != self.clock {
            self.clock = clock;
            changed = true;
        }
        self.send_queued();
        changed |= self.voice_tick();
        // Animate while anything works.
        changed || self.working()
    }

    fn working(&self) -> bool {
        self.running_count() > 0 || self.tabs.iter().any(|t| t.sending)
    }

    pub fn clock(&self) -> &str {
        &self.clock
    }

    /// Pasted text goes to the prompt or the open dialog's filter. Paths
    /// of files dropped on the terminal attach those files instead.
    pub fn on_paste(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or_default();
        match &mut self.overlay {
            Some(
                Overlay::Models(p)
                | Overlay::Sessions(p)
                | Overlay::Agents(p)
                | Overlay::Workspaces(p),
            ) => {
                p.filter.push_str(line);
            }
            Some(Overlay::Folder(f)) => f.filter.push_str(line),
            Some(Overlay::Rename(r)) => r.text.push_str(line),
            Some(_) => {}
            None => {
                self.focus = Focus::Input;
                match attach::dropped_paths(text) {
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
        if !self.link.connected {
            let why = match &self.link.stopped {
                Some(reason) => format!("the daemon stopped ({reason})"),
                None => "the daemon is not reachable".to_owned(),
            };
            let next = if self.link.reconnecting {
                " · reconnecting…"
            } else {
                " · Ctrl-R reconnects"
            };
            return vec![
                Span::styled(why, t.fg(t.warning)),
                Span::styled(next, t.dim()),
            ];
        }
        if let Some(talk) = &self.talk {
            return self.talk_activity(talk);
        }
        let tab = self.tab();
        if self.busy() {
            let view = tab.root().and_then(|r| self.agents.get(r));
            let snapshot = tab.root().and_then(|r| self.snapshots.get(r));
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

    /// A calm note when the daemon runs another version than this terminal.
    pub fn daemon_segment(&self) -> Option<String> {
        self.daemon.version_mismatch.then(|| {
            format!(
                "daemon {} running; restart it when idle",
                self.daemon.version
            )
        })
    }

    /// What the computer-control service is missing, if anything.
    pub fn control_issue(&self) -> Option<&'static str> {
        if !self.control.enabled {
            return None;
        }
        if self.control.error.is_some() {
            return Some("control unavailable");
        }
        let missing = |key: &str| {
            self.control
                .permissions
                .as_ref()
                .is_some_and(|p| p[key] == serde_json::Value::Bool(false))
        };
        (missing("screen_recording") || missing("accessibility"))
            .then_some("control needs permissions")
    }

    /// Agents and jobs running anywhere in the daemon, as far as this
    /// terminal knows: its workspace from events, the others from summaries.
    pub fn daemon_busy(&self) -> bool {
        let elsewhere = self
            .workspaces
            .iter()
            .filter(|w| !self.is_ours(&w.id))
            .any(|w| w.running > 0);
        elsewhere || self.running_count() > 0 || self.jobs.iter().any(|j| j.job.is_running())
    }
}

/// Saved sessions started in `cwd` that no tab holds, for the welcome screen.
pub(super) fn recent_sessions(
    cwd: &Path,
    tabs: &[Tab],
    sessions: &[SessionSummary],
) -> Vec<SessionSummary> {
    let held = |id: &str| tabs.iter().any(|t| t.agents.iter().any(|a| a == id));
    sessions
        .iter()
        .filter(|s| s.meta.cwd == cwd && !held(&s.meta.id) && !s.title.is_empty())
        .take(5)
        .cloned()
        .collect()
}

/// A slash command offered while its name is typed.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub name: String,
    /// What it takes, e.g. `[provider/model]`.
    pub args: String,
    pub help: String,
    /// Where a custom command comes from (`project`, `user`, `plugin <id>`,
    /// `skill`); `None` for the built-in ones.
    pub source: Option<String>,
}

impl Completion {
    /// Every built-in command, then the custom ones whose names are free.
    pub fn all(custom: &[CommandInfo]) -> Vec<Self> {
        let mut all: Vec<Self> = COMMANDS
            .iter()
            .chain(TUI_COMMANDS)
            .map(|(name, args, help)| Self {
                name: (*name).to_owned(),
                args: (*args).to_owned(),
                help: (*help).to_owned(),
                source: None,
            })
            .collect();
        for command in custom {
            if !all.iter().any(|c| c.name == command.name) {
                all.push(Self {
                    name: command.name.clone(),
                    args: command.argument_hint.clone().unwrap_or_default(),
                    help: command.description.clone(),
                    source: Some(command.source.clone()),
                });
            }
        }
        all
    }
}

pub fn completions(input: &str, custom: &[CommandInfo]) -> Vec<Completion> {
    let Some(prefix) = input.strip_prefix('/') else {
        return Vec::new();
    };
    if prefix.contains(char::is_whitespace) {
        return Vec::new();
    }
    Completion::all(custom)
        .into_iter()
        .filter(|c| c.name.starts_with(prefix))
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
        let names = |input: &str, custom: &[CommandInfo]| -> Vec<String> {
            completions(input, custom)
                .into_iter()
                .map(|c| c.name)
                .collect()
        };
        assert_eq!(names("/mo", &[]), vec!["model", "mode"]);
        assert!(completions("/model lora", &[]).is_empty());
        assert!(completions("hello", &[]).is_empty());
        assert_eq!(
            completions("/", &[]).len(),
            COMMANDS.len() + TUI_COMMANDS.len()
        );
        assert!(names("/c", &[]).contains(&"cd".to_owned()));
        let custom = |name: &str| CommandInfo {
            name: name.into(),
            description: "ship it".into(),
            argument_hint: Some("[env]".into()),
            source: "project".into(),
        };
        let found = completions("/de", &[custom("deploy"), custom("model")]);
        assert_eq!(found.len(), 1, "built-in names win");
        assert_eq!(found[0].source.as_deref(), Some("project"));
        assert_eq!(found[0].args, "[env]");
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
