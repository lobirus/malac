//! Everything the user can ask for, and the menus and lists that offer it.

use super::*;
use crate::connections::{self, Connections};
use crate::protocol::{Command, TabSpec};
use malac_core::config::KeepAwake;

/// Everything the user can ask for, from a key, a click, a menu or a command.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Help,
    Copy,
    /// The agent list or map, whichever was used last.
    Agents,
    AgentList,
    AgentMap,
    FolderDialog,
    OpenFolder(PathBuf),
    /// Puts away the notice suggesting the whole repository for this tab.
    HideSuggestion,
    Jobs,
    StopJob(String),
    /// Attach the clipboard's image.
    PasteImage,
    CopySelection,
    PickModel,
    CycleMode,
    TabMenu,
    Sessions,
    Stop,
    Menu,
    Quit,
    NewTab,
    CloseTab(usize),
    /// Close a tab even though its agents work, stopping them.
    ForceCloseTab(TabId),
    NextTab,
    PrevTab,
    GoTab(usize),
    /// Moves the active tab left or right.
    MoveTab(isize),
    RenameTab,
    /// Another top-level agent in the active tab.
    NewAgent,
    /// The next or previous top-level agent of the tab.
    StepAgent(isize),
    ShowTabAgent(AgentId),
    CloseAgent,
    NewWorkspace,
    Workspaces,
    OpenWorkspace(String),
    RenameWorkspace,
    DeleteWorkspace,
    ForceDeleteWorkspace(String),
    /// A name typed into a one-line prompt.
    Named(Ask, String),
    Compact,
    SetMode(Mode),
    ShowMode,
    SetModel(String),
    Resume(String),
    OpenAgent(AgentId),
    StopAgent(AgentId),
    Back,
    ToggleSidebar,
    ToggleMouse,
    ToggleThinking,
    ShowCost,
    ShowMcp,
    ShowMemory,
    ShowWeb,
    CopyWebUrl,
    UpdateDialog,
    CheckUpdate,
    InstallUpdate,
    RestartDaemon,
    Reconnect,
    RequestPermissions,
    ShowApprovals,
    Answer(Decision),
    CopyEntry(usize),
    CopyCode(usize, Option<usize>),
    ToggleEntry(usize),
    JumpBottom,
    CloseDialog,
    /// Opens a menu bar title's drop-down.
    OpenMenu(usize),
    /// Controls another daemon: a saved connection, or `None` for this machine's.
    Connect(Option<String>),
    /// Opens the Remote access dialog, or refreshes it when open.
    RemoteDialog,
    PairDevice,
    /// Asks before revoking a paired device.
    RevokeDevice(String),
    RevokeNow(String),
    KeepAwake(KeepAwake),
    /// Starts dictation, or stops it and types what was said.
    Dictate,
    /// Starts or ends talk mode.
    Talk,
    /// Whether this terminal speaks.
    Speak(bool),
    ShowVoice,
    /// A voice setting and its new value, as `/voice` takes them.
    SetVoice(String, String),
    /// Opens the nth newest file or image malac sent, from 1.
    OpenFile(usize),
    /// Opens a file or image of a transcript entry: the entry and which one.
    OpenEntry(usize, usize),
}

