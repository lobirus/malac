//! The interface's one way to the daemon. Commands go over the client's
//! WebSocket and REST calls over its HTTP API, each in a task of its own so
//! the screen never waits; replies come back as [`Update`]s that name what
//! they answer. A lost connection is retried from here too.

#[cfg(test)]
use std::sync::Mutex;

use malac_core::event::AgentId;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::*;
use crate::client::{self, DaemonFile, Launched};
use crate::connections::{Connections, Endpoint};
use crate::daemon::ServeArgs;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::protocol::{AttachmentInput, Command, ImageInput, Launch, WorkspaceChoice};

/// A connection the event loop takes over: the client, its events and the
/// snapshot that opened it. `switched` is set when it is to another daemon,
/// with what was opened there.
pub struct Connection {
    pub client: Client,
    pub events: Events,
    pub snapshot: Snapshot,
    pub switched: Option<(Endpoint, Launched)>,
}

/// What a command's reply is for.
#[derive(Debug, Clone)]
pub enum Reply {
    /// Only failures matter.
    Quiet,
    /// A prompt; on failure it goes back to the editor.
    Prompt {
        tab: TabId,
        prompt: Prompt,
    },
    /// An agent this terminal started or opened, to show.
    ShowAgent,
    /// A tab this terminal asked for; `placeholder` when an empty
    /// workspace's stand-in tab waits for it.
    TabCreated {
        placeholder: bool,
    },
    /// A workspace to switch to; `fresh` when this terminal created it.
    WorkspaceOpened {
        fresh: bool,
    },
    Workspaces,
    Snapshot,
    Jobs,
    JobOutput {
        job: String,
    },
    Control,
    /// An answer to an approval; another client may have answered first.
    Approve,
}

/// What a REST answer is for.
#[derive(Debug, Clone)]
pub enum Rest {
    History { agent: AgentId },
    Sessions,
    Models,
    Tools,
    Memory,
    UpdateChecked,
    UpdateInstalled,
    DaemonStopped,
    Devices,
    Pairing,
    Revoked,
}

pub struct Backend {
    client: Option<Arc<Client>>,
    /// The daemon this terminal talks to.
    endpoint: Endpoint,
    updates: mpsc::UnboundedSender<Update>,
    connections: mpsc::UnboundedSender<Connection>,
    /// Commands and REST calls made without a daemon, for tests.
    #[cfg(test)]
    pub sent: Arc<Mutex<Vec<Command>>>,
    #[cfg(test)]
    pub rests: Arc<Mutex<Vec<Rest>>>,
}

impl Backend {
    pub fn new(
        client: Client,
        endpoint: Endpoint,
        updates: mpsc::UnboundedSender<Update>,
        connections: mpsc::UnboundedSender<Connection>,
    ) -> Self {
        Self {
            client: Some(Arc::new(client)),
            endpoint,
            updates,
            connections,
            #[cfg(test)]
            sent: Arc::default(),
            #[cfg(test)]
            rests: Arc::default(),
        }
    }

    /// A backend without a daemon that records what it is asked to send.
    #[cfg(test)]
    pub fn recording() -> (Self, mpsc::UnboundedReceiver<Update>) {
        let (updates, received) = mpsc::unbounded_channel();
        let (connections, _) = mpsc::unbounded_channel();
        let backend = Self {
            client: None,
            endpoint: Endpoint::Local,
            updates,
            connections,
            sent: Arc::default(),
            rests: Arc::default(),
        };
        (backend, received)
    }

    pub fn updates(&self) -> mpsc::UnboundedSender<Update> {
        self.updates.clone()
    }

    pub fn file(&self) -> Option<&DaemonFile> {
        self.client.as_deref().map(Client::file)
    }

    /// The web UI's addresses, with the token.
    pub fn web_urls(&self) -> Vec<String> {
        self.file().map(|f| f.urls.clone()).unwrap_or_default()
    }

