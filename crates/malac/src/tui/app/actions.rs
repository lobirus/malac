//! Everything the user can ask for, and the menus and lists that offer it.

use super::*;

/// Everything the user can ask for, from a key, a click, a menu or a command.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Help,
    Copy,
    Agents,
    PickModel,
    CycleMode,
    TabMenu,
    Sessions,
    Stop,
    Menu,
    Quit,
    NewTab,
    CloseTab(usize),
    NextTab,
    PrevTab,
    GoTab(usize),
    RenameTab,
    SetTabTitle(String),
    Compact,
    Clear,
    SetMode(Mode),
    ShowMode,
    SetModel(String),
    Resume(String),
    OpenAgent(AgentId),
    PinAgent(AgentId),
    StopAgent(AgentId),
    Back,
    ToggleSidebar,
    ToggleMouse,
    ToggleThinking,
    ShowCost,
    ShowMcp,
    ShowMemory,
    Web(bool),
    CopyWebUrl,
    UpdateDialog,
    CheckUpdate,
    InstallUpdate,
    ShowApprovals,
    Answer(Decision),
    CopyEntry(usize),
    CopyCode(usize, Option<usize>),
    ToggleEntry(usize),
    JumpBottom,
    CloseDialog,
}

impl App {
    /// The items of a menu bar title.
    pub fn menu_items(&self, index: usize) -> Vec<Item> {
        let mode = self.session.runtime.policy.mode();
        let read_only = self.tab().read_only();
        match index {
            0 => vec![
                Item::new("New tab", 'n', Action::NewTab).hint("^T"),
                Item::new("Rename tab…", 'r', Action::RenameTab),
                Item::new("Open session…", 'o', Action::Sessions).hint("F7"),
                Item::new("Compact conversation", 'c', Action::Compact).enabled(!read_only),
                Item::new("Start over", 's', Action::Clear).enabled(!read_only),
                Item::new("Close tab", 't', Action::CloseTab(self.active))
                    .hint("^W")
                    .enabled(self.tabs.len() > 1),
                Item::separator(),
                Item::new("Quit", 'q', Action::Quit).hint("F10"),
            ],
            1 => {
                let current = self.shown().map(|v| v.model.clone()).unwrap_or_default();
                let mut items = vec![Item::new("All models…", 'a', Action::PickModel).hint("F4")];
                let mut configured: Vec<String> = self
                    .session
                    .runtime
                    .config
                    .providers
                    .iter()
                    .flat_map(|(name, p)| p.models.iter().map(move |m| format!("{name}/{m}")))
                    .collect();
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
            ],
            3 => {
                let mut items = vec![
                    Item::new("Agents…", 'a', Action::Agents).hint("F3"),
                    Item::new("Back to parent", 'b', Action::Back)
                        .hint("Esc")
                        .enabled(!self.tab().view.is_empty()),
                    Item::new("Stop", 's', Action::Stop)
                        .hint("F8")
                        .enabled(self.busy()),
                ];
                let tree = self.agents.tree(self.tab().root());
                if tree.len() > 1 {
                    items.push(Item::separator());
                    for (depth, view) in tree.into_iter().skip(1).take(12) {
                        let label = format!("{}{}", "  ".repeat(depth - 1), view.label);
                        items.push(Item::plain(label, Action::OpenAgent(view.id.clone())));
                    }
                }
                items
            }
            4 => vec![
                Item::new("Sidebar", 's', Action::ToggleSidebar)
                    .hint("^G")
                    .checked(self.geometry.sidebar),
                Item::new("Mouse", 'm', Action::ToggleMouse)
                    .hint("^S")
                    .checked(self.mouse_capture),
                Item::new("Thinking", 't', Action::ToggleThinking)
                    .checked(self.agents.show_thinking),
                Item::separator(),
                Item::new("Usage and cost", 'u', Action::ShowCost),
                Item::new("MCP servers", 'c', Action::ShowMcp),
                Item::new("Memories", 'e', Action::ShowMemory),
            ],
            5 => {
                let serving = !self.session.web_urls.is_empty();
                let mut items = vec![
                    Item::new("Serve on this machine", 's', Action::Web(false)).enabled(!serving),
                    Item::new("Serve on the network", 'n', Action::Web(true)).enabled(!serving),
                    Item::new("Copy URL", 'c', Action::CopyWebUrl).enabled(serving),
                ];
                if let Some(url) = self.session.web_urls.first() {
                    items.push(Item::separator());
                    items.push(Item::note(text::truncate(url, 48).into_owned()));
                }
                items
            }
            _ => vec![
                Item::new("Keys and commands", 'k', Action::Help).hint("F1"),
                Item::new("Check for updates", 'c', Action::CheckUpdate),
                Item::new("Install update", 'i', Action::UpdateDialog)
                    .enabled(dialog::can_install(&self.update)),
                Item::separator(),
                Item::note(format!("malac {}", env!("CARGO_PKG_VERSION"))),
            ],
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
        items.push(Item::new("New tab", 'n', Action::NewTab).hint("^T"));
        items.push(Item::new("Rename tab…", 'r', Action::RenameTab));
        items.push(
            Item::new("Close tab", 'w', Action::CloseTab(self.active))
                .hint("^W")
                .enabled(self.tabs.len() > 1),
        );
        items
    }

    pub fn model_rows(&self) -> Vec<Row> {
        let config = &self.session.runtime.config;
        let current = self.shown().map(|v| v.model.clone()).unwrap_or_default();
        let mut rows = Vec::new();
        let mut add = |group: &str, provider: &str, model: &str, detail: &str| {
            let value = format!("{provider}/{model}");
            if rows.iter().any(|r: &Row| r.value == value) {
                return;
            }
            rows.push(Row {
                group: group.to_owned(),
                label: model.to_owned(),
                detail: detail.to_owned(),
                current: value == current,
                value,
            });
        };
        if self.models.is_empty() {
            for (name, provider) in &config.providers {
                for model in &provider.models {
                    add(name, name, model, "configured");
                }
            }
        } else {
            for provider in &self.models {
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
                    add(&group, &provider.provider, &model, detail);
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

    fn session_rows(&self) -> Vec<Row> {
        let home = dirs_home();
        session::list(false, 200)
            .unwrap_or_default()
            .into_iter()
            .map(|s| {
                let cwd = s.meta.cwd.display().to_string();
                let cwd = match &home {
                    Some(home) => cwd.replacen(home.as_str(), "~", 1),
                    None => cwd,
                };
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
                    detail: format!("{when}  {cwd}"),
                    current: self.tabs.iter().any(|t| t.root() == s.meta.id),
                    value: s.meta.id,
                }
            })
            .collect()
    }

    pub fn agent_rows(&self) -> Vec<Row> {
        let shown = self.tab().shown().to_owned();
        let mut rows = Vec::new();
        for (index, tab) in self.tabs.iter().enumerate() {
            let group = format!("{} {}", index + 1, self.tab_label(index));
            for (depth, view) in self.agents.tree(tab.root()) {
                let snapshot = self.snapshots.get(&view.id);
                let status = match snapshot.map(|s| s.status) {
                    Some(AgentStatus::Running) => {
                        let activity = snapshot.map(|s| s.activity.as_str()).unwrap_or_default();
                        format!("{} {activity}", self.spinner())
                    }
                    Some(AgentStatus::Failed) => "failed".to_owned(),
                    Some(AgentStatus::Cancelled) => "cancelled".to_owned(),
                    Some(AgentStatus::Completed) => "done".to_owned(),
                    _ => "idle".to_owned(),
                };
                rows.push(Row {
                    group: group.clone(),
                    label: format!("{}{}", "  ".repeat(depth), view.label),
                    detail: format!("{status} · {}", view.model),
                    current: view.id == shown,
                    value: view.id.clone(),
                });
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
        if !self.discovering {
            self.discovering = true;
            let config = self.session.runtime.config.clone();
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let models = malac_core::provider::discover_models(&config).await;
                let _ = tx.send(Update::Models(models));
            });
        }
        if self.discovering {
            picker.note = "asking providers…".to_owned();
        }
        self.overlay = Some(Overlay::Models(picker));
    }

    /// Opens a saved session, reusing an empty tab or the tab showing it.
    fn open_session(&mut self, id: &str) {
        if let Some(index) = self.tabs.iter().position(|t| t.root() == id) {
            self.switch_tab(index);
            return;
        }
        let handle = match self.session.runtime.resume_agent(id) {
            Ok(handle) => handle,
            Err(err) => {
                self.notice(Level::Error, format!("{err:#}"));
                return;
            }
        };
        self.ensure_view(&handle);
        let fresh = {
            let tab = self.tab();
            !tab.read_only()
                && !tab.handle.is_busy()
                && tab.title.is_none()
                && self
                    .agents
                    .get(tab.root())
                    .is_none_or(|v| v.transcript.is_empty())
        };
        if fresh {
            let tab = self.tab_mut();
            tab.handle = handle;
            tab.reset_view();
            self.switch_tab(self.active);
        } else {
            let index = self.open_tab(handle);
            self.switch_tab(index);
        }
    }

    /// Shows an agent: inside the tab that owns it, else in a new tab.
    fn open_agent(&mut self, id: &str) {
        let root = self.agents.root_of(id).to_owned();
        let index = match self.tabs.iter().position(|t| t.root() == root) {
            Some(index) => index,
            None => match self.session.runtime.agent(&root) {
                Some(handle) => self.open_tab(handle),
                None => return,
            },
        };
        self.switch_tab(index);
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

    fn copy(&mut self, text: &str) {
        if text.is_empty() {
            self.flash("nothing to copy");
            return;
        }
        term::copy(text);
        let lines = text.lines().count();
        let noun = if lines == 1 { "line" } else { "lines" };
        self.flash(format!("copied {lines} {noun}"));
    }

    fn usage_lines(&self) -> Vec<Line<'static>> {
        let t = theme();
        let mut lines = Vec::new();
        for (depth, view) in self.agents.tree(self.tab().root()) {
            let u = view.total;
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

    fn mcp_lines(&self) -> Vec<Line<'static>> {
        let t = theme();
        let servers = self.session.runtime.servers();
        if servers.is_empty() {
            return vec![Line::styled("  no MCP servers configured", t.dim())];
        }
        servers
            .into_iter()
            .map(|server| match server.error {
                Some(err) => Line::from(vec![
                    Span::styled("  ✗ ", t.fg(t.error)),
                    Span::raw(server.name),
                    Span::styled(format!("  {}", text::first_line(&err)), t.dim()),
                ]),
                None => Line::from(vec![
                    Span::styled("  ● ", t.fg(t.accent)),
                    Span::raw(server.name),
                    Span::styled(format!("  {} tools", server.tools), t.dim()),
                ]),
            })
            .collect()
    }

    fn memory_lines(&self) -> Vec<Line<'static>> {
        let t = theme();
        let entries = self.session.runtime.memory.list();
        if entries.is_empty() {
            return vec![Line::styled("  no memories saved", t.dim())];
        }
        entries
            .into_iter()
            .map(|entry| {
                Line::from(vec![
                    Span::raw(format!("  {}", entry.name)),
                    Span::styled(
                        format!("  {:?} · {}", entry.scope, entry.description),
                        t.dim(),
                    ),
                ])
            })
            .collect()
    }

    pub async fn perform(&mut self, action: Action) {
        self.dirty = true;
        let runtime = self.session.runtime.clone();
        match action {
            Action::Help => self.overlay = Some(Overlay::Help(Help { scroll: 0 })),
            Action::Copy => {
                let answer = self
                    .shown()
                    .and_then(|v| v.transcript.last_answer())
                    .unwrap_or_default()
                    .to_owned();
                self.copy(&answer);
            }
            Action::Agents => {
                let picker = Picker::new(
                    PickKind::Agent,
                    "Agents",
                    "enter open · tab open in a new tab · ^X stop · esc close",
                    self.agent_rows(),
                );
                self.overlay = Some(Overlay::Agents(picker));
            }
            Action::PickModel => self.open_models(),
            Action::CycleMode => {
                let next = match runtime.policy.mode() {
                    Mode::Ask => Mode::Edits,
                    Mode::Edits => Mode::Autopilot,
                    Mode::Autopilot => Mode::Ask,
                };
                runtime.policy.set_mode(next);
                self.flash(format!("mode: {}", next.as_str()));
            }
            Action::SetMode(mode) => {
                runtime.policy.set_mode(mode);
                self.flash(format!("mode: {}", mode.as_str()));
            }
            Action::ShowMode => {
                let mode = runtime.policy.mode().as_str();
                self.notice(
                    Level::Info,
                    format!("mode: {mode} (F5 or Shift-Tab cycles)"),
                );
            }
            Action::TabMenu => {
                let items = self.tab_menu_items();
                self.overlay = Some(Overlay::Menu(Menu::new(Place::Tabs, items, true)));
            }
            Action::Sessions => {
                let picker = Picker::new(
                    PickKind::Session,
                    "Open session",
                    "type to filter · enter open in a tab · esc close",
                    self.session_rows(),
                );
                self.overlay = Some(Overlay::Sessions(picker));
            }
            Action::Stop => {
                if self.busy() {
                    runtime.cancel_tree(self.tab().root());
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
            Action::Quit => {
                let armed = self
                    .quit_armed
                    .is_some_and(|at| at.elapsed() < DOUBLE_PRESS);
                if self.running_count() > 0 && !armed {
                    self.quit_armed = Some(Instant::now());
                    self.flash("agents are still working · quit again to stop them and exit");
                } else {
                    self.quit = true;
                }
            }
            Action::NewTab => match runtime.spawn_agent(AgentSpec::default()) {
                Ok(handle) => {
                    let index = self.open_tab(handle);
                    self.switch_tab(index);
                }
                Err(err) => self.notice(Level::Error, format!("{err:#}")),
            },
            Action::CloseTab(index) => {
                if self.tabs.len() == 1 {
                    self.flash("this is the last tab · F10 quits");
                } else if index < self.tabs.len() {
                    if index != self.active {
                        let draft = self.editor.text().to_owned();
                        self.tabs[self.active].draft = draft;
                    }
                    self.tabs.remove(index);
                    self.active = if self.active > index {
                        self.active - 1
                    } else {
                        self.active.min(self.tabs.len() - 1)
                    };
                    let draft = std::mem::take(&mut self.tabs[self.active].draft);
                    self.editor.set_text(&draft);
                    self.switch_tab(self.active);
                }
            }
            Action::NextTab => self.switch_tab((self.active + 1) % self.tabs.len()),
            Action::PrevTab => {
                self.switch_tab((self.active + self.tabs.len() - 1) % self.tabs.len());
            }
            Action::GoTab(index) => self.switch_tab(index),
            Action::RenameTab => {
                let text = self.tab_label(self.active);
                self.overlay = Some(Overlay::Rename(Rename { text }));
            }
            Action::SetTabTitle(title) => {
                self.tab_mut().title = (!title.is_empty()).then_some(title);
            }
            Action::Compact => {
                if !self.tab().read_only() {
                    self.tab().handle.compact();
                    self.flash("compacting the conversation");
                }
            }
            Action::Clear => {
                if self.tab().read_only() {
                    return;
                }
                match runtime.spawn_agent(AgentSpec::default()) {
                    Ok(handle) => {
                        self.ensure_view(&handle);
                        let tab = self.tab_mut();
                        tab.handle = handle;
                        tab.title = None;
                        tab.queued.clear();
                        tab.reset_view();
                        self.switch_tab(self.active);
                    }
                    Err(err) => self.notice(Level::Error, format!("{err:#}")),
                }
            }
            Action::SetModel(spec) => self.tab().handle.set_model(&spec),
            Action::Resume(id) => self.open_session(&id),
            Action::OpenAgent(id) => self.open_agent(&id),
            Action::PinAgent(id) => {
                if let Some(handle) = runtime.agent(&id) {
                    let index = match self.tabs.iter().position(|t| t.root() == id) {
                        Some(index) => index,
                        None => self.open_tab(handle),
                    };
                    self.switch_tab(index);
                }
            }
            Action::StopAgent(id) => runtime.cancel_tree(&id),
            Action::Back => {
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
            Action::ShowMcp => {
                let lines = self.mcp_lines();
                self.info(lines);
            }
            Action::ShowMemory => {
                let lines = self.memory_lines();
                self.info(lines);
            }
            Action::Web(remote) => match self.session.serve_web(remote, None).await {
                Ok(()) => {
                    let t = theme();
                    let lines = self
                        .session
                        .web_urls
                        .iter()
                        .map(|url| {
                            Line::from(vec![
                                Span::styled("  web UI  ", t.dim()),
                                Span::styled(url.clone(), t.fg(t.accent)),
                            ])
                        })
                        .collect();
                    self.info(lines);
                }
                Err(err) => self.notice(Level::Error, format!("web UI: {err:#}")),
            },
            Action::CopyWebUrl => match self.session.web_urls.first().cloned() {
                Some(url) => {
                    term::copy(&url);
                    self.flash(format!("copied {url}"));
                }
                None => self.flash("the web UI is not serving · the Web menu starts it"),
            },
            Action::UpdateDialog => {
                self.overlay = Some(Overlay::Update(UpdateDialog {
                    installing: false,
                    result: None,
                }));
            }
            Action::CheckUpdate => {
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let result = update::check(true).await.map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Update::Checked(result));
                });
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
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let result = runtime.install_update().await.map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Update::Installed(result));
                });
            }
            Action::ShowApprovals => self.approvals_hidden = false,
            Action::Answer(decision) => {
                if let Some(mut approval) = self.approvals.pop_front() {
                    approval.answer(decision);
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
                let id = self.tab().shown().to_owned();
                if let Some(view) = self.agents.get_mut(&id) {
                    view.transcript.toggle(index);
                }
                self.tab_mut().selected = Some(index);
            }
            Action::JumpBottom => self.tab_mut().scroll = None,
            Action::CloseDialog => self.overlay = None,
        }
    }
}