impl App {
    /// The items of a menu bar title.
    pub fn menu_items(&self, index: usize) -> Vec<Item> {
        let mode = self.tab().mode;
        let tab = self.tab();
        let has_agent = tab.root().is_some();
        match index {
            0 => vec![
                Item::new("New tab here", 'n', Action::NewTab).hint("^T"),
                Item::new("Open folder…", 'f', Action::FolderDialog).hint("/cd"),
                self.repository_item(),
                Item::new("Open session…", 'o', Action::Sessions).hint("F7"),
                Item::new("Rename tab…", 'r', Action::RenameTab).enabled(tab.id.is_some()),
                Item::new("Close tab", 't', Action::CloseTab(self.active))
                    .hint("^W")
                    .enabled(self.tabs.len() > 1),
                Item::separator(),
                Item::new("New agent in this tab", 'a', Action::NewAgent).hint("^N"),
                Item::new("Compact conversation", 'c', Action::Compact).enabled(has_agent),
                Item::separator(),
                Item::new("New workspace…", 'w', Action::NewWorkspace),
                Item::new("Switch workspace…", 's', Action::Workspaces),
                Item::new("Rename workspace…", 'e', Action::RenameWorkspace),
                Item::new("Delete workspace…", 'd', Action::DeleteWorkspace),
                Item::separator(),
                Item::new("Quit", 'q', Action::Quit).hint("F10"),
            ],
            1 => {
                let current = self.current_model();
                let mut items = vec![Item::new("All models…", 'a', Action::PickModel).hint("F4")];
                let mut configured = self.models.clone();
                if !configured.contains(&current) && !current.is_empty() {
                    configured.insert(0, current.clone());
                }
                if !configured.is_empty() {
                    items.push(Item::separator());
                }
                for spec in configured.into_iter().take(12) {
                    let checked = spec == current;
                    items.push(Item::plain(spec.clone(), Action::SetModel(spec)).checked(checked));
                }
                items
            }
            2 => vec![
                Item::new("Ask", 'a', Action::SetMode(Mode::Ask))
                    .hint("confirm edits and commands")
                    .checked(mode == Mode::Ask),
                Item::new("Edits", 'e', Action::SetMode(Mode::Edits))
                    .hint("confirm commands")
                    .checked(mode == Mode::Edits),
                Item::new("Autopilot", 'p', Action::SetMode(Mode::Autopilot))
                    .hint("never ask")
                    .checked(mode == Mode::Autopilot),
                Item::separator(),
                Item::new("Cycle", 'c', Action::CycleMode).hint("F5"),
                Item::note("for this tab".to_owned()),
            ],
            3 => self.agent_menu(),
            4 => {
                let mut items = vec![
                    Item::new("Sidebar", 's', Action::ToggleSidebar)
                        .hint("^G")
                        .checked(self.geometry.sidebar),
                    Item::new("Mouse", 'm', Action::ToggleMouse)
                        .hint("^S")
                        .checked(self.mouse_capture),
                    Item::new("Thinking", 't', Action::ToggleThinking)
                        .checked(self.agents.show_thinking),
                    Item::separator(),
                    Item::new("Jobs…", 'j', Action::Jobs).hint("/jobs"),
                    Item::new("Usage and cost", 'u', Action::ShowCost),
                    Item::new("MCP servers", 'c', Action::ShowMcp),
                    Item::new("Memories", 'e', Action::ShowMemory),
                ];
                items.push(Item::separator());
                let mode = self.awake.mode;
                items.push(
                    Item::new(
                        "Keep awake while working",
                        'k',
                        Action::KeepAwake(KeepAwake::Busy),
                    )
                    .checked(mode == KeepAwake::Busy),
                );
                items.push(
                    Item::new(
                        "Keep awake always",
                        'l',
                        Action::KeepAwake(KeepAwake::Always),
                    )
                    .checked(mode == KeepAwake::Always),
                );
                items.push(
                    Item::new(
                        "Let the machine sleep",
                        'z',
                        Action::KeepAwake(KeepAwake::Off),
                    )
                    .checked(mode == KeepAwake::Off),
                );
                items.push(Item::note(self.awake_summary()));
                items.extend(self.voice_items());
                if self.control.enabled {
                    items.push(Item::separator());
                    items.push(Item::note(self.control_summary()));
                    items.push(Item::new(
                        "Request macOS permissions",
                        'p',
                        Action::RequestPermissions,
                    ));
                }
                items
            }
            5 => self.connection_menu(),
            _ => {
                let mut items = vec![
                    Item::new("Keys and commands", 'k', Action::Help).hint("F1"),
                    Item::new("Check for updates", 'c', Action::CheckUpdate),
                    Item::new("Install update", 'i', Action::UpdateDialog)
                        .enabled(dialog::can_install(&self.update)),
                    Item::new("Restart the daemon", 'r', Action::RestartDaemon)
                        .hint("when idle")
                        .enabled(self.link.connected && !self.daemon_busy()),
                ];
                if !self.link.connected {
                    items.push(Item::new("Reconnect", 'o', Action::Reconnect).hint("^R"));
                }
                items.push(Item::separator());
                items.push(Item::note(format!(
                    "malac {} · daemon {}",
                    env!("CARGO_PKG_VERSION"),
                    self.daemon.version
                )));
                items
            }
        }
    }

    /// The daemons this terminal can control, remote access and the web UI.
    fn connection_menu(&self) -> Vec<Item> {
        let current = self.connection_name().map(str::to_owned);
        let saved = Connections::load().unwrap_or_default();
        let mut items = vec![
            Item::plain("This computer", Action::Connect(None))
                .hint("local")
                .checked(current.is_none()),
        ];
        for connection in &saved.connections {
            let address = connection
                .url
                .split_once("://")
                .map_or(connection.url.as_str(), |(_, rest)| rest);
            items.push(
                Item::plain(
                    connection.name.clone(),
                    Action::Connect(Some(connection.name.clone())),
                )
                .hint(&text::truncate(address, 34))
                .checked(current.as_deref() == Some(connection.name.as_str())),
            );
        }
        items.push(Item::separator());
        items.push(Item::new("Remote access…", 'r', Action::RemoteDialog));
        items.push(Item::new("Pair a device…", 'p', Action::PairDevice));
        let urls = self.backend.web_urls();
        items.push(Item::new("Show the web UI address", 's', Action::ShowWeb));
        items.push(
            Item::new("Copy the web UI address", 'c', Action::CopyWebUrl).enabled(!urls.is_empty()),
        );
        items.push(Item::separator());
        items.push(Item::note(match saved.connections.is_empty() {
            true => "add a daemon: malac remote add <name> <url> --pair <code>",
            false => "more: malac remote add <name> <url> --pair <code>",
        }));
        items
    }

    /// The View menu's voice section: what this terminal does with it, and
    /// the daemon's switches.
    fn voice_items(&self) -> Vec<Item> {
        use crate::voice::ready;
        let v = &self.voice;
        let mut items = vec![
            Item::separator(),
            Item::new("Dictate", 'd', Action::Dictate)
                .hint("^X")
                .enabled(ready(&v.dictation) && self.talk.is_none()),
            Item::new("Talk mode", 'o', Action::Talk)
                .hint("/talk")
                .checked(self.talk.is_some())
                .enabled(ready(&v.talk) || self.talk.is_some()),
            Item::new("Speak on this terminal", 'r', Action::Speak(!self.speak))
                .hint("/speak")
                .checked(self.speak),
        ];
        for (label, key, name, feature) in [
            ("Dictation", 'i', "dictation", &v.dictation),
            ("Speech", 'h', "speech", &v.speech),
            ("Talk", 'a', "talk", &v.talk),
        ] {
            let value = (!feature.enabled).to_string();
            items.push(
                Item::new(label, key, Action::SetVoice(name.to_owned(), value))
                    .hint(feature.model.as_deref().unwrap_or("no model"))
                    .checked(feature.enabled),
            );
        }
        items.push(Item::new("Voice settings…", 'v', Action::ShowVoice).hint("/voice"));
        items
    }