    pub fn connect(&mut self, client: Client) {
        self.client = Some(Arc::new(client));
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Talks to another daemon from now on.
    pub fn switch_to(&mut self, client: Client, endpoint: Endpoint) {
        self.client = Some(Arc::new(client));
        self.endpoint = endpoint;
    }

    pub fn disconnect(&mut self) {
        self.client = None;
    }

    /// Sends a command; the reply comes back as [`Update::Reply`].
    pub fn request(&self, command: Command, reply: Reply) {
        let Some(client) = self.client.clone() else {
            #[cfg(test)]
            {
                self.sent.lock().unwrap().push(command);
                drop(reply);
            }
            #[cfg(not(test))]
            {
                drop(command);
                let error = Err("not connected to the daemon".into());
                let _ = self.updates.send(Update::Reply(reply, error));
            }
            return;
        };
        let updates = self.updates.clone();
        tokio::spawn(async move {
            let result = client.request(command).await.map_err(|e| format!("{e:#}"));
            let _ = updates.send(Update::Reply(reply, result));
        });
    }

    /// A command whose reply only matters when it fails.
    pub fn command(&self, command: Command) {
        self.request(command, Reply::Quiet);
    }

    fn get(&self, path: String, rest: Rest) {
        let Some(client) = self.client.clone() else {
            #[cfg(test)]
            self.rests.lock().unwrap().push(rest);
            return;
        };
        let updates = self.updates.clone();
        tokio::spawn(async move {
            let result = client
                .get::<Value>(&path)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = updates.send(Update::Rest(rest, result));
        });
    }

    fn delete(&self, path: String, rest: Rest) {
        let Some(client) = self.client.clone() else {
            #[cfg(test)]
            self.rests.lock().unwrap().push(rest);
            return;
        };
        let updates = self.updates.clone();
        tokio::spawn(async move {
            let result = client
                .delete::<Value>(&path)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = updates.send(Update::Rest(rest, result));
        });
    }

    pub fn devices(&self) {
        self.get("/api/devices".into(), Rest::Devices);
    }

    pub fn create_pairing(&self) {
        self.post("/api/pair/codes", json!({}), Rest::Pairing);
    }

    pub fn revoke(&self, id: &str) {
        self.delete(format!("/api/devices/{id}"), Rest::Revoked);
    }

    /// Asks every other known daemon for its workspaces, for the switcher.
    pub fn other_workspaces(&self) {
        let mut others: Vec<Endpoint> = Connections::load()
            .map(|saved| saved.connections.into_iter().map(Endpoint::Saved).collect())
            .unwrap_or_default();
        others.insert(0, Endpoint::Local);
        others.retain(|e| e != &self.endpoint);
        for endpoint in others {
            let updates = self.updates.clone();
            tokio::spawn(async move {
                let file = match &endpoint {
                    Endpoint::Local => client::running().await,
                    Endpoint::Saved(c) => Some(c.file()),
                };
                let result = match file {
                    None => Err("not running".to_owned()),
                    Some(file) => {
                        let ask = client::get::<Value>(&file, "/api/workspaces");
                        match tokio::time::timeout(std::time::Duration::from_secs(4), ask).await {
                            Ok(Ok(value)) => serde_json::from_value(value["workspaces"].clone())
                                .map_err(|e| e.to_string()),
                            Ok(Err(err)) => Err(text::first_line(&format!("{err:#}")).to_owned()),
                            Err(_) => Err("no answer".to_owned()),
                        }
                    }
                };
                let _ = updates.send(Update::OtherWorkspaces(endpoint, result));
            });
        }
    }

    /// Connects to another daemon and opens a workspace there: `workspace`,
    /// else the one it used last. The event loop takes the connection over.
    pub fn switch(&self, endpoint: Endpoint, workspace: Option<String>, cwd: PathBuf) {
        let connections = self.connections.clone();
        let updates = self.updates.clone();
        tokio::spawn(async move {
            let result = async {
                let (client, events, snapshot) = endpoint.connect().await?;
                let launched = client
                    .launch(Launch {
                        workspace: workspace.map_or(WorkspaceChoice::Last, WorkspaceChoice::Open),
                        cwd,
                        ..Default::default()
                    })
                    .await?;
                anyhow::Ok(Connection {
                    client,
                    events,
                    snapshot,
                    switched: Some((endpoint, launched)),
                })
            }
            .await;
            match result {
                Ok(connection) => {
                    let _ = connections.send(connection);
                }
                Err(err) => {
                    let _ = updates.send(Update::SwitchFailed(format!("{err:#}")));
                }
            }
        });
    }

    fn post(&self, path: &'static str, body: Value, rest: Rest) {
        let Some(client) = self.client.clone() else {
            #[cfg(test)]
            self.rests.lock().unwrap().push(rest);
            return;
        };
        let updates = self.updates.clone();
        tokio::spawn(async move {
            let result = client
                .post::<Value>(path, &body)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = updates.send(Update::Rest(rest, result));
        });
    }

    /// Reads an agent's saved conversation; returns whether it will arrive.
    pub fn history(&self, agent: &str) -> bool {
        if self.client.is_none() && !cfg!(test) {
            return false;
        }
        let rest = Rest::History {
            agent: agent.to_owned(),
        };
        self.get(format!("/api/sessions/{agent}"), rest);
        true
    }

    /// Asks the daemon for saved sessions, newest first.
    pub fn sessions(&self) {
        self.get("/api/sessions?limit=200".into(), Rest::Sessions);
    }

    pub fn discover_models(&self) {
        self.get("/api/models".into(), Rest::Models);
    }

    pub fn tools(&self) {
        self.get("/api/tools".into(), Rest::Tools);
    }

    pub fn memory(&self) {
        self.get("/api/memory".into(), Rest::Memory);
    }

    pub fn check_update(&self) {
        self.post("/api/update/check", json!({}), Rest::UpdateChecked);
    }

    pub fn install_update(&self) {
        self.post("/api/update/install", json!({}), Rest::UpdateInstalled);
    }

    /// Asks the daemon to stop; it refuses while agents or jobs run.
    pub fn stop_daemon(&self) {
        self.post(
            "/api/daemon/stop",
            json!({ "force": false }),
            Rest::DaemonStopped,
        );
    }

    /// Connects again in the background. With `start`, a local daemon is
    /// started if none runs; otherwise it waits for one to appear. A daemon
    /// elsewhere is retried for a minute.
    pub fn reconnect(&self, start: bool) {
        let connections = self.connections.clone();
        let updates = self.updates.clone();
        let endpoint = self.endpoint.clone();
        tokio::spawn(async move {
            let mut last_error = String::new();
            for attempt in 0.. {
                if attempt > 0 {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                let connected = match (&endpoint, start) {
                    (Endpoint::Saved(_), _) if attempt >= 30 => break,
                    (Endpoint::Saved(c), _) => Client::connect(&c.file()).await,
                    // A stopping daemon may still hold the lock for a moment.
                    (Endpoint::Local, true) if attempt < 5 => {
                        Client::connect_or_start(&ServeArgs::default()).await
                    }
                    (Endpoint::Local, true) => break,
                    (Endpoint::Local, false) => match client::running().await {
                        Some(file) => Client::connect(&file).await,
                        None => continue,
                    },
                };
                match connected {
                    Ok((client, events, snapshot)) => {
                        let _ = connections.send(Connection {
                            client,
                            events,
                            snapshot,
                            switched: None,
                        });
                        return;
                    }
                    Err(err) => last_error = format!("{err:#}"),
                }
            }
            let _ = updates.send(Update::Unreachable(last_error));
        });
    }
}

impl App {
    /// Sends the prompt at the head of a tab's queue, to its shown agent or
    /// to a new one when the tab has none yet.
    pub(super) fn dispatch(&mut self, index: usize, prompt: Prompt) {
        let tab = &mut self.tabs[index];
        let Some(id) = tab.id.clone() else {
            tab.queued.push_front(prompt);
            return;
        };
        tab.sending = true;
        let model = match tab.agent {
            None => tab.model.take(),
            Some(_) => None,
        };
        // Pasted images travel with the prompt, so a daemon elsewhere gets them too.
        let (pasted, files): (Vec<&Attachment>, Vec<&Attachment>) =
            prompt.attachments.iter().partition(|a| a.inline.is_some());
        let command = Command::Prompt {
            tab: Some(id.clone()),
            agent: tab.agent.clone(),
            text: prompt.text.clone(),
            images: pasted
                .iter()
                .filter_map(|a| {
                    Some(ImageInput {
                        media_type: "image/png".into(),
                        data: BASE64.encode(&a.inline.as_ref()?.0),
                        name: Some(a.name.clone()),
                    })
                })
                .collect(),
            attachments: files
                .iter()
                .map(|a| AttachmentInput {
                    path: a.path.display().to_string(),
                })
                .collect(),
            model,
        };
        self.backend
            .request(command, Reply::Prompt { tab: id, prompt });
    }

    /// Shows an agent of a tab here, and tells the daemon it is the tab's
    /// active one so `malac` returns to it.
    pub(super) fn show_agent(&mut self, tab: &str, agent: &str) {
        let Some(index) = self.tab_index(tab) else {
            return;
        };
        self.switch_tab(index);
        let tab = self.tab_mut();
        if tab.agent.as_deref() != Some(agent) && tab.agents.iter().any(|a| a == agent) {
            tab.agent = Some(agent.to_owned());
            tab.reset_view();
        } else if tab.agent.is_none() {
            tab.agent = Some(agent.to_owned());
        }
        self.selection = None;
        if let Some(tab) = self.tab().id.clone() {
            self.backend.command(Command::AgentActivate {
                tab,
                agent: agent.to_owned(),
            });
        }
    }

    pub(super) fn on_reply(&mut self, reply: Reply, result: Result<ServerMessage, String>) {
        let message = match result {
            Ok(message) => message,
            Err(err) => return self.reply_failed(reply, err),
        };
        match (reply, message) {
            (
                Reply::Prompt { tab, .. },
                ServerMessage::Ack {
                    agent: Some(agent), ..
                },
            ) => {
                if let Some(index) = self.tab_index(&tab) {
                    let tab = &mut self.tabs[index];
                    if tab.agent.is_none() {
                        tab.agent = Some(agent);
                    }
                }
            }
            (
                Reply::ShowAgent,
                ServerMessage::Ack {
                    agent: Some(agent),
                    tab: Some(tab),
                    ..
                },
            ) => {
                if self.tab_index(&tab).is_some() {
                    self.show_agent(&tab, &agent);
                } else {
                    // The daemon opened it in a tab this terminal has not heard of yet.
                    self.resync();
                }
            }
            (Reply::TabCreated { placeholder }, ServerMessage::Ack { tab: Some(tab), .. }) => {
                self.created(tab, placeholder);
            }
            (
                Reply::WorkspaceOpened { fresh },
                ServerMessage::WorkspaceOpened { workspace, .. },
            ) => {
                self.opened_workspace(&workspace, fresh);
            }
            (Reply::Workspaces, ServerMessage::Workspaces { workspaces, .. }) => {
                self.workspaces = workspaces;
                let rows = self.workspace_rows();
                if let Some(Overlay::Workspaces(picker)) = &mut self.overlay {
                    picker.set_rows(rows);
                }
            }
            (Reply::Snapshot, ServerMessage::Snapshot(snapshot)) => self.apply_snapshot(*snapshot),
            (Reply::Jobs, ServerMessage::Jobs { jobs, .. }) => self.jobs = jobs,
            (Reply::JobOutput { job }, ServerMessage::JobOutput { output, .. }) => {
                if let Some(Overlay::Jobs(view)) = &mut self.overlay {
                    view.log = Some((job, output.text));
                }
            }
            (Reply::Control, ServerMessage::Control { control, .. }) => {
                self.control = control;
                match self.control_issue() {
                    Some(issue) => self.flash(format!("{issue}: see System Settings")),
                    None => self.flash_ok("computer control has what it needs"),
                }
            }
            _ => {}
        }
    }

    fn reply_failed(&mut self, reply: Reply, err: String) {
        match reply {
            Reply::Prompt { tab, prompt } => {
                if let Some(index) = self.tab_index(&tab) {
                    self.tabs[index].sending = false;
                    // A prompt the daemon refused goes back to be fixed and sent again.
                    if index == self.active && self.editor.is_empty() {
                        self.editor.set_text(&prompt.text);
                        self.attachments = prompt.attachments;
                    }
                }
                self.notice(Level::Error, err);
            }
            // Another client answered first; its answer stands.
            Reply::Approve => {}
            Reply::JobOutput { .. } => {}
            Reply::TabCreated { placeholder: true } => {
                for tab in &mut self.tabs {
                    tab.creating = false;
                }
                self.flash(err);
            }
            _ => self.flash(err),
        }
    }

    /// A tab this terminal asked for exists.
    fn created(&mut self, id: TabId, placeholder: bool) {
        if !placeholder {
            match self.tab_index(&id) {
                Some(index) => self.switch_tab(index),
                None => self.focus_tab = Some(id),
            }
            return;
        }
        let Some(stand_in) = self.tabs.iter().position(|t| t.id.is_none() && t.creating) else {
            return;
        };
        if self.tab_index(&id).is_some() {
            // The announcement came first: what waited moves to the real tab.
            let was_active = stand_in == self.active;
            let waiting = self.tabs.remove(stand_in);
            if self.active > stand_in {
                self.active -= 1;
            }
            if let Some(real) = self.tab_index(&id) {
                self.tabs[real].queued.extend(waiting.queued);
                if was_active {
                    self.active = real;
                }
            }
        } else {
            let tab = &mut self.tabs[stand_in];
            tab.id = Some(id);
            tab.creating = false;
        }
        self.send_queued();
    }

    fn opened_workspace(&mut self, workspace: &WorkspaceView, fresh: bool) {
        self.apply_workspace(workspace);
        // Questions and jobs come with a full snapshot of it.
        self.resync();
        if fresh && workspace.tabs.is_empty() {
            self.create_tab_for(self.active);
        }
        self.flash_ok(format!("workspace {}", workspace.name));
    }

    pub(super) fn on_rest(&mut self, rest: Rest, result: Result<Value, String>) {
        let summarize = |tools: &[Arc<dyn Tool>], name: &str, input: &Value| {
            tools
                .iter()
                .find(|t| t.name() == name)
                .map(|t| t.summary(input))
                .unwrap_or_else(|| malac_core::tools::summarize_arguments(input))
        };
        match (rest, result) {
            (Rest::History { agent }, result) => {
                // A new agent has no saved conversation yet.
                let saved = result.unwrap_or_default();
                let history: Vec<malac_core::Message> =
                    serde_json::from_value(saved["history"].clone()).unwrap_or_default();
                let usage = serde_json::from_value(saved["usage"].clone()).ok();
                let tools = self.tools.clone();
                self.agents.loaded(&agent, &history, usage, |name, input| {
                    summarize(&tools, name, input)
                });
            }
            (Rest::Sessions, Ok(value)) => {
                self.sessions =
                    serde_json::from_value(value["sessions"].clone()).unwrap_or_default();
                self.recent = recent_sessions(&self.tab_cwd(), &self.tabs, &self.sessions);
                let rows = self.session_rows();
                if let Some(Overlay::Sessions(picker)) = &mut self.overlay {
                    picker.set_rows(rows);
                }
            }
            // The lists stay as they were.
            (Rest::Sessions, Err(_)) => {}
            (Rest::Models, Ok(value)) => {
                self.discovered =
                    serde_json::from_value(value["providers"].clone()).unwrap_or_default();
                self.discovering = false;
                let rows = self.model_rows();
                if let Some(Overlay::Models(picker)) = &mut self.overlay {
                    picker.set_rows(rows);
                    picker.note.clear();
                }
            }
            (Rest::Tools, Ok(value)) => {
                let lines = mcp_lines(&value["servers"]);
                self.info(lines);
            }
            (Rest::Memory, Ok(value)) => {
                let lines = memory_lines(&value["entries"]);
                self.info(lines);
            }
            (Rest::UpdateChecked | Rest::UpdateInstalled, result) => {
                let result = result
                    .and_then(|v| serde_json::from_value::<Status>(v).map_err(|e| e.to_string()));
                self.update_result(result);
            }
            (Rest::DaemonStopped, Ok(_)) => {
                self.link.restarting = true;
                self.flash_ok("restarting the daemon");
            }
            (Rest::DaemonStopped, Err(err)) => {
                self.flash(format!("the daemon stays: {err}"));
            }
            (Rest::Models, Err(err)) => {
                self.discovering = false;
                if let Some(Overlay::Models(picker)) = &mut self.overlay {
                    picker.note = text::truncate(text::first_line(&err), 60).into_owned();
                }
            }
            (Rest::Tools | Rest::Memory, Err(err)) => self.notice(Level::Error, err),
            (Rest::Devices, result) => {
                let devices = result.and_then(|v| {
                    serde_json::from_value::<Vec<crate::protocol::DeviceView>>(v["devices"].clone())
                        .map_err(|e| e.to_string())
                });
                if let Some(Overlay::Remote(dialog)) = &mut self.overlay {
                    if let Ok(list) = &devices {
                        dialog.selected = dialog.selected.min(list.len().saturating_sub(1));
                    }
                    dialog.devices = Some(devices);
                }
            }
            (Rest::Pairing, result) => {
                let pairing =
                    result.and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()));
                if let Some(Overlay::Remote(dialog)) = &mut self.overlay {
                    dialog.pairing = Some(pairing);
                }
            }
            (Rest::Revoked, Ok(value)) => {
                let name = value["revoked"]["name"].as_str().unwrap_or("the device");
                self.flash_ok(format!("revoked {name}"));
            }
            (Rest::Revoked, Err(err)) => self.flash(format!("cannot revoke: {err}")),
        }
    }

