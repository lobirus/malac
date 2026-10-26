//! The daemon's wire protocol, shared by the server (`web`) and the typed
//! client (`client`). Every message is a JSON object whose `type` names it;
//! `docs/protocol.md` describes them with examples.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use malac_core::attach::Attachment;
use malac_core::config::{KeepAwake, Tunnel};
use malac_core::event::{AgentId, Level};
use malac_core::jobs::{JobInfo, JobOutput};
use malac_core::permission::{Decision, Mode, PermissionRequest};
use malac_core::{AgentSnapshot, Envelope, update};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type WorkspaceId = String;
pub type TabId = String;

/// A client message: a command plus an optional reference that the reply echoes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    #[serde(flatten)]
    pub command: Command,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Sends a prompt to `agent`, or starts a new top-level agent in `tab`
    /// (default: the active tab of the connection's workspace) and sends it there.
    Prompt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<TabId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<AgentId>,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImageInput>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<AttachmentInput>,
        /// Model for a new agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Starts another top-level agent in a tab without prompting it.
    AgentStart {
        tab: TabId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Makes a saved or live session an agent of a tab (by default the one
    /// that has it, else a tab for the session's directory) and loads it.
    AgentOpen {
        agent: AgentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<TabId>,
    },
    /// The web UI's original name for `agent_open`.
    Resume {
        session: AgentId,
    },
    AgentActivate {
        tab: TabId,
        agent: AgentId,
    },
    /// Removes an idle agent from its tab. Its session stays on disk.
    AgentClose {
        tab: TabId,
        agent: AgentId,
    },
    /// Cancels the agent's turn and those of its subagents.
    Cancel {
        agent: AgentId,
    },
    Approve {
        request_id: String,
        decision: Decision,
    },
    /// Changes a tab's permission mode, or every tab's without `tab`.
    Mode {
        mode: Mode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<TabId>,
    },
    Model {
        agent: AgentId,
        model: String,
    },
    Compact {
        agent: AgentId,
    },

    /// Picks a workspace and tab the way the `malac` command line does.
    Launch(Launch),
    /// Sends the snapshot again, optionally for another workspace.
    Snapshot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<String>,
    },
    WorkspaceList,
    /// Creates an empty workspace and opens it.
    WorkspaceCreate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Opens a workspace by id, unique id prefix or name.
    WorkspaceOpen {
        workspace: String,
    },
    WorkspaceRename {
        workspace: String,
        name: String,
    },
    /// Deletes a workspace. Refused while its agents run, unless `force`.
    WorkspaceDelete {
        workspace: String,
        #[serde(default)]
        force: bool,
    },
    TabCreate(TabSpec),
    /// Closes a tab. Refused while its agents run, unless `force`.
    TabClose {
        tab: TabId,
        #[serde(default)]
        force: bool,
    },
    TabRename {
        tab: TabId,
        name: String,
    },
    /// Moves a tab to `index` among its workspace's tabs.
    TabMove {
        tab: TabId,
        index: usize,
    },
    TabActivate {
        tab: TabId,
    },

    JobList,
    JobOutput {
        job: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tail: Option<usize>,
    },
    JobKill {
        job: String,
    },

    /// Reports the control service's permissions; with `request`, asks
    /// macOS for missing ones.
    ControlPermissions {
        #[serde(default)]
        request: bool,
    },
    /// Changes when the daemon keeps the machine awake, until it restarts.
    KeepAwake {
        mode: KeepAwake,
    },
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageInput {
    pub media_type: String,
    /// Base64.
    pub data: String,
    /// Shows the image as an attachment of this name, e.g. a pasted
    /// screenshot; the image itself travels here, not as a path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInput {
    /// Absolute, `~/`, or relative to the tab's directory.
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TabSpec {
    /// The workspace to add the tab to (default: the connection's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, flatten)]
    pub options: TabOptions,
    /// Return an existing tab with the same directory instead of adding one.
    #[serde(default)]
    pub reuse: bool,
}

/// Per-tab settings applied on top of the tab directory's configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabOptions {
    /// Model for new agents, as provider/model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_control: bool,
}

/// What `malac [options] [prompt]` asks for.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Launch {
    #[serde(default)]
    pub workspace: WorkspaceChoice,
    /// The directory a new tab opens in.
    pub cwd: PathBuf,
    /// Names the new tab, or a new workspace when `workspace` is `new`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Attach this session in a tab (`-r`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<AgentId>,
    /// Attach the latest session started in `cwd` (`-c`).
    #[serde(default)]
    pub continue_last: bool,
    /// Run this in a new tab for `cwd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentInput>,
    #[serde(default, flatten)]
    pub options: TabOptions,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceChoice {
    /// The last used workspace, created as "default" the first time.
    #[default]
    Last,
    /// A fresh workspace (`-n`).
    New,
    /// A workspace by id, id prefix or name (`-w`).
    Open(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// The first message on every connection, and the reply to `snapshot`.
    Snapshot(Box<Snapshot>),
    /// An agent event. `tab` is null for events that belong to no tab, such
    /// as update notices.
    Event {
        #[serde(default)]
        tab: Option<TabId>,
        #[serde(default)]
        workspace: Option<WorkspaceId>,
        envelope: Box<Envelope>,
    },
    Ack {
        #[serde(default)]
        client_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<AgentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<TabId>,
        /// The workspace the tab belongs to, for commands that create or
        /// pick one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<WorkspaceId>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Attachment>,
    },
    Error {
        #[serde(default)]
        client_ref: Option<String>,
        message: String,
    },
    Pong {
        #[serde(default)]
        client_ref: Option<String>,
    },
    Launched {
        #[serde(default)]
        client_ref: Option<String>,
        workspace: Box<WorkspaceView>,
        tab: Option<TabId>,
        agent: Option<AgentId>,
    },
    Workspaces {
        #[serde(default)]
        client_ref: Option<String>,
        workspaces: Vec<WorkspaceSummary>,
    },
    WorkspaceOpened {
        #[serde(default)]
        client_ref: Option<String>,
        workspace: Box<WorkspaceView>,
    },
    WorkspaceCreated {
        workspace: WorkspaceSummary,
    },
    /// Renamed, or its active tab changed.
    WorkspaceUpdated {
        workspace: WorkspaceSummary,
    },
    WorkspaceDeleted {
        workspace: WorkspaceId,
    },
    TabCreated {
        workspace: WorkspaceId,
        index: usize,
        tab: Box<TabView>,
    },
    /// Renamed, its agents or settings changed, or its runtime was loaded.
    TabUpdated {
        workspace: WorkspaceId,
        tab: Box<TabView>,
    },
    TabClosed {
        workspace: WorkspaceId,
        tab: TabId,
    },
    TabsReordered {
        workspace: WorkspaceId,
        tabs: Vec<TabId>,
    },
    JobStarted {
        job: JobView,
    },
    JobFinished {
        job: JobView,
    },
    Jobs {
        #[serde(default)]
        client_ref: Option<String>,
        jobs: Vec<JobView>,
    },
    JobOutput {
        #[serde(default)]
        client_ref: Option<String>,
        output: Box<JobOutput>,
    },
    Control {
        #[serde(default)]
        client_ref: Option<String>,
        control: ControlStatus,
    },
    /// Something to show the user, such as a configuration warning.
    Notice {
        level: Level,
        message: String,
        #[serde(default)]
        tab: Option<TabId>,
    },
    /// A permission mode changed: one tab's, or every tab of `workspace`
    /// when `tab` is null.
    Mode {
        mode: Mode,
        #[serde(default)]
        tab: Option<TabId>,
        #[serde(default)]
        workspace: Option<WorkspaceId>,
    },
    /// The number of connected clients changed.
    Clients {
        clients: usize,
    },
    /// Remote access changed: a tunnel started or stopped, a device was
    /// paired or revoked.
    Remote {
        remote: RemoteStatus,
    },
    /// The daemon started or stopped keeping the machine awake.
    Awake {
        awake: AwakeStatus,
    },
    /// Settings were applied; refetch cached resources.
    Reloaded,
    /// The connection fell behind and missed `skipped` messages; resync.
    Lagged {
        skipped: u64,
    },
    /// The daemon is stopping.
    Shutdown {
        reason: String,
    },
}

impl ServerMessage {
    /// The reference of the request this message answers, if any.
    pub fn client_ref(&self) -> Option<&str> {
        match self {
            ServerMessage::Snapshot(snapshot) => snapshot.client_ref.as_deref(),
            ServerMessage::Ack { client_ref, .. }
            | ServerMessage::Error { client_ref, .. }
            | ServerMessage::Pong { client_ref }
            | ServerMessage::Launched { client_ref, .. }
            | ServerMessage::Workspaces { client_ref, .. }
            | ServerMessage::WorkspaceOpened { client_ref, .. }
            | ServerMessage::Jobs { client_ref, .. }
            | ServerMessage::JobOutput { client_ref, .. }
            | ServerMessage::Control { client_ref, .. } => client_ref.as_deref(),
            _ => None,
        }
    }

    /// Sets the reference on a reply.
    pub fn answering(mut self, reference: Option<String>) -> Self {
        match &mut self {
            ServerMessage::Snapshot(snapshot) => snapshot.client_ref = reference,
            ServerMessage::Ack { client_ref, .. }
            | ServerMessage::Error { client_ref, .. }
            | ServerMessage::Pong { client_ref }
            | ServerMessage::Launched { client_ref, .. }
            | ServerMessage::Workspaces { client_ref, .. }
            | ServerMessage::WorkspaceOpened { client_ref, .. }
            | ServerMessage::Jobs { client_ref, .. }
            | ServerMessage::JobOutput { client_ref, .. }
            | ServerMessage::Control { client_ref, .. } => *client_ref = reference,
            _ => {}
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Set when this answers a `snapshot` request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_ref: Option<String>,
    pub daemon: DaemonStatus,
    pub workspaces: Vec<WorkspaceSummary>,
    /// The connection's workspace.
    pub workspace: Option<WorkspaceView>,
    /// Permission requests waiting for an answer, from every tab.
    pub pending: Vec<PendingApproval>,
    pub jobs: Vec<JobView>,
    pub update: update::Status,
    pub control: ControlStatus,
    /// Every configured `provider/model`.
    pub models: Vec<String>,
    pub default_model: Option<String>,
    #[serde(default)]
    pub remote: RemoteStatus,
    #[serde(default)]
    pub awake: AwakeStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub pid: u32,
    pub started: DateTime<Utc>,
    /// The client said it runs a different malac version; show a notice
    /// suggesting `malac daemon restart`.
    pub version_mismatch: bool,
    /// Connected WebSocket clients, this one included.
    pub clients: usize,
    /// Serving a terminal session's runtime instead of a daemon: workspace
    /// and tab commands are refused.
    pub attached: bool,
    /// Whether the server also listens on other interfaces.
    pub remote: bool,
    /// The user's home directory, so clients can show paths as `~/…`.
    #[serde(default)]
    pub user_home: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: WorkspaceId,
    pub name: String,
    pub tabs: usize,
    pub active_tab: Option<TabId>,
    /// Agents of this workspace that are working right now.
    pub running: usize,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceView {
    pub id: WorkspaceId,
    pub name: String,
    pub active_tab: Option<TabId>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub tabs: Vec<TabView>,
}

impl WorkspaceView {
    pub fn tab(&self, id: &str) -> Option<&TabView> {
        self.tabs.iter().find(|t| t.id == id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabView {
    pub id: TabId,
    pub name: String,
    pub cwd: PathBuf,
    /// Top-level agents, in order.
    pub agents: Vec<AgentId>,
    pub active_agent: Option<AgentId>,
    /// The permission mode in effect.
    pub mode: Mode,
    /// What the tab was opened with.
    pub options: TabOptions,
    /// Whether the tab's runtime is loaded. Tabs load on first use.
    pub loaded: bool,
    /// Every known agent of the tab: the top-level ones, live or saved, and
    /// the live subagents (which have a `parent`).
    pub snapshots: Vec<AgentView>,
    /// Once loaded: where the tab sits in its repository (with a
    /// `suggest_root` when it is one part of a larger one) and its custom
    /// slash commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<malac_core::context::Summary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentView {
    #[serde(flatten)]
    pub snapshot: AgentSnapshot,
    /// Loaded in the daemon; a saved session is loaded when it is prompted.
    pub live: bool,
    /// The first prompt, for saved sessions.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    #[serde(flatten)]
    pub request: PermissionRequest,
    pub tab: Option<TabId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobView {
    #[serde(flatten)]
    pub job: JobInfo,
    /// The tab whose agent started it, when known.
    pub tab: Option<TabId>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ControlStatus {
    pub enabled: bool,
    pub connected: bool,
    pub tools: usize,
    pub error: Option<String>,
    /// The `computer_permissions` report of the connected service: Screen
    /// Recording and Accessibility state and the app holding them.
    pub permissions: Option<Value>,
}

/// How other machines reach the daemon.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RemoteStatus {
    /// Paired devices on other machines may connect (`web.remote`).
    pub enabled: bool,
    /// The address and port the daemon listens on, e.g. `0.0.0.0:7700`.
    pub listen: String,
    /// Addresses other devices can use, best first.
    pub urls: Vec<RemoteUrl>,
    pub tunnel: Option<TunnelStatus>,
    /// Paired devices.
    pub devices: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteUrl {
    pub kind: UrlKind,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UrlKind {
    /// The public address of a running tunnel.
    Tunnel,
    /// `remote.public_url`, e.g. a reverse proxy.
    Public,
    /// This machine's address on its network (on a server, possibly public).
    Lan,
    /// The loopback address, for this machine and SSH tunnels.
    Local,
}

impl UrlKind {
    pub fn label(self) -> &'static str {
        match self {
            UrlKind::Tunnel => "tunnel",
            UrlKind::Public => "public",
            UrlKind::Lan => "network address",
            UrlKind::Local => "this machine or an SSH tunnel",
        }
    }

    /// A label for a column of addresses.
    pub fn short(self) -> &'static str {
        match self {
            UrlKind::Tunnel => "tunnel",
            UrlKind::Public => "public",
            UrlKind::Lan => "network",
            UrlKind::Local => "SSH tunnel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TunnelStatus {
    pub provider: Tunnel,
    pub state: TunnelState,
    /// The public address, once the tunnel reports it.
    pub url: Option<String>,
    pub error: Option<String>,
    /// What to know about this kind of tunnel.
    #[serde(default)]
    pub notes: Vec<String>,
    pub started: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    Starting,
    Running,
    Failed,
    Stopped,
}

/// Whether the daemon keeps the machine from sleeping, and why.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AwakeStatus {
    pub mode: KeepAwake,
    /// A power assertion is held right now.
    pub awake: bool,
    /// Why it is held or not, e.g. "2 agents running".
    pub reason: String,
    /// `caffeinate` or `systemd-inhibit`.
    pub method: Option<String>,
    pub error: Option<String>,
    /// When the current state began.
    pub since: Option<DateTime<Utc>>,
}

/// A paired device, as `GET /api/devices` lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    /// `cli`, `browser`, `app` or what the device said it is.
    pub kind: String,
    pub created: DateTime<Utc>,
    pub last_seen: Option<DateTime<Utc>>,
    /// The address it last connected from, as the daemon saw it.
    pub address: Option<String>,
    /// The device making this request.
    #[serde(default)]
    pub current: bool,
}

/// A fresh pairing code and the links that carry it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pairing {
    /// `XXXXX-XXXXX`, valid once.
    pub code: String,
    pub expires: DateTime<Utc>,
    /// `<url>/#pair=<code>` for each address in `RemoteStatus::urls`, and
    /// the loopback one.
    pub links: Vec<PairLink>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairLink {
    pub kind: UrlKind,
    /// The daemon's address.
    pub base: String,
    /// The address with the code, for a browser or a QR code.
    pub url: String,
    /// The link as a QR code.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub svg: String,
}

/// `POST /api/pair`: a device trades a pairing code for its token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paired {
    /// The device token; send it like the master token.
    pub token: String,
    pub device: DeviceView,
    /// The daemon's version.
    pub version: String,
}

/// `GET /api/daemon`: what `malac daemon status` shows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonInfo {
    #[serde(flatten)]
    pub status: DaemonStatus,
    pub url: String,
    pub workspaces: usize,
    pub tabs_loaded: usize,
    pub agents_running: usize,
    pub jobs_running: usize,
    pub pending_approvals: usize,
    pub idle_minutes: u64,
    /// Named apart from `remote`, the flag in the status above.
    #[serde(default)]
    pub remote_access: RemoteStatus,
    #[serde(default)]
    pub awake: AwakeStatus,
}

impl DaemonInfo {
    pub fn busy(&self) -> bool {
        self.agents_running > 0 || self.jobs_running > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn web_ui_messages_still_parse() {
        let request: Request = serde_json::from_value(json!({
            "type": "prompt", "agent": null, "text": "hi", "images": [], "client_ref": "r1"
        }))
        .unwrap();
        assert_eq!(request.client_ref.as_deref(), Some("r1"));
        assert!(matches!(
            request.command,
            Command::Prompt { agent: None, ref text, .. } if text == "hi"
        ));
        let request: Request =
            serde_json::from_value(json!({ "type": "resume", "session": "abc" })).unwrap();
        assert!(matches!(request.command, Command::Resume { .. }));
        let request: Request =
            serde_json::from_value(json!({ "type": "mode", "mode": "autopilot" })).unwrap();
        assert!(matches!(
            request.command,
            Command::Mode {
                mode: Mode::Autopilot,
                tab: None
            }
        ));
    }

    #[test]
    fn launch_round_trips() {
        let launch = Launch {
            workspace: WorkspaceChoice::Open("work".into()),
            cwd: "/src/app".into(),
            prompt: Some("fix it".into()),
            options: TabOptions {
                mode: Some(Mode::Autopilot),
                ..Default::default()
            },
            ..Default::default()
        };
        let text = serde_json::to_string(&Request {
            command: Command::Launch(launch),
            client_ref: Some("r2".into()),
        })
        .unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["type"], "launch");
        assert_eq!(value["workspace"]["open"], "work");
        assert_eq!(value["mode"], "autopilot");
        let back: Request = serde_json::from_str(&text).unwrap();
        let Command::Launch(launch) = back.command else {
            panic!("launch expected");
        };
        assert_eq!(launch.prompt.as_deref(), Some("fix it"));
        assert_eq!(launch.options.mode, Some(Mode::Autopilot));
    }

    #[test]
    fn tab_views_round_trip() {
        let view = TabView {
            id: "t".into(),
            name: "app".into(),
            cwd: "/src/app".into(),
            agents: vec!["a".into()],
            active_agent: Some("a".into()),
            mode: Mode::Ask,
            options: TabOptions {
                mode: Some(Mode::Autopilot),
                ..Default::default()
            },
            loaded: false,
            snapshots: Vec::new(),
            project: None,
        };
        let text = serde_json::to_string(&view).unwrap();
        let back: TabView = serde_json::from_str(&text).unwrap();
        assert_eq!(back.mode, Mode::Ask);
        assert_eq!(back.options.mode, Some(Mode::Autopilot));
    }

    #[test]
    fn replies_carry_the_reference() {
        let reply = ServerMessage::Pong { client_ref: None }.answering(Some("r3".into()));
        assert_eq!(reply.client_ref(), Some("r3"));
        let text = serde_json::to_string(&reply).unwrap();
        assert_eq!(text, r#"{"type":"pong","client_ref":"r3"}"#);
    }
}