    /// One line on keep-awake for the View menu.
    fn awake_summary(&self) -> String {
        let a = &self.awake;
        match (a.awake, &a.error) {
            (_, Some(err)) => format!("not awake: {}", text::truncate(text::first_line(err), 40)),
            (true, None) => format!("awake: {}", a.reason),
            (false, None) => a.reason.clone(),
        }
    }

    fn agent_menu(&self) -> Vec<Item> {
        let (list, map) = match self.agents_view {
            AgentsView::List => ("F3", ""),
            AgentsView::Map => ("", "F3"),
        };
        let tab = self.tab();
        let mut items = vec![
            Item::new("Agent list…", 'l', Action::AgentList).hint(list),
            Item::new("Agent map…", 'm', Action::AgentMap).hint(map),
            Item::new("New agent in this tab", 'n', Action::NewAgent).hint("^N"),
            Item::new("Next agent", 'x', Action::StepAgent(1))
                .hint("Alt-↓")
                .enabled(tab.agents.len() > 1),
            Item::new("Close this agent", 'c', Action::CloseAgent)
                .enabled(tab.agents.len() > 1 && !self.busy()),
            Item::new("Back to parent", 'b', Action::Back)
                .hint("Esc")
                .enabled(!tab.view.is_empty()),
            Item::new("Stop", 's', Action::Stop)
                .hint("F8")
                .enabled(self.busy()),
        ];
        if tab.agents.len() > 1 {
            items.push(Item::separator());
            for agent in &tab.agents {
                let label = self.agent_label(agent);
                let checked = tab.agent.as_deref() == Some(agent.as_str());
                items
                    .push(Item::plain(label, Action::ShowTabAgent(agent.clone())).checked(checked));
            }
        }
        let subagents: Vec<(usize, &AgentView)> = self.tree(tab).into_iter().skip(1).collect();
        if !subagents.is_empty() {
            items.push(Item::separator());
            for (depth, view) in subagents.into_iter().take(12) {
                let label = format!("{}{}", "  ".repeat(depth - 1), view.label);
                items.push(Item::plain(label, Action::OpenAgent(view.id.clone())));
            }
        }
        items
    }

    /// An agent's name for menus and the switcher: its label, or its first prompt.
    pub fn agent_label(&self, id: &str) -> String {
        let view = self.agents.get(id);
        let prompt = view.and_then(|v| {
            v.transcript.items().iter().find_map(|i| match &i.entry {
                Entry::User(text) => {
                    Some(text::first_line(&attach::split_user_text(text).0).to_owned())
                }
                _ => None,
            })
        });
        let label = view.map(|v| v.label.clone()).unwrap_or_default();
        match prompt.filter(|p| !p.is_empty()) {
            Some(prompt) if label.is_empty() || label == "main" => {
                text::truncate(&prompt, 24).into_owned()
            }
            _ if label.is_empty() => "agent".to_owned(),
            _ => label,
        }
    }

    fn control_summary(&self) -> String {
        let c = &self.control;
        if let Some(err) = &c.error {
            return format!("control: {}", text::truncate(text::first_line(err), 40));
        }
        let state = |key: &str| match c.permissions.as_ref().map(|p| &p[key]) {
            Some(serde_json::Value::Bool(true)) => "granted",
            Some(serde_json::Value::Bool(false)) => "missing",
            _ => "n/a",
        };
        match (c.connected, &c.permissions) {
            (false, _) => "control: not connected".to_owned(),
            (true, None) => format!("control: {} tools", c.tools),
            (true, Some(_)) => format!(
                "screen {} · accessibility {}",
                state("screen_recording"),
                state("accessibility")
            ),
        }
    }

    /// Opens the repository the tab's folder is part of, when there is one.
    fn repository_item(&self) -> Item {
        let tab = self.tab();
        match tab
            .project
            .as_ref()
            .and_then(|p| p.layout.suggest_root.clone())
        {
            Some(root) => {
                Item::new("Open the whole repository", 'p', Action::OpenFolder(root)).hint("/root")
            }
            None => Item::new("Open the whole repository", 'p', Action::CloseDialog).enabled(false),
        }
    }

    fn tab_menu_items(&self) -> Vec<Item> {
        let mut items: Vec<Item> = (0..self.tabs.len())
            .map(|i| {
                let label = format!("{} {}", i + 1, self.tab_label(i));
                let key = char::from_digit((i + 1) as u32, 10).unwrap_or(' ');
                Item::new(label, key, Action::GoTab(i)).checked(i == self.active)
            })
            .collect();
        items.push(Item::separator());
        items.push(Item::new("New tab here", 'n', Action::NewTab).hint("^T"));
        items.push(Item::new("Open folder…", 'f', Action::FolderDialog));
        items.push(self.repository_item());
        items.push(Item::new("Rename tab…", 'r', Action::RenameTab));
        items.push(
            Item::new("Move left", 'l', Action::MoveTab(-1))
                .hint("^⇧PgUp")
                .enabled(self.active > 0),
        );
        items.push(
            Item::new("Move right", 'g', Action::MoveTab(1))
                .hint("^⇧PgDn")
                .enabled(self.active + 1 < self.tabs.len()),
        );
        items.push(
            Item::new("Close tab", 'w', Action::CloseTab(self.active))
                .hint("^W")
                .enabled(self.tabs.len() > 1),
        );
        items
    }

    /// The shown agent's model, or what the tab's next agent will use.
    pub fn current_model(&self) -> String {
        let tab = self.tab();
        match tab.root().and_then(|r| self.agents.get(r)) {
            Some(view) => view.model.clone(),
            None => tab
                .model
                .clone()
                .or_else(|| self.default_model.clone())
                .unwrap_or_default(),
        }
    }