    fn update_result(&mut self, result: Result<Status, String>) {
        let message = match &result {
            Ok(status) => {
                self.update = status.clone();
                Ok(
                    match (&status.installed, status.available, &status.latest) {
                        (Some(version), _, _) => {
                            format!("installed {version}; restart malac and the daemon to use it")
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

    /// The daemon went away: try to get it back.
    pub fn disconnected(&mut self) {
        self.link.connected = false;
        self.backend.disconnect();
        for tab in &mut self.tabs {
            tab.sending = false;
        }
        let start = std::mem::take(&mut self.link.restarting);
        self.reconnect(start);
    }

    pub fn reconnect(&mut self, start: bool) {
        if self.link.connected || self.link.reconnecting {
            return;
        }
        self.link.reconnecting = true;
        self.backend.reconnect(start);
    }

    /// Connected again: catch up on this workspace.
    pub fn reconnected(&mut self, client: Client, snapshot: Snapshot) {
        self.backend.connect(client);
        self.link = Link {
            connected: true,
            reconnecting: false,
            stopped: None,
            restarting: false,
        };
        self.daemon = snapshot.daemon;
        self.resync();
        self.flash_ok("connected to the daemon");
    }

    /// Asks for the log of the job an open jobs view shows.
    pub(super) fn refresh_job_log(&self) {
        let Some(Overlay::Jobs(view)) = &self.overlay else {
            return;
        };
        if let Some(job) = view.shown_job(&self.workspace_jobs()) {
            let tail = Some(view.rows.clamp(20, 500));
            self.backend.request(
                Command::JobOutput {
                    job: job.clone(),
                    tail,
                },
                Reply::JobOutput { job },
            );
        }
    }

    /// Jobs started by this workspace's agents, and those whose agent is unknown.
    pub fn workspace_jobs(&self) -> Vec<JobView> {
        self.jobs
            .iter()
            .filter(|j| j.tab.as_ref().is_none_or(|t| self.tab_index(t).is_some()))
            .cloned()
            .collect()
    }

    pub fn running_jobs(&self) -> usize {
        self.workspace_jobs()
            .iter()
            .filter(|j| j.job.is_running())
            .count()
    }

    /// Folders sessions were started in, newest first, for the folder browser.
    pub fn recent_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self.tabs.iter().map(|t| t.cwd.clone()).collect();
        for summary in &self.sessions {
            if dirs.len() >= 8 {
                break;
            }
            let dir = &summary.meta.cwd;
            if !dirs.contains(dir) {
                dirs.push(dir.clone());
            }
        }
        dirs.retain(|d| *d != self.tab_cwd());
        dirs
    }

    /// Indexes the tab's folder for the `@` picker in the background.
    pub(super) fn index_files(&mut self) {
        let root = self.tab_cwd();
        if self.files.root == root && (self.files.loading || self.files.list.is_some()) {
            return;
        }
        self.files = FileIndex {
            root: root.clone(),
            list: None,
            loading: true,
        };
        let updates = self.backend.updates();
        std::thread::spawn(move || {
            let list = files::index(&root);
            let _ = updates.send(Update::Files { root, list });
        });
    }

    /// Saves the clipboard's image off the interface thread and attaches it.
    pub(super) fn paste_image(&mut self) {
        let updates = self.backend.updates();
        let dir = std::env::temp_dir().join("malac");
        tokio::task::spawn_blocking(move || {
            let _ = updates.send(Update::Pasted(attach::clipboard_image(&dir)));
        });
    }

    /// Every top-level agent of every tab and their subagents, depth first,
    /// as the map shows them.
    pub fn agent_nodes(&self) -> Vec<Node> {
        let mut nodes = Vec::new();
        for (index, tab) in self.tabs.iter().enumerate() {
            for root in &tab.agents {
                for (_, view) in self.agents.tree(root) {
                    let snapshot = self.snapshots.get(&view.id);
                    let waiting = self.approvals.iter().any(|a| a.request.agent == view.id);
                    let state = match snapshot.map(|s| s.status) {
                        _ if waiting => State::Waiting,
                        Some(AgentStatus::Running) => State::Running,
                        Some(AgentStatus::Completed) => State::Done,
                        Some(AgentStatus::Failed) => State::Failed,
                        Some(AgentStatus::Cancelled) => State::Cancelled,
                        Some(AgentStatus::Idle) | None => State::Idle,
                    };
                    let activity = match (state, snapshot) {
                        (State::Running | State::Waiting, Some(s)) => s.activity.clone(),
                        (_, Some(s)) => s.report.clone().unwrap_or_default(),
                        (_, None) => String::new(),
                    };
                    let usage = snapshot.map_or(view.total, |s| s.usage);
                    nodes.push(Node {
                        id: view.id.clone(),
                        parent: view.parent.clone(),
                        label: view.label.clone(),
                        model: view.model.clone(),
                        state,
                        activity,
                        tokens: usage.input_tokens
                            + usage.cache_read_tokens
                            + usage.cache_write_tokens
                            + usage.output_tokens,
                        cost: usage.cost,
                        depth: snapshot.map_or(0, |s| s.info.depth),
                        tab: view.parent.is_none().then_some(index),
                    });
                }
            }
        }
        nodes
    }
}

fn mcp_lines(servers: &Value) -> Vec<Line<'static>> {
    let t = theme();
    let servers = servers.as_array().cloned().unwrap_or_default();
    if servers.is_empty() {
        return vec![Line::styled("  no MCP servers configured", t.dim())];
    }
    servers
        .iter()
        .map(|server| {
            let name = server["name"].as_str().unwrap_or_default().to_owned();
            match server["error"].as_str() {
                Some(err) => Line::from(vec![
                    Span::styled("  ✗ ", t.fg(t.error)),
                    Span::raw(name),
                    Span::styled(format!("  {}", text::first_line(err)), t.dim()),
                ]),
                None => Line::from(vec![
                    Span::styled("  ● ", t.fg(t.accent)),
                    Span::raw(name),
                    Span::styled(format!("  {} tools", server["tools"]), t.dim()),
                ]),
            }
        })
        .collect()
}

fn memory_lines(entries: &Value) -> Vec<Line<'static>> {
    let t = theme();
    let entries = entries.as_array().cloned().unwrap_or_default();
    if entries.is_empty() {
        return vec![Line::styled("  no memories saved", t.dim())];
    }
    entries
        .iter()
        .map(|entry| {
            let text = |key: &str| entry[key].as_str().unwrap_or_default().to_owned();
            Line::from(vec![
                Span::raw(format!("  {}", text("name"))),
                Span::styled(
                    format!("  {} · {}", text("scope"), text("description")),
                    t.dim(),
                ),
            ])
        })
        .collect()
}