    pub fn model_rows(&self) -> Vec<Row> {
        let current = self.current_model();
        let mut rows = Vec::new();
        let mut add = |group: &str, value: String, detail: &str| {
            if rows.iter().any(|r: &Row| r.value == value) {
                return;
            }
            let label = value.split_once('/').map_or(value.as_str(), |(_, m)| m);
            rows.push(Row {
                group: group.to_owned(),
                label: label.to_owned(),
                detail: detail.to_owned(),
                current: value == current,
                value,
            });
        };
        if self.discovered.is_empty() {
            for spec in &self.models {
                let provider = spec.split_once('/').map_or("", |(p, _)| p);
                add(provider, spec.clone(), "configured");
            }
        } else {
            for provider in &self.discovered {
                let group = match &provider.error {
                    Some(err) => format!(
                        "{} · {}",
                        provider.provider,
                        text::truncate(text::first_line(err), 50)
                    ),
                    None => provider.provider.clone(),
                };
                for model in provider.all() {
                    let detail = if provider.configured.contains(&model) {
                        "configured"
                    } else {
                        ""
                    };
                    add(&group, format!("{}/{model}", provider.provider), detail);
                }
            }
        }
        if !current.is_empty() && !rows.iter().any(|r| r.current) {
            let (provider, model) = current.split_once('/').unwrap_or(("", &current));
            rows.insert(
                0,
                Row {
                    group: provider.to_owned(),
                    label: model.to_owned(),
                    detail: "current".to_owned(),
                    value: current.clone(),
                    current: true,
                },
            );
        }
        rows
    }

    pub(super) fn session_rows(&self) -> Vec<Row> {
        self.sessions
            .iter()
            .map(|s| {
                let when = s
                    .updated
                    .with_timezone(&chrono::Local)
                    .format("%m-%d %H:%M");
                let title = if s.title.is_empty() {
                    "(no prompt yet)".to_owned()
                } else {
                    s.title.clone()
                };
                Row {
                    group: String::new(),
                    label: title,
                    detail: format!("{when}  {}", short_path(&s.meta.cwd)),
                    current: self
                        .tabs
                        .iter()
                        .any(|t| t.agents.iter().any(|a| *a == s.meta.id)),
                    value: s.meta.id.clone(),
                }
            })
            .collect()
    }

    /// Workspaces of this daemon and, under their own headers, of the
    /// others this terminal knows. A row's value is the workspace id, or
    /// `<connection>:<id>` on another daemon (`local` for this machine's).
    pub fn workspace_rows(&self) -> Vec<Row> {
        let row = |group: &str, w: &WorkspaceSummary, value: String, current: bool| {
            let tabs = if w.tabs == 1 { "tab" } else { "tabs" };
            let running = match w.running {
                0 => String::new(),
                n => format!(" · {n} running"),
            };
            let when = w
                .updated
                .with_timezone(&chrono::Local)
                .format("%m-%d %H:%M");
            Row {
                group: group.to_owned(),
                current,
                label: w.name.clone(),
                detail: format!("{} {tabs}{running} · {when}", w.tabs),
                value,
            }
        };
        let sorted = |list: &[WorkspaceSummary]| {
            let mut list = list.to_vec();
            list.sort_by(|a, b| b.updated.cmp(&a.updated));
            list
        };
        let others = !self.other_workspaces.is_empty();
        let here = match others {
            true => format!("{} (here)", self.backend.endpoint().label()),
            false => String::new(),
        };
        let mut rows: Vec<Row> = sorted(&self.workspaces)
            .iter()
            .map(|w| row(&here, w, w.id.clone(), self.is_ours(&w.id)))
            .collect();
        for (endpoint, result) in &self.other_workspaces {
            let name = endpoint.name().unwrap_or(connections::LOCAL);
            match result {
                Ok(list) => {
                    for w in sorted(list) {
                        rows.push(row(
                            &endpoint.label(),
                            &w,
                            format!("{name}:{}", w.id),
                            false,
                        ));
                    }
                }
                Err(err) => rows.push(Row {
                    group: format!("{} · {}", endpoint.label(), text::truncate(err, 40)),
                    label: "connect anyway".into(),
                    detail: String::new(),
                    value: format!("{name}:"),
                    current: false,
                }),
            }
        }
        rows
    }

    pub fn agent_rows(&self) -> Vec<Row> {
        let shown = self.tab().shown().map(str::to_owned);
        let mut rows = Vec::new();
        for (index, tab) in self.tabs.iter().enumerate() {
            let group = format!("{} {}", index + 1, self.tab_label(index));
            for root in &tab.agents {
                for (depth, view) in self.agents.tree(root) {
                    let snapshot = self.snapshots.get(&view.id);
                    let status = match snapshot.map(|s| s.status) {
                        Some(AgentStatus::Running) => {
                            let activity =
                                snapshot.map(|s| s.activity.as_str()).unwrap_or_default();
                            format!("{} {activity}", self.spinner())
                        }
                        Some(AgentStatus::Failed) => "failed".to_owned(),
                        Some(AgentStatus::Cancelled) => "cancelled".to_owned(),
                        Some(AgentStatus::Completed) => "done".to_owned(),
                        _ => "idle".to_owned(),
                    };
                    let label = match depth {
                        0 => self.agent_label(&view.id),
                        _ => view.label.clone(),
                    };
                    rows.push(Row {
                        group: group.clone(),
                        label: format!("{}{label}", "  ".repeat(depth)),
                        detail: format!("{status} · {}", view.model),
                        current: Some(&view.id) == shown.as_ref(),
                        value: view.id.clone(),
                    });
                }
            }
        }
        rows
    }

    fn open_models(&mut self) {
        let mut picker = Picker::new(
            PickKind::Model,
            "Model",
            "type to filter · enter switch · esc close",
            self.model_rows(),
        );
        if !self.discovering && self.discovered.is_empty() {
            self.discovering = true;
            self.backend.discover_models();
        }
        if self.discovering {
            picker.note = "asking providers…".to_owned();
        }
        self.overlay = Some(Overlay::Models(picker));
    }

    /// Shows an agent of this workspace, in the tab that holds it, opening
    /// subagents inside their top-level agent. Unknown agents are opened
    /// by the daemon.
    fn open_agent(&mut self, id: &str) {
        let Some(index) = self.tab_of_agent(id) else {
            self.backend.request(
                Command::AgentOpen {
                    agent: id.to_owned(),
                    tab: None,
                },
                Reply::ShowAgent,
            );
            return;
        };
        let root = self.agents.root_of(id).to_owned();
        if let Some(tab) = self.tabs[index].id.clone() {
            self.show_agent(&tab, &root);
        }
        let mut path = Vec::new();
        let mut at = id.to_owned();
        while at != root {
            path.push(at.clone());
            match self.agents.get(&at).and_then(|v| v.parent.clone()) {
                Some(parent) => at = parent,
                None => break,
            }
        }
        path.reverse();
        let tab = self.tab_mut();
        tab.reset_view();
        tab.view = path;
    }

    pub(super) fn copy(&mut self, text: &str) {
        if text.is_empty() {
            self.flash("nothing to copy");
            return;
        }
        term::copy(text);
        let lines = text.lines().count();
        self.flash_ok(match (lines, text.chars().count()) {
            (0 | 1, 1) => "Copied 1 character".to_owned(),
            (0 | 1, chars) => format!("Copied {chars} characters"),
            (lines, _) => format!("Copied {lines} lines"),
        });
    }

    fn open_agent_list(&mut self) {
        self.agents_view = AgentsView::List;
        let picker = Picker::new(
            PickKind::Agent,
            "Agents",
            "enter open · ^X stop · F3 map · esc close",
            self.agent_rows(),
        );
        self.overlay = Some(Overlay::Agents(picker));
    }

    fn open_agent_map(&mut self) {
        self.agents_view = AgentsView::Map;
        let shown = self.tab().shown().map(str::to_owned);
        self.overlay = Some(Overlay::AgentMap(AgentMap::new(self.agent_nodes(), shown)));
    }

    fn usage_lines(&self) -> Vec<Line<'static>> {
        let t = theme();
        let mut lines = Vec::new();
        for (depth, view) in self.tree(self.tab()) {
            let u = self.snapshots.get(&view.id).map_or(view.total, |s| s.usage);
            lines.push(Line::from(vec![
                Span::raw(format!("  {}{:<16}", "  ".repeat(depth), view.label)),
                Span::styled(
                    format!(
                        " in {} · cached {} · out {} · ${:.4}",
                        tokens(u.input_tokens),
                        tokens(u.cache_read_tokens),
                        tokens(u.output_tokens),
                        u.cost
                    ),
                    t.dim(),
                ),
            ]));
        }
        lines
    }

    fn web_lines(&self) -> Vec<Line<'static>> {
        let t = theme();
        self.backend
            .web_urls()
            .into_iter()
            .map(|url| {
                Line::from(vec![
                    Span::styled("  web UI  ", t.dim()),
                    Span::styled(text::hide_token(&url), t.fg(t.accent)),
                ])
            })
            .collect()
    }

    /// The tab's id, or a flash saying it is not ready.
    pub(super) fn tab_id(&mut self) -> Option<TabId> {
        let id = self.tab().id.clone();
        if id.is_none() {
            self.flash("this tab is still being created");
        }
        id
    }

    fn set_mode(&mut self, mode: Mode) {
        self.tab_mut().mode = mode;
        if let Some(tab) = self.tab().id.clone() {
            self.backend.command(Command::Mode {
                mode,
                tab: Some(tab),
            });
        }
        self.flash(format!("mode: {} for this tab", mode.as_str()));
    }

    pub async fn perform(&mut self, action: Action) {
        self.dirty = true;
        match action {
            Action::Help => {
                let commands = self.tab().commands().to_vec();
                self.overlay = Some(Overlay::Help(Help {
                    scroll: 0,
                    commands,
                }));
            }
            Action::Copy => {
                let answer = self
                    .shown()
                    .and_then(|v| v.transcript.last_answer())
                    .unwrap_or_default()
                    .to_owned();
                self.copy(&answer);
            }
            Action::Agents => match self.agents_view {
                AgentsView::List => self.open_agent_list(),
                AgentsView::Map => self.open_agent_map(),
            },
            Action::AgentList => self.open_agent_list(),
            Action::AgentMap => self.open_agent_map(),
            Action::FolderDialog => match self.connection_name() {
                // The browser lists this computer's folders, not the daemon's.
                Some(name) => {
                    self.flash(format!("type /cd <folder on {name}>, e.g. /cd ~/src/app"))
                }
                None => {
                    let browser = FolderBrowser::new(&self.tab_cwd(), self.recent_dirs());
                    self.overlay = Some(Overlay::Folder(browser));
                }
            },
            Action::OpenFolder(dir) => self.open_folder(&dir),
            Action::HideSuggestion => {
                let cwd = self.tab_cwd();
                self.hidden_suggestions.insert(cwd);
            }
            Action::Jobs => {
                self.overlay = Some(Overlay::Jobs(JobsView::default()));
                self.backend.request(Command::JobList, Reply::Jobs);
                self.refresh_job_log();
            }
            Action::StopJob(id) => {
                self.backend.command(Command::JobKill { job: id });
                self.flash("stopping the job");
            }
            Action::PasteImage => self.paste_image(),
            Action::CopySelection => self.copy_selection(),
            Action::PickModel => self.open_models(),
            Action::CycleMode => {
                let next = match self.tab().mode {
                    Mode::Ask => Mode::Edits,
                    Mode::Edits => Mode::Autopilot,
                    Mode::Autopilot => Mode::Ask,
                };
                self.set_mode(next);
            }
            Action::SetMode(mode) => self.set_mode(mode),
            Action::ShowMode => {
                let mode = self.tab().mode.as_str();
                self.notice(
                    Level::Info,
                    format!("mode: {mode} in this tab (F5 or Shift-Tab cycles)"),
                );
            }
            Action::TabMenu => {
                let items = self.tab_menu_items();
                self.overlay = Some(Overlay::Menu(Menu::new(Place::Tabs, items, true)));
            }
            Action::Sessions => {
                self.backend.sessions();
                let picker = Picker::new(
                    PickKind::Session,
                    "Open session",
                    "type to filter · enter open in its folder's tab · esc close",
                    self.session_rows(),
                );
                self.overlay = Some(Overlay::Sessions(picker));
            }
            Action::Stop => {
                if let Some(agent) = self.tab().root().map(str::to_owned).filter(|_| self.busy()) {
                    self.backend.command(Command::Cancel { agent });
                    let dropped = std::mem::take(&mut self.tab_mut().queued).len();
                    self.flash(match dropped {
                        0 => "stopping".to_owned(),
                        n => format!("stopping; dropped {n} queued"),
                    });
                }
            }
            Action::Menu => match &self.overlay {
                Some(Overlay::Menu(menu)) if matches!(menu.place, Place::Bar(_)) => {
                    self.overlay = None;
                }
                _ => self.open_menu(0, false),
            },
            // Agents keep working in the daemon.
            Action::Quit => self.quit = true,
            Action::NewTab => {
                let spec = TabSpec {
                    cwd: self.tab_cwd(),
                    workspace: self.workspace_id.clone(),
                    ..Default::default()
                };
                self.backend.request(
                    Command::TabCreate(spec),
                    Reply::TabCreated { placeholder: false },
                );
            }
            Action::CloseTab(index) => self.close_tab(index),
            Action::ForceCloseTab(tab) => {
                self.backend.command(Command::TabClose { tab, force: true });
            }
            Action::NextTab => self.switch_tab((self.active + 1) % self.tabs.len()),
            Action::PrevTab => {
                self.switch_tab((self.active + self.tabs.len() - 1) % self.tabs.len());
            }
            Action::GoTab(index) => self.switch_tab(index),
            Action::MoveTab(step) => {
                let index = self.active as isize + step;
                if let (Some(tab), true) = (
                    self.tab().id.clone(),
                    (0..self.tabs.len() as isize).contains(&index),
                ) {
                    self.backend.command(Command::TabMove {
                        tab,
                        index: index as usize,
                    });
                }
            }
            Action::RenameTab => {
                let text = self.tab_label(self.active);
                self.overlay = Some(Overlay::Rename(Rename {
                    ask: Ask::TabName,
                    text,
                }));
            }
            Action::NewAgent => {
                if let Some(tab) = self.tab_id() {
                    let model = self.tab_mut().model.take();
                    self.backend.request(
                        Command::AgentStart {
                            tab,
                            model,
                            label: None,
                        },
                        Reply::ShowAgent,
                    );
                }
            }
            Action::StepAgent(step) => {
                let tab = self.tab();
                let count = tab.agents.len() as isize;
                if count > 1 {
                    let at = tab
                        .root()
                        .and_then(|r| tab.agents.iter().position(|a| a == r))
                        .unwrap_or(0) as isize;
                    let next = tab.agents[(at + step).rem_euclid(count) as usize].clone();
                    if let Some(id) = tab.id.clone() {
                        self.show_agent(&id, &next);
                    }
                }
            }
            Action::ShowTabAgent(agent) => {
                if let Some(tab) = self.tab().id.clone() {
                    self.show_agent(&tab, &agent);
                }
            }
            Action::CloseAgent => {
                let tab = self.tab();
                if let (Some(id), Some(agent)) = (tab.id.clone(), tab.agent.clone()) {
                    self.backend.command(Command::AgentClose { tab: id, agent });
                }
            }
            Action::NewWorkspace => {
                self.overlay = Some(Overlay::Rename(Rename {
                    ask: Ask::NewWorkspace,
                    text: String::new(),
                }));
            }
            Action::Workspaces => {
                let picker = Picker::new(
                    PickKind::Workspace,
                    "Workspaces",
                    "type to filter · enter switch · esc close",
                    self.workspace_rows(),
                );
                self.overlay = Some(Overlay::Workspaces(picker));
                self.backend
                    .request(Command::WorkspaceList, Reply::Workspaces);
                self.backend.other_workspaces();
            }
            Action::OpenWorkspace(value) => match value.split_once(':') {
                Some((name, id)) => {
                    let name = (name != connections::LOCAL).then(|| name.to_owned());
                    let workspace = (!id.is_empty()).then(|| id.to_owned());
                    self.connect(name, workspace);
                }
                None if !self.is_ours(&value) => {
                    self.backend.request(
                        Command::WorkspaceOpen { workspace: value },
                        Reply::WorkspaceOpened { fresh: false },
                    );
                }
                None => {}
            },
            Action::RenameWorkspace => {
                self.overlay = Some(Overlay::Rename(Rename {
                    ask: Ask::WorkspaceName,
                    text: self.workspace_name.clone(),
                }));
            }
            Action::DeleteWorkspace => {
                if let Some(id) = self.workspace_id.clone() {
                    let working = self.tabs.iter().any(|t| self.tab_working(t));
                    let message = match working {
                        true => format!(
                            "Delete {}? Its agents are working and will be stopped.",
                            self.workspace_name
                        ),
                        false => format!(
                            "Delete {}? Its tabs close; the conversations stay saved.",
                            self.workspace_name
                        ),
                    };
                    self.overlay = Some(Overlay::Confirm(Confirm {
                        title: "Delete workspace".into(),
                        message,
                        label: " Delete  y ",
                        yes: Action::ForceDeleteWorkspace(id),
                    }));
                }
            }
            Action::ForceDeleteWorkspace(id) => {
                self.backend.command(Command::WorkspaceDelete {
                    workspace: id,
                    force: true,
                });
            }
            Action::Named(ask, name) => self.named(ask, name),
            Action::Compact => {
                if let Some(agent) = self.tab().root().map(str::to_owned) {
                    self.backend.command(Command::Compact { agent });
                    self.flash("compacting the conversation");
                }
            }
            Action::SetModel(spec) => match self.tab().root().map(str::to_owned) {
                Some(agent) => self.backend.command(Command::Model { agent, model: spec }),
                None => {
                    self.flash(format!("the tab's next agent uses {spec}"));
                    self.tab_mut().model = Some(spec);
                }
            },
            Action::Resume(id) => self.open_agent(&id),
            Action::OpenAgent(id) => {
                self.selection = None;
                self.open_agent(&id);
            }
            Action::StopAgent(id) => self.backend.command(Command::Cancel { agent: id }),
            Action::Back => {
                self.selection = None;
                let tab = self.tab_mut();
                tab.view.pop();
                tab.scroll = None;
                tab.selected = None;
            }
            Action::ToggleSidebar => self.sidebar = Some(!self.geometry.sidebar),
            Action::ToggleMouse => {
                self.mouse_capture = !self.mouse_capture;
                term::set_mouse_capture(self.mouse_capture);
                self.flash(if self.mouse_capture {
                    "mouse on"
                } else {
                    "mouse off: select text with the terminal · ^S turns it back on"
                });
            }
            Action::ToggleThinking => {
                let show = !self.agents.show_thinking;
                self.agents.show_thinking = show;
                let ids: Vec<AgentId> = self.snapshots.keys().cloned().collect();
                for id in ids {
                    if let Some(view) = self.agents.get_mut(&id) {
                        view.transcript.set_thinking_expanded(show);
                    }
                }
            }
            Action::ShowCost => {
                let lines = self.usage_lines();
                self.info(lines);
            }
            Action::ShowMcp => self.backend.tools(),
            Action::ShowMemory => self.backend.memory(),
            Action::ShowWeb => {
                let lines = self.web_lines();
                self.info(lines);
            }
            Action::CopyWebUrl => match self.backend.web_urls().into_iter().next() {
                Some(url) => {
                    term::copy(&url);
                    self.flash_ok(format!("copied {url}"));
                }
                None => self.flash("not connected to the daemon"),
            },
            Action::UpdateDialog => {
                self.overlay = Some(Overlay::Update(UpdateDialog {
                    installing: false,
                    result: None,
                }));
            }
            Action::CheckUpdate => {
                self.backend.check_update();
                if let Some(Overlay::Update(dialog)) = &mut self.overlay {
                    dialog.result = None;
                } else {
                    self.flash("checking for updates");
                }
            }
            Action::InstallUpdate => {
                if let Some(Overlay::Update(dialog)) = &mut self.overlay {
                    dialog.installing = true;
                    dialog.result = None;
                }
                self.backend.install_update();
            }
            Action::RestartDaemon => {
                if self.daemon_busy() {
                    self.flash("agents or jobs are running; restart the daemon when idle");
                } else {
                    self.backend.stop_daemon();
                }
            }
            Action::Reconnect => self.reconnect(true),
            Action::RequestPermissions => {
                self.backend.request(
                    Command::ControlPermissions { request: true },
                    Reply::Control,
                );
                self.flash("asking macOS; System Settings opens at what is missing");
            }
            Action::ShowApprovals => self.approvals_hidden = false,
            Action::Answer(decision) => {
                if let Some(approval) = self.approvals.pop_front() {
                    self.backend.request(
                        Command::Approve {
                            request_id: approval.request.id,
                            decision,
                        },
                        Reply::Approve,
                    );
                }
            }
            Action::CopyEntry(index) => {
                let text = self
                    .shown()
                    .and_then(|v| v.transcript.items().get(index))
                    .map(|i| i.entry.plain_text())
                    .unwrap_or_default();
                self.copy(&text);
            }
            Action::CopyCode(index, line) => {
                let code = self
                    .shown()
                    .and_then(|v| v.transcript.items().get(index))
                    .and_then(|i| i.code_block(line))
                    .unwrap_or_default()
                    .to_owned();
                self.copy(&code);
            }
            Action::ToggleEntry(index) => {
                if let Some(id) = self.tab().shown().map(str::to_owned) {
                    if let Some(view) = self.agents.get_mut(&id) {
                        view.transcript.toggle(index);
                    }
                }
                self.tab_mut().selected = Some(index);
            }
            Action::JumpBottom => self.tab_mut().scroll = None,
            Action::CloseDialog => self.overlay = None,
            Action::OpenMenu(index) => self.open_menu(index, true),
            Action::Connect(name) => {
                if name.as_deref() == self.connection_name() {
                    self.flash(format!("already on {}", self.backend.endpoint().label()));
                } else {
                    self.connect(name, None);
                }
            }
            Action::RemoteDialog => {
                if !matches!(self.overlay, Some(Overlay::Remote(_))) {
                    self.overlay = Some(Overlay::Remote(RemoteDialog::new()));
                }
                // Remote status and keep-awake arrive as broadcasts; devices are asked for.
                self.backend.devices();
            }
            Action::PairDevice => {
                match &mut self.overlay {
                    Some(Overlay::Remote(dialog)) => {
                        dialog.page = RemotePage::Pair;
                        dialog.pairing = None;
                    }
                    _ => {
                        let mut dialog = RemoteDialog::new();
                        dialog.page = RemotePage::Pair;
                        self.overlay = Some(Overlay::Remote(dialog));
                    }
                }
                self.backend.create_pairing();
            }
            Action::RevokeDevice(id) => {
                let name = match &self.overlay {
                    Some(Overlay::Remote(dialog)) => dialog
                        .devices
                        .as_ref()
                        .and_then(|d| d.as_ref().ok())
                        .and_then(|list| list.iter().find(|d| d.id == id))
                        .map(|d| (d.name.clone(), d.current)),
                    _ => None,
                };
                let (name, current) = name.unwrap_or_else(|| (id.clone(), false));
                let message = match current {
                    true => {
                        format!("Revoke {name}? That is this terminal: it disconnects at once.")
                    }
                    false => format!("Revoke {name}? Its token stops working and it disconnects."),
                };
                self.overlay = Some(Overlay::Confirm(Confirm {
                    title: "Revoke device".into(),
                    message,
                    label: " Revoke  y ",
                    yes: Action::RevokeNow(id),
                }));
            }
            Action::RevokeNow(id) => {
                self.backend.revoke(&id);
                self.overlay = Some(Overlay::Remote(RemoteDialog::new()));
                self.backend.devices();
            }
            Action::Dictate => self.dictate(),
            Action::Talk => self.toggle_talk(),
            Action::Speak(on) => self.set_speak(on),
            Action::ShowVoice => self.show_voice(),
            Action::SetVoice(key, value) => self.set_voice(&key, &value),
            Action::OpenFile(n) => self.open_nth(n),
            Action::OpenEntry(index, file) => self.open_entry(index, file),
            Action::KeepAwake(mode) => {
                self.backend.command(Command::KeepAwake { mode });
                self.flash(match mode {
                    KeepAwake::Busy => "keeping the machine awake while agents or jobs run",
                    KeepAwake::Always => "keeping the machine awake while the daemon runs",
                    KeepAwake::Off => "letting the machine sleep",
                });
            }
        }
    }

    /// Switches this terminal to another daemon, opening `workspace` there
    /// (else the one it used last).
    fn connect(&mut self, name: Option<String>, workspace: Option<String>) {
        let endpoint = match connections::resolve(name.as_deref()) {
            Ok(endpoint) => endpoint,
            Err(err) => return self.flash(format!("{err:#}")),
        };
        if workspace.is_none() && endpoint == *self.backend.endpoint() {
            return;
        }
        let cwd = match &endpoint {
            Endpoint::Local => std::env::current_dir().unwrap_or_else(|_| PathBuf::from("~")),
            Endpoint::Saved(_) => PathBuf::from("~"),
        };
        self.flash(format!("connecting to {}…", endpoint.label()));
        self.backend.switch(endpoint, workspace, cwd);
    }

    /// Closes a tab, asking first when its agents work.
    fn close_tab(&mut self, index: usize) {
        if self.tabs.len() == 1 {
            self.flash("this is the last tab · F10 quits");
            return;
        }
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let Some(id) = tab.id.clone() else {
            return;
        };
        if self.tab_working(tab) {
            self.overlay = Some(Overlay::Confirm(Confirm {
                title: "Close tab".into(),
                message: format!(
                    "Agents are working in {}. Close the tab and stop them?",
                    tab.name
                ),
                label: " Close and stop  y ",
                yes: Action::ForceCloseTab(id),
            }));
        } else {
            self.backend.command(Command::TabClose {
                tab: id,
                force: false,
            });
        }
    }

    fn named(&mut self, ask: Ask, name: String) {
        if name.is_empty() && ask != Ask::NewWorkspace {
            return;
        }
        match ask {
            Ask::TabName => {
                if let Some(tab) = self.tab_id() {
                    self.backend.command(Command::TabRename { tab, name });
                }
            }
            Ask::WorkspaceName => {
                if let Some(workspace) = self.workspace_id.clone() {
                    self.backend
                        .command(Command::WorkspaceRename { workspace, name });
                }
            }
            Ask::NewWorkspace => {
                let name = (!name.is_empty()).then_some(name);
                self.backend.request(
                    Command::WorkspaceCreate { name },
                    Reply::WorkspaceOpened { fresh: true },
                );
            }
        }
    }

    /// Works in `dir`: a tab for it in this workspace, the existing one if
    /// there is one.
    pub fn open_folder(&mut self, dir: &Path) {
        // A daemon elsewhere checks its own folders.
        if self.connection_name().is_none() && !dir.is_dir() {
            self.flash(format!("{} is not a folder", short_path(dir)));
            return;
        }
        let spec = TabSpec {
            cwd: dir.to_owned(),
            workspace: self.workspace_id.clone(),
            reuse: true,
            ..Default::default()
        };
        self.backend.request(
            Command::TabCreate(spec),
            Reply::TabCreated { placeholder: false },
        );
    }
}
