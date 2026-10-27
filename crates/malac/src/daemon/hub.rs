//! The daemon's state: workspaces and their tabs, one runtime per loaded tab,
//! pending approvals, jobs and the stream of messages every client receives.
//! The web server hands it each command; it publishes each change.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use malac_core::agent::AgentInfo;
use malac_core::attach;
use malac_core::config::{self, Config, KeepAwake, Tunnel};
use malac_core::event::{AgentId, Event, Level};
use malac_core::jobs::{DEFAULT_TAIL_LINES, JobEvent, JobInfo};
use malac_core::mcp::CONTROL_SERVER;
use malac_core::message::Usage;
use malac_core::permission::{Approver, Decision, Mode, PermissionRequest};
use malac_core::provider::Provider;
use malac_core::tools::truncate_chars;
use malac_core::{
    AgentHandle, AgentSnapshot, AgentSpec, AgentStatus, Block, Envelope, Host, Runtime, session,
    update,
};
use tokio::sync::{Notify, OnceCell, broadcast, oneshot, watch};
use tokio::task::JoinHandle;

use super::access::Access;
use super::awake::{self, Awake, Runner, SystemRunner, Work};
use super::store::{Store, Tab, Workspace};
use super::tunnel;
use super::uploads;
use super::voice::Voice;
use crate::protocol::{
    AgentView, AttachmentInput, AwakeStatus, Command, ControlStatus, DaemonInfo, DaemonStatus,
    ImageInput, JobView, Launch, PairLink, Pairing, PendingApproval, RemoteStatus, RemoteUrl,
    ServerMessage, Snapshot, TabId, TabOptions, TabSpec, TabView, TunnelState, TunnelStatus,
    UrlKind, WorkspaceChoice, WorkspaceId, WorkspaceSummary, WorkspaceView,
};

const DEFAULT_WORKSPACE: &str = "default";
const ATTACHED_ID: &str = "terminal";
/// Messages a slow client may fall behind before it is told to resync.
const OUTBOX: usize = 8192;

/// Settings fixed when the daemon starts.
#[derive(Debug, Clone)]
pub struct HubOptions {
    /// Whether paired devices on other machines may connect.
    pub remote: bool,
    pub idle_minutes: u64,
    /// Root of the directory picker.
    pub browse_root: PathBuf,
    /// When to keep the machine awake, until a client changes it.
    pub keep_awake: KeepAwake,
    /// `remote.public_url`, offered in pairing links.
    pub public_url: Option<String>,
}

/// Where the server listens, once it is bound.
#[derive(Debug, Clone, Default)]
struct Listen {
    addr: Option<SocketAddr>,
    /// This machine's address on the local network, when listening on all interfaces.
    lan: Option<String>,
}

/// One client connection's view: the workspace it has open and the malac
/// version it reported.
#[derive(Debug, Default, Clone)]
pub struct Conn {
    pub workspace: Option<WorkspaceId>,
    pub version: Option<String>,
    /// Its listener id while it plays speech (`voice_listen`).
    pub listener: Option<u64>,
}

pub struct Hub {
    host: Host,
    /// `None` when serving a terminal session's runtime.
    store: Option<Store>,
    options: HubOptions,
    base: RwLock<Config>,
    approvals: Arc<Approvals>,
    started: DateTime<Utc>,
    state: Mutex<State>,
    runtimes: Mutex<HashMap<TabId, Arc<OnceCell<Arc<Runtime>>>>>,
    forwarders: Mutex<HashMap<TabId, JoinHandle<()>>>,
    providers: Mutex<Vec<(String, Arc<dyn Provider>)>>,
    out: broadcast::Sender<Arc<str>>,
    clients: AtomicUsize,
    /// The running count last announced per workspace.
    announced: Mutex<HashMap<WorkspaceId, usize>>,
    control: Mutex<ControlStatus>,
    stop: watch::Sender<Option<String>>,
    reload_lock: tokio::sync::Mutex<()>,
    access: Access,
    awake: Mutex<Awake>,
    /// Woken when agents or jobs start or finish, to update `awake`.
    activity: Notify,
    tunnel: tokio::sync::Mutex<Option<tunnel::Running>>,
    /// The tunnel's last status, readable without waiting.
    tunnel_status: Mutex<Option<TunnelStatus>>,
    listen: Mutex<Listen>,
    voice: Arc<Voice>,
}

struct State {
    workspaces: Vec<Workspace>,
    last: Option<WorkspaceId>,
}

impl Hub {
    /// A daemon hub over the workspaces saved in `~/.malac`. Jobs left by an
    /// earlier daemon are restored; tabs load their runtime on first use.
    pub fn daemon(options: HubOptions) -> Result<Arc<Self>> {
        let store = Store::new(config::home_dir());
        let workspaces = store.load_all();
        let last = store
            .last()
            .filter(|id| workspaces.iter().any(|w| &w.id == id));
        let base = base_config()?;
        let access = Access::load(config::home_dir().join("devices.json"));
        let runner: Arc<dyn Runner> = Arc::new(SystemRunner::detect());
        let hub = Self::build(
            Host::new(),
            Some(store),
            options,
            base,
            (workspaces, last),
            (access, runner),
        );
        if let Err(err) = hub.host.jobs.restore() {
            tracing::warn!("cannot restore jobs: {err:#}");
        }
        hub.start_tasks();
        Ok(hub)
    }

    /// A hub for one terminal session: a single tab holding `runtime`.
    /// Workspace and tab commands are refused.
    pub fn attached(runtime: Arc<Runtime>, options: HubOptions) -> Arc<Self> {
        let mut workspace = Workspace::new(ATTACHED_ID);
        workspace.id = ATTACHED_ID.into();
        let mut tab = Tab::new(
            &tab_name(&runtime.cwd, None),
            &runtime.cwd,
            TabOptions::default(),
        );
        tab.id = ATTACHED_ID.into();
        workspace.active_tab = Some(tab.id.clone());
        workspace.tabs.push(tab);
        let host = Host {
            bus: runtime.bus.clone(),
            jobs: runtime.jobs.clone(),
            mcp: Arc::default(),
        };
        let options = HubOptions {
            keep_awake: KeepAwake::Off,
            ..options
        };
        let runner: Arc<dyn Runner> = Arc::new(SystemRunner::detect());
        let hub = Self::build(
            host,
            None,
            options,
            runtime.config.clone(),
            (vec![workspace], Some(ATTACHED_ID.into())),
            (Access::in_memory(), runner),
        );
        let cell = Arc::new(OnceCell::new_with(Some(runtime.clone())));
        hub.runtimes
            .lock()
            .unwrap()
            .insert(ATTACHED_ID.into(), cell);
        hub.forward(ATTACHED_ID, ATTACHED_ID, &runtime);
        hub.forward_jobs();
        hub
    }

    fn build(
        host: Host,
        store: Option<Store>,
        options: HubOptions,
        base: Config,
        (workspaces, last): (Vec<Workspace>, Option<WorkspaceId>),
        (access, runner): (Access, Arc<dyn Runner>),
    ) -> Arc<Self> {
        let (out, _) = broadcast::channel(OUTBOX);
        let (stop, _) = watch::channel(None);
        let control = ControlStatus {
            enabled: base.control.enabled,
            ..Default::default()
        };
        let awake = Awake::new(options.keep_awake, runner, awake::GRACE);
        Arc::new(Self {
            host,
            store,
            options,
            base: RwLock::new(base),
            approvals: Arc::default(),
            started: Utc::now(),
            state: Mutex::new(State { workspaces, last }),
            runtimes: Mutex::default(),
            forwarders: Mutex::default(),
            providers: Mutex::default(),
            out,
            clients: AtomicUsize::new(0),
            announced: Mutex::default(),
            control: Mutex::new(control),
            stop,
            reload_lock: tokio::sync::Mutex::new(()),
            access,
            awake: Mutex::new(awake),
            activity: Notify::new(),
            tunnel: tokio::sync::Mutex::new(None),
            tunnel_status: Mutex::new(None),
            listen: Mutex::default(),
            voice: Arc::default(),
        })
    }

    fn start_tasks(self: &Arc<Self>) {
        let base = self.base_config();
        self.voice_changed(&base, false);
        let hub = self.clone();
        tokio::spawn(async move { hub.refresh_voice().await });
        self.forward_jobs();
        self.forward_host_events();
        self.start_update_checks();
        self.start_control();
        self.watch_awake();
    }

    pub fn voice(&self) -> &Arc<Voice> {
        &self.voice
    }

    /// The daemon's configuration: the user's, without a project's.
    pub fn base_config(&self) -> Config {
        self.base.read().unwrap().clone()
    }

    /// Rereads the user's configuration without reloading tabs.
    pub fn refresh_base(&self) -> Result<()> {
        *self.base.write().unwrap() = base_config()?;
        Ok(())
    }

    pub fn is_attached(&self) -> bool {
        self.store.is_none()
    }

    pub fn options(&self) -> &HubOptions {
        &self.options
    }

    /// The approver every tab runtime uses; also raced against a terminal's.
    pub fn approver(&self) -> Arc<dyn Approver> {
        self.approvals.clone()
    }

    /// Registers a provider in every runtime this hub builds, replacing the
    /// configured one. For embedding malac with a custom backend, and tests.
    pub fn insert_provider(&self, name: &str, provider: Arc<dyn Provider>) {
        for runtime in self.loaded() {
            runtime.insert_provider(name, provider.clone());
        }
        self.providers
            .lock()
            .unwrap()
            .push((name.to_owned(), provider));
    }

    /// Drops cached clients for `name` in every tab, e.g. after its key changed.
    pub fn forget_provider(&self, name: &str) {
        for runtime in self.loaded() {
            runtime.forget_provider(name);
        }
    }

    /// Every message for clients, serialized once.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<str>> {
        self.out.subscribe()
    }

    /// Resolves with the reason once the hub is asked to stop.
    pub async fn stopped(&self) -> String {
        let mut stop = self.stop.subscribe();
        loop {
            if let Some(reason) = stop.borrow_and_update().clone() {
                return reason;
            }
            if stop.changed().await.is_err() {
                return "stopped".into();
            }
        }
    }

    pub fn request_stop(&self, reason: &str) {
        self.publish(&ServerMessage::Shutdown {
            reason: reason.to_owned(),
        });
        self.stop.send_replace(Some(reason.to_owned()));
    }

    /// Cancels every agent, closes MCP connections, stops the tunnel and
    /// lets the machine sleep again.
    pub async fn close(&self) {
        if let Some(running) = self.tunnel.lock().await.take() {
            running.stop().await;
        }
        for runtime in self.loaded() {
            runtime.shutdown().await;
        }
        if !self.is_attached() {
            self.host.mcp.close().await;
        }
        let mut awake = self.awake.lock().unwrap();
        awake.set_mode(KeepAwake::Off, Work::default(), Instant::now());
    }

    /// Counts a client while the guard lives, announcing every change.
    pub fn connected(&self) -> ClientGuard<'_> {
        let clients = self.clients.fetch_add(1, Ordering::SeqCst) + 1;
        self.publish(&ServerMessage::Clients { clients });
        ClientGuard(self)
    }

    /// No clients, running agents, jobs or questions.
    pub fn is_idle(&self) -> bool {
        self.clients.load(Ordering::SeqCst) == 0
            && !self.loaded().iter().any(|r| r.is_busy())
            && self.host.jobs.running() == 0
            && self.approvals.pending().is_empty()
    }

    pub fn info(&self, url: &str) -> DaemonInfo {
        let workspaces = self.state.lock().unwrap().workspaces.len();
        let runtimes = self.loaded();
        DaemonInfo {
            status: self.status(None),
            url: url.to_owned(),
            workspaces,
            tabs_loaded: runtimes.len(),
            agents_running: runtimes
                .iter()
                .flat_map(|r| r.agents())
                .filter(|a| a.status == AgentStatus::Running)
                .count(),
            jobs_running: self.host.jobs.running(),
            pending_approvals: self.approvals.pending().len(),
            idle_minutes: self.options.idle_minutes,
            remote_access: self.remote_status(),
            awake: self.awake_status(),
        }
    }

    fn status(&self, client_version: Option<&str>) -> DaemonStatus {
        DaemonStatus {
            version: update::CURRENT.into(),
            pid: std::process::id(),
            started: self.started,
            version_mismatch: client_version.is_some_and(|v| v != update::CURRENT),
            clients: self.clients.load(Ordering::SeqCst),
            attached: self.is_attached(),
            remote: self.options.remote,
            user_home: dirs::home_dir().map(|h| h.display().to_string()),
        }
    }

    /// A reply naming the agent and tab a command acted on, and the tab's workspace.
    fn ack(&self, agent: Option<AgentId>, tab: Option<TabId>) -> ServerMessage {
        ServerMessage::Ack {
            client_ref: None,
            agent,
            workspace: tab
                .as_deref()
                .and_then(|t| self.find_tab(t))
                .map(|(w, _)| w),
            tab,
            attachments: Vec::new(),
        }
    }

    pub(crate) fn publish(&self, message: &ServerMessage) {
        match serde_json::to_string(message) {
            Ok(text) => {
                let _ = self.out.send(Arc::from(text));
            }
            Err(err) => tracing::warn!("cannot encode a message: {err}"),
        }
    }

    fn notice(&self, level: Level, message: String, tab: Option<&str>) {
        tracing::warn!("{message}");
        self.publish(&ServerMessage::Notice {
            level,
            message,
            tab: tab.map(str::to_owned),
        });
    }

    // Remote access, tunnels and keep-awake

    /// Paired devices and pairing codes.
    pub fn access(&self) -> &Access {
        &self.access
    }

    /// Records where the server listens, for status and pairing links.
    /// `lan` is this machine's address on the network, when listening on
    /// every interface.
    pub fn set_listen(&self, addr: SocketAddr, lan: Option<String>) {
        *self.listen.lock().unwrap() = Listen {
            addr: Some(addr),
            lan,
        };
    }

    pub fn remote_status(&self) -> RemoteStatus {
        let listen = self.listen.lock().unwrap().clone();
        let tunnel = self.tunnel_status.lock().unwrap().clone();
        let mut urls = Vec::new();
        if self.options.remote {
            let running = tunnel
                .as_ref()
                .filter(|t| t.state == TunnelState::Running)
                .and_then(|t| t.url.clone());
            if let Some(url) = running {
                urls.push(RemoteUrl {
                    kind: UrlKind::Tunnel,
                    url,
                });
            }
            if let Some(url) = &self.options.public_url {
                urls.push(RemoteUrl {
                    kind: UrlKind::Public,
                    url: url.trim_end_matches('/').to_owned(),
                });
            }
            if let Some(url) = listen.lan.clone() {
                urls.push(RemoteUrl {
                    kind: UrlKind::Lan,
                    url,
                });
            }
        }
        if let Some(addr) = listen.addr {
            let ip = match addr.ip() {
                ip if ip.is_unspecified() => "127.0.0.1".to_owned(),
                ip => ip.to_string(),
            };
            urls.push(RemoteUrl {
                kind: UrlKind::Local,
                url: format!("http://{ip}:{}", addr.port()),
            });
        }
        RemoteStatus {
            enabled: self.options.remote,
            listen: listen.addr.map(|a| a.to_string()).unwrap_or_default(),
            urls,
            tunnel,
            devices: self.access.device_count(),
        }
    }

    /// Tells clients that remote access changed.
    pub fn remote_changed(&self) {
        self.publish(&ServerMessage::Remote {
            remote: self.remote_status(),
        });
    }

    /// A one-time pairing code and a link with it for every address.
    pub fn pairing(&self) -> Pairing {
        let (code, expires) = self.access.create_code();
        let links = self
            .remote_status()
            .urls
            .into_iter()
            .map(|address| {
                let url = format!("{}/#pair={code}", address.url);
                let svg = crate::qr::QrCode::encode(&url)
                    .map(|qr| qr.svg())
                    .unwrap_or_default();
                PairLink {
                    kind: address.kind,
                    base: address.url,
                    url,
                    svg,
                }
            })
            .collect();
        Pairing {
            code,
            expires,
            links,
        }
    }

    /// Starts a tunnel to this daemon, replacing one that runs.
    pub async fn start_tunnel(self: &Arc<Self>, provider: Tunnel) -> Result<TunnelStatus> {
        let port = self
            .listen
            .lock()
            .unwrap()
            .addr
            .map(|a| a.port())
            .context("the daemon is not listening yet")?;
        let spec = tunnel::Spec::new(provider, port).map_err(anyhow::Error::msg)?;
        self.start_tunnel_with(spec).await
    }

    /// Starts a tunnel from a given command, e.g. a stand-in in tests.
    pub async fn start_tunnel_with(self: &Arc<Self>, spec: tunnel::Spec) -> Result<TunnelStatus> {
        if self.is_attached() {
            bail!("tunnels belong to the daemon; this server is a terminal session's");
        }
        if !self.options.remote {
            bail!(
                "remote access is off, so the daemon would refuse everything that comes through \
                 a tunnel; restart it with `malac daemon restart --remote` or set web.remote = true"
            );
        }
        let mut slot = self.tunnel.lock().await;
        if let Some(old) = slot.take() {
            old.stop().await;
        }
        let hub = Arc::downgrade(self);
        let program = spec.program.display().to_string();
        let running = tunnel::Running::start(spec, move |status| {
            if let Some(hub) = hub.upgrade() {
                *hub.tunnel_status.lock().unwrap() = Some(status);
                hub.remote_changed();
            }
        })
        .with_context(|| format!("cannot run {program}"))?;
        let status = running.status();
        *self.tunnel_status.lock().unwrap() = Some(status.clone());
        *slot = Some(running);
        drop(slot);
        self.remote_changed();
        Ok(status)
    }

    /// Stops the tunnel; `false` when none runs.
    pub async fn stop_tunnel(&self) -> bool {
        let running = self.tunnel.lock().await.take();
        match running {
            Some(running) => {
                running.stop().await;
                true
            }
            None => false,
        }
    }

    pub fn awake_status(&self) -> AwakeStatus {
        self.awake.lock().unwrap().status().clone()
    }

    /// Replaces how keep-awake holds its assertion, for embedding and tests.
    pub fn set_awake_runner(&self, runner: Arc<dyn Runner>, grace: Duration) {
        let mode = self.awake_status().mode;
        *self.awake.lock().unwrap() = Awake::new(mode, runner, grace);
        self.update_awake(None);
    }

    /// Agents and jobs running anywhere in the daemon.
    fn work(&self) -> Work {
        Work {
            agents: self
                .loaded()
                .iter()
                .flat_map(|r| r.agents())
                .filter(|a| a.status == AgentStatus::Running)
                .count(),
            jobs: self.host.jobs.running(),
        }
    }

    /// Brings keep-awake in line with the work, changing its mode first
    /// when given one. It may start a process, so call it off the runtime.
    fn update_awake(&self, mode: Option<KeepAwake>) {
        let work = self.work();
        let changed = {
            let mut awake = self.awake.lock().unwrap();
            match mode {
                Some(mode) => awake.set_mode(mode, work, Instant::now()),
                None => awake.update(work, Instant::now()),
            }
        };
        if changed {
            self.publish(&ServerMessage::Awake {
                awake: self.awake_status(),
            });
        }
    }

    /// Updates keep-awake when work starts or ends, and every few seconds.
    fn watch_awake(self: &Arc<Self>) {
        let hub = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let Some(strong) = hub.upgrade() else { break };
                if strong.stop.borrow().is_some() {
                    break;
                }
                let checking = strong.clone();
                let _ = tokio::task::spawn_blocking(move || checking.update_awake(None)).await;
                tokio::select! {
                    _ = strong.activity.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
            }
        });
    }

    // Snapshots and views

    pub fn snapshot(&self, conn: &Conn) -> Snapshot {
        let workspace = self
            .conn_workspace(conn)
            .and_then(|id| self.workspace(&id))
            .map(|w| self.workspace_view(&w));
        let (models, default_model) = {
            let base = self.base.read().unwrap();
            (configured_models(&base), base.model.clone())
        };
        Snapshot {
            client_ref: None,
            daemon: self.status(conn.version.as_deref()),
            workspaces: self.summaries(),
            workspace,
            pending: self.pending(),
            jobs: self.jobs(),
            update: update::status(),
            control: self.control.lock().unwrap().clone(),
            models,
            default_model,
            remote: self.remote_status(),
            awake: self.awake_status(),
            voice: self.voice.status(),
        }
    }

    /// Every agent loaded in any tab, subagents included.
    pub fn live_agents(&self) -> Vec<AgentSnapshot> {
        self.loaded().iter().flat_map(|r| r.agents()).collect()
    }

    pub fn pending_requests(&self) -> Vec<PermissionRequest> {
        self.approvals.pending()
    }

    pub fn agent_busy(&self, agent: &str) -> bool {
        self.live_agent(agent).is_some_and(|(_, h)| h.is_busy())
    }

    /// Removes a deleted session from the tabs that list it.
    pub fn forget_agent(&self, agent: &str) {
        let tabs: Vec<TabId> = {
            let state = self.state.lock().unwrap();
            state
                .workspaces
                .iter()
                .flat_map(|w| &w.tabs)
                .filter(|t| t.agents.iter().any(|a| a == agent))
                .map(|t| t.id.clone())
                .collect()
        };
        for tab in tabs {
            let result = self.update_tab(&tab, |t| {
                t.agents.retain(|a| a != agent);
                if t.active_agent.as_deref() == Some(agent) {
                    t.active_agent = t.agents.last().cloned();
                }
                Ok(())
            });
            if let Err(err) = result {
                tracing::warn!("cannot update tab {tab}: {err:#}");
            }
        }
    }

    pub fn job_output(
        &self,
        job: &str,
        tail: Option<usize>,
    ) -> Result<malac_core::jobs::JobOutput> {
        self.host
            .jobs
            .output(job, tail.unwrap_or(DEFAULT_TAIL_LINES))
    }

    pub fn summaries(&self) -> Vec<WorkspaceSummary> {
        let workspaces = self.state.lock().unwrap().workspaces.clone();
        workspaces.iter().map(|w| self.summary(w)).collect()
    }

    /// Broadcasts `workspace_updated` when the workspace's number of running
    /// agents differs from the one last announced.
    fn running_changed(&self, workspace: &str) {
        let Some(current) = self.workspace(workspace) else {
            return;
        };
        let summary = self.summary(&current);
        let changed = self
            .announced
            .lock()
            .unwrap()
            .insert(workspace.to_owned(), summary.running)
            != Some(summary.running);
        if changed {
            self.publish(&ServerMessage::WorkspaceUpdated { workspace: summary });
        }
    }

    fn summary(&self, workspace: &Workspace) -> WorkspaceSummary {
        let running = workspace
            .tabs
            .iter()
            .filter_map(|t| self.loaded_runtime(&t.id))
            .flat_map(|r| r.agents())
            .filter(|a| a.status == AgentStatus::Running)
            .count();
        WorkspaceSummary {
            id: workspace.id.clone(),
            name: workspace.name.clone(),
            tabs: workspace.tabs.len(),
            active_tab: workspace.active_tab.clone(),
            running,
            created: workspace.created,
            updated: workspace.updated,
        }
    }

    fn workspace_view(&self, workspace: &Workspace) -> WorkspaceView {
        WorkspaceView {
            id: workspace.id.clone(),
            name: workspace.name.clone(),
            active_tab: workspace.active_tab.clone(),
            created: workspace.created,
            updated: workspace.updated,
            tabs: workspace.tabs.iter().map(|t| self.tab_view(t)).collect(),
        }
    }

    fn tab_view(&self, tab: &Tab) -> TabView {
        let runtime = self.loaded_runtime(&tab.id);
        let live: Vec<AgentSnapshot> = runtime.as_ref().map(|r| r.agents()).unwrap_or_default();
        let mut agents = tab.agents.clone();
        // A terminal session's tab keeps no list; its runtime's agents are it.
        if self.is_attached() {
            for snap in live.iter().filter(|s| s.info.parent.is_none()) {
                if !agents.contains(&snap.info.id) {
                    agents.push(snap.info.id.clone());
                }
            }
        }
        let mut snapshots: Vec<AgentView> = agents
            .iter()
            .filter_map(|id| match live.iter().find(|s| &s.info.id == id) {
                Some(snap) => Some(AgentView {
                    snapshot: snap.clone(),
                    live: true,
                    title: String::new(),
                }),
                None => saved_agent(id),
            })
            .collect();
        snapshots.extend(
            live.iter()
                .filter(|s| s.info.parent.is_some())
                .map(|snap| AgentView {
                    snapshot: snap.clone(),
                    live: true,
                    title: String::new(),
                }),
        );
        let mode = match &runtime {
            Some(runtime) => runtime.policy.mode(),
            None => match tab.options.mode {
                Some(mode) => mode,
                None => self.base.read().unwrap().mode,
            },
        };
        TabView {
            id: tab.id.clone(),
            name: tab.name.clone(),
            cwd: tab.cwd.clone(),
            active_agent: tab.active_agent.clone().or_else(|| agents.last().cloned()),
            agents,
            mode,
            options: tab.options.clone(),
            loaded: runtime.is_some(),
            project: runtime.as_ref().map(|r| r.project_summary()),
            snapshots,
        }
    }

    fn pending(&self) -> Vec<PendingApproval> {
        self.approvals
            .pending()
            .into_iter()
            .map(|request| PendingApproval {
                tab: self.tab_of_agent(&request.agent),
                request,
            })
            .collect()
    }

    pub fn jobs(&self) -> Vec<JobView> {
        self.host
            .jobs
            .list()
            .into_iter()
            .map(|job| self.job_view(job))
            .collect()
    }

    fn job_view(&self, job: JobInfo) -> JobView {
        JobView {
            tab: job.agent.as_deref().and_then(|a| self.tab_of_agent(a)),
            job,
        }
    }

    // Lookups

    fn workspace(&self, id: &str) -> Option<Workspace> {
        self.state
            .lock()
            .unwrap()
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .cloned()
    }

    /// The workspace and a copy of the tab.
    fn find_tab(&self, tab: &str) -> Option<(WorkspaceId, Tab)> {
        let state = self.state.lock().unwrap();
        state
            .workspaces
            .iter()
            .find_map(|w| Some((w.id.clone(), w.tab(tab)?.clone())))
    }

    fn tab(&self, tab: &str) -> Result<(WorkspaceId, Tab)> {
        self.find_tab(tab).with_context(|| format!("no tab {tab}"))
    }

    /// Resolves an id, a unique id prefix or a name.
    fn resolve_workspace(&self, selector: &str) -> Result<WorkspaceId> {
        let state = self.state.lock().unwrap();
        let all = &state.workspaces;
        if let Some(w) = all.iter().find(|w| w.id == selector) {
            return Ok(w.id.clone());
        }
        if let Some(w) = all.iter().find(|w| w.name == selector) {
            return Ok(w.id.clone());
        }
        let folded: Vec<&Workspace> = all
            .iter()
            .filter(|w| w.name.eq_ignore_ascii_case(selector))
            .collect();
        if let [w] = folded.as_slice() {
            return Ok(w.id.clone());
        }
        let prefixed: Vec<&Workspace> = all
            .iter()
            .filter(|w| selector.len() >= 4 && w.id.starts_with(selector))
            .collect();
        match prefixed.as_slice() {
            [w] => Ok(w.id.clone()),
            [] => {
                let names: Vec<&str> = all.iter().map(|w| w.name.as_str()).collect();
                if names.is_empty() {
                    bail!("no workspace `{selector}`; there are none yet")
                }
                bail!("no workspace `{selector}`; there are: {}", names.join(", "))
            }
            _ => bail!("`{selector}` matches several workspaces; use more of the id"),
        }
    }

    /// The tab holding an agent: the runtime it lives in, else the tab that
    /// lists it.
    fn tab_of_agent(&self, agent: &str) -> Option<TabId> {
        let runtimes: Vec<(TabId, Arc<Runtime>)> = self.loaded_with_ids();
        if let Some((tab, _)) = runtimes.iter().find(|(_, r)| r.agent(agent).is_some()) {
            return Some(tab.clone());
        }
        let state = self.state.lock().unwrap();
        state
            .workspaces
            .iter()
            .flat_map(|w| &w.tabs)
            .find(|t| t.agents.iter().any(|a| a == agent))
            .map(|t| t.id.clone())
    }

    fn live_agent(&self, agent: &str) -> Option<(TabId, AgentHandle)> {
        self.loaded_with_ids()
            .into_iter()
            .find_map(|(tab, runtime)| Some((tab, runtime.agent(agent)?)))
    }

    fn conn_workspace(&self, conn: &Conn) -> Option<WorkspaceId> {
        let state = self.state.lock().unwrap();
        conn.workspace
            .clone()
            .filter(|id| state.workspaces.iter().any(|w| &w.id == id))
            .or_else(|| state.last.clone())
            .or_else(|| state.workspaces.last().map(|w| w.id.clone()))
    }

    // Runtimes

    fn loaded(&self) -> Vec<Arc<Runtime>> {
        self.loaded_with_ids().into_iter().map(|(_, r)| r).collect()
    }

    fn loaded_with_ids(&self) -> Vec<(TabId, Arc<Runtime>)> {
        self.runtimes
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(id, cell)| Some((id.clone(), cell.get()?.clone())))
            .collect()
    }

    fn loaded_runtime(&self, tab: &str) -> Option<Arc<Runtime>> {
        self.runtimes.lock().unwrap().get(tab)?.get().cloned()
    }

    /// The tab's runtime, built and connected on first use.
    pub async fn runtime(self: &Arc<Self>, tab: &str) -> Result<Arc<Runtime>> {
        let cell = self
            .runtimes
            .lock()
            .unwrap()
            .entry(tab.to_owned())
            .or_default()
            .clone();
        let mut built = false;
        let runtime = cell
            .get_or_try_init(|| async {
                built = true;
                self.build_runtime(tab).await
            })
            .await?
            .clone();
        if built {
            self.tab_changed(tab);
        }
        Ok(runtime)
    }

    async fn build_runtime(self: &Arc<Self>, tab_id: &str) -> Result<Arc<Runtime>> {
        let (workspace, tab) = self.tab(tab_id)?;
        let loaded = Config::load(&tab.cwd, tab.options.profile.as_deref())?;
        for warning in loaded.warnings {
            self.notice(Level::Warning, warning, Some(tab_id));
        }
        let config = apply_options(loaded.config, &tab.options);
        config.main_model()?;
        let runtime = Runtime::hosted(config, tab.cwd.clone(), &self.host)?;
        for warning in &runtime.warnings {
            self.notice(Level::Warning, warning.clone(), Some(tab_id));
        }
        runtime.set_approver(self.approvals.clone());
        for (name, provider) in self.providers.lock().unwrap().iter() {
            runtime.insert_provider(name, provider.clone());
        }
        self.forward(&workspace, tab_id, &runtime);
        for server in runtime.connect_mcp().await {
            if let Some(err) = server.error {
                self.notice(
                    Level::Warning,
                    format!(
                        "MCP server `{}` unavailable in {}: {}",
                        server.name,
                        tab.name,
                        err.lines().next().unwrap_or_default()
                    ),
                    Some(tab_id),
                );
            }
        }
        Ok(runtime)
    }

    /// Relays a runtime's events to clients, tagged with its tab.
    /// Relays a runtime's events to clients, tagged with its tab, and
    /// announces the workspace's running count when a turn starts or ends.
    fn forward(self: &Arc<Self>, workspace: &str, tab: &str, runtime: &Runtime) {
        let mut events = runtime.bus.subscribe();
        let hub = Arc::downgrade(self);
        let (workspace, tab_id) = (workspace.to_owned(), tab.to_owned());
        let task = tokio::spawn(async move {
            loop {
                let (message, turn, envelope) = match events.recv().await {
                    Ok(envelope) => {
                        let turn = matches!(
                            envelope.event,
                            Event::UserMessage { .. } | Event::TurnFinished { .. }
                        );
                        let message = ServerMessage::Event {
                            tab: Some(tab_id.clone()),
                            workspace: Some(workspace.clone()),
                            envelope: Box::new(Envelope::clone(&envelope)),
                        };
                        (message, turn, Some(envelope))
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        (ServerMessage::Lagged { skipped }, false, None)
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let Some(hub) = hub.upgrade() else { break };
                hub.publish(&message);
                if let Some(envelope) = envelope {
                    hub.voice_observe(&tab_id, &envelope);
                }
                if turn {
                    hub.running_changed(&workspace);
                    hub.activity.notify_one();
                }
            }
        });
        if let Some(old) = self.forwarders.lock().unwrap().insert(tab.to_owned(), task) {
            old.abort();
        }
    }

    fn forward_host_events(self: &Arc<Self>) {
        let mut events = self.host.bus.subscribe();
        let out = self.out.clone();
        tokio::spawn(async move {
            while let Ok(envelope) = events.recv().await {
                let message = ServerMessage::Event {
                    tab: None,
                    workspace: None,
                    envelope: Box::new(Envelope::clone(&envelope)),
                };
                if let Ok(text) = serde_json::to_string(&message) {
                    let _ = out.send(Arc::from(text));
                }
            }
        });
    }

    fn forward_jobs(self: &Arc<Self>) {
        let mut events = self.host.jobs.subscribe();
        let hub = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let event = match events.recv().await {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let Some(hub) = hub.upgrade() else { break };
                let message = match event {
                    JobEvent::Started { job } => ServerMessage::JobStarted {
                        job: hub.job_view(job),
                    },
                    JobEvent::Finished { job } => ServerMessage::JobFinished {
                        job: hub.job_view(job),
                    },
                };
                hub.publish(&message);
                hub.activity.notify_one();
            }
        });
    }

    /// Checks for a newer malac once a day and announces it.
    fn start_update_checks(self: &Arc<Self>) {
        let settings = self.base.read().unwrap().update.clone();
        if !settings.check {
            return;
        }
        let bus = self.host.bus.clone();
        tokio::spawn(async move {
            loop {
                if let Ok(mut status) = update::check(false).await {
                    if status.available
                        && settings.auto
                        && status.method == update::Method::SelfUpdate
                    {
                        match update::install().await {
                            Ok(_) => status = update::status(),
                            Err(err) => tracing::warn!("automatic update failed: {err:#}"),
                        }
                    }
                    if status.available || status.installed.is_some() {
                        bus.emit("", Event::Update { status });
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
            }
        });
    }

    /// Installs the latest release; running agents keep this version.
    pub async fn install_update(&self) -> Result<update::Status> {
        update::install().await?;
        let status = update::status();
        self.host.bus.emit(
            "",
            Event::Update {
                status: status.clone(),
            },
        );
        Ok(status)
    }

    /// Connects the control service with the daemon's own configuration, so
    /// its status and permissions are known before any tab loads.
    fn start_control(self: &Arc<Self>) {
        if !self.base.read().unwrap().control.enabled {
            return;
        }
        let hub = self.clone();
        tokio::spawn(async move {
            let control = hub.check_control(false).await;
            hub.publish(&ServerMessage::Control {
                client_ref: None,
                control,
            });
        });
    }

    /// Refreshes the control service status, asking macOS for missing
    /// permissions when `request` is set.
    pub async fn check_control(&self, request: bool) -> ControlStatus {
        let config = self.base.read().unwrap().clone();
        let mut status = ControlStatus {
            enabled: config.control.enabled,
            ..Default::default()
        };
        if !status.enabled {
            *self.control.lock().unwrap() = status.clone();
            return status;
        }
        if self.host.mcp.client(CONTROL_SERVER).is_none() {
            let connected = self.host.mcp.connect_all(&config).await;
            if let Some(server) = connected.servers.iter().find(|s| s.name == CONTROL_SERVER) {
                status.error = server.error.clone();
                status.tools = server.tools;
            }
        }
        if let Some(client) = self.host.mcp.client(CONTROL_SERVER) {
            status.connected = true;
            if status.tools == 0 {
                status.tools = client.list_tools().await.map_or(0, |t| t.len());
            }
            let arguments = serde_json::json!({ "request": request });
            match client.call_tool("computer_permissions", arguments).await {
                Ok(result) if !result.is_error => {
                    let text: String = result
                        .content
                        .iter()
                        .filter_map(|c| match c {
                            malac_mcp::Content::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    status.permissions = serde_json::from_str(&text).ok();
                }
                Ok(result) => {
                    status.error = result.content.iter().find_map(|c| match c {
                        malac_mcp::Content::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                }
                Err(err) => status.error = Some(format!("{err:#}")),
            }
        }
        *self.control.lock().unwrap() = status.clone();
        status
    }

    /// The runtime settings pages read: the active tab's, else one for the
    /// home directory.
    pub async fn settings_runtime(self: &Arc<Self>) -> Result<Arc<Runtime>> {
        let tab = self
            .conn_workspace(&Conn::default())
            .and_then(|id| self.workspace(&id))
            .and_then(|w| {
                w.active_tab
                    .or_else(|| w.tabs.first().map(|t| t.id.clone()))
            });
        match tab {
            Some(tab) => self.runtime(&tab).await,
            None => {
                let config = self.base.read().unwrap().clone();
                Runtime::hosted(config, home(), &self.host)
            }
        }
    }

    /// Applies changed configuration files: idle tabs reload their runtime
    /// on next use and their agents resume from their sessions.
    pub async fn reload(&self) -> Result<()> {
        if self.is_attached() {
            bail!("saved; restart this malac session to apply the changes");
        }
        let _guard = self.reload_lock.lock().await;
        let running = self
            .loaded()
            .iter()
            .flat_map(|r| r.agents())
            .filter(|a| a.status == AgentStatus::Running)
            .count();
        if running > 0 {
            bail!("saved, but {running} agents are running; stop them or wait, then apply again");
        }
        *self.base.write().unwrap() = base_config()?;
        self.voice_changed(&self.base_config(), false);
        let tabs: Vec<TabId> = self.runtimes.lock().unwrap().keys().cloned().collect();
        for tab in tabs {
            self.unload(&tab).await;
            self.tab_changed(&tab);
        }
        self.publish(&ServerMessage::Reloaded);
        Ok(())
    }

    async fn unload(&self, tab: &str) {
        let cell = self.runtimes.lock().unwrap().remove(tab);
        if let Some(handle) = self.forwarders.lock().unwrap().remove(tab) {
            handle.abort();
        }
        if let Some(runtime) = cell.and_then(|c| c.get().cloned()) {
            runtime.shutdown().await;
        }
    }

    // Commands

    /// Runs one client command and returns the reply, if it has one.
    pub async fn execute(
        self: &Arc<Self>,
        conn: &mut Conn,
        command: Command,
    ) -> Result<Option<ServerMessage>> {
        if let Some(listener) = conn.listener.filter(|_| !matches!(command, Command::Ping)) {
            self.voice.touch(listener);
        }
        let reply = match command {
            Command::Prompt {
                tab,
                agent,
                text,
                images,
                attachments,
                model,
            } => {
                self.prompt(conn, tab, agent, text, images, attachments, model)
                    .await?
            }
            Command::AgentStart { tab, model, label } => {
                let handle = self.start_agent(&tab, model, label).await?;
                self.ack(Some(handle.id), Some(tab))
            }
            Command::AgentOpen { agent, tab } => {
                let (tab, handle) = self.open_agent(conn, &agent, tab.as_deref()).await?;
                self.activate_agent(&tab, &handle.id)?;
                self.ack(Some(handle.id), Some(tab))
            }
            Command::Resume { session } => {
                let (tab, handle) = self.open_agent(conn, &session, None).await?;
                self.activate_agent(&tab, &handle.id)?;
                self.ack(Some(handle.id), Some(tab))
            }
            Command::AgentActivate { tab, agent } => {
                self.activate_agent(&tab, &agent)?;
                self.ack(Some(agent), Some(tab))
            }
            Command::AgentClose { tab, agent } => {
                self.close_agent(&tab, &agent)?;
                self.ack(Some(agent), Some(tab))
            }
            Command::Cancel { agent } => {
                if let Some(runtime) = self
                    .live_agent(&agent)
                    .and_then(|(tab, _)| self.loaded_runtime(&tab))
                {
                    runtime.cancel_tree(&agent);
                }
                return Ok(None);
            }
            Command::Approve {
                request_id,
                decision,
            } => {
                if !self.approvals.resolve(&request_id, decision) {
                    bail!("no pending permission request {request_id}; it was already answered");
                }
                return Ok(None);
            }
            Command::Mode { mode, tab } => {
                self.set_mode(conn, mode, tab.as_deref())?;
                return Ok(None);
            }
            Command::Model { agent, model } => {
                let (_, handle) = self.open_agent(conn, &agent, None).await?;
                handle.set_model(&model);
                return Ok(None);
            }
            Command::Compact { agent } => {
                let (_, handle) = self.open_agent(conn, &agent, None).await?;
                handle.compact();
                return Ok(None);
            }
            Command::Launch(launch) => self.launch(conn, launch).await?,
            Command::Snapshot { workspace } => {
                if let Some(selector) = workspace {
                    conn.workspace = Some(self.resolve_workspace(&selector)?);
                }
                ServerMessage::Snapshot(Box::new(self.snapshot(conn)))
            }
            Command::WorkspaceList => ServerMessage::Workspaces {
                client_ref: None,
                workspaces: self.summaries(),
            },
            Command::WorkspaceCreate { name } => {
                let id = self.create_workspace(name.as_deref())?;
                self.open_workspace(conn, &id)?
            }
            Command::WorkspaceOpen { workspace } => {
                let id = self.resolve_workspace(&workspace)?;
                self.open_workspace(conn, &id)?
            }
            Command::WorkspaceRename { workspace, name } => {
                let id = self.resolve_workspace(&workspace)?;
                let name = clean_name(&name)?;
                let workspace = self.update_workspace(&id, |w| {
                    w.name = name;
                    Ok(())
                })?;
                self.publish(&ServerMessage::WorkspaceUpdated {
                    workspace: self.summary(&workspace),
                });
                self.ack(None, None)
            }
            Command::WorkspaceDelete { workspace, force } => {
                let id = self.resolve_workspace(&workspace)?;
                self.delete_workspace(&id, force).await?;
                if conn.workspace.as_deref() == Some(id.as_str()) {
                    conn.workspace = None;
                }
                self.ack(None, None)
            }
            Command::TabCreate(spec) => {
                let tab = self.create_tab(conn, spec)?;
                self.ack(None, Some(tab))
            }
            Command::TabClose { tab, force } => {
                self.close_tab(&tab, force).await?;
                self.ack(None, Some(tab))
            }
            Command::TabRename { tab, name } => {
                let name = clean_name(&name)?;
                self.update_tab(&tab, |t| {
                    t.name = name;
                    Ok(())
                })?;
                self.ack(None, Some(tab))
            }
            Command::TabMove { tab, index } => {
                self.move_tab(&tab, index)?;
                self.ack(None, Some(tab))
            }
            Command::TabActivate { tab } => {
                self.activate_tab(&tab)?;
                self.ack(None, Some(tab))
            }
            Command::JobList => ServerMessage::Jobs {
                client_ref: None,
                jobs: self.jobs(),
            },
            Command::JobOutput { job, tail } => ServerMessage::JobOutput {
                client_ref: None,
                output: Box::new(
                    self.host
                        .jobs
                        .output(&job, tail.unwrap_or(DEFAULT_TAIL_LINES))?,
                ),
            },
            Command::JobKill { job } => {
                self.host.jobs.kill(&job)?;
                self.ack(None, None)
            }
            Command::ControlPermissions { request } => {
                let control = self.check_control(request).await;
                self.publish(&ServerMessage::Control {
                    client_ref: None,
                    control: control.clone(),
                });
                ServerMessage::Control {
                    client_ref: None,
                    control,
                }
            }
            Command::KeepAwake { mode } => {
                if self.is_attached() {
                    bail!("only the daemon keeps the machine awake");
                }
                let hub = self.clone();
                tokio::task::spawn_blocking(move || hub.update_awake(Some(mode))).await?;
                self.ack(None, None)
            }
            Command::VoiceListen { on } => {
                match (on, conn.listener) {
                    (true, None) => conn.listener = Some(self.voice.listen()),
                    (false, Some(listener)) => {
                        self.voice.unlisten(listener);
                        conn.listener = None;
                    }
                    _ => {}
                }
                ServerMessage::VoiceListening {
                    client_ref: None,
                    listener: conn.listener,
                }
            }
            Command::Ping => ServerMessage::Pong { client_ref: None },
        };
        Ok(Some(reply))
    }

    #[allow(clippy::too_many_arguments)]
    async fn prompt(
        self: &Arc<Self>,
        conn: &Conn,
        tab: Option<TabId>,
        agent: Option<AgentId>,
        text: String,
        images: Vec<ImageInput>,
        attachments: Vec<AttachmentInput>,
        model: Option<String>,
    ) -> Result<ServerMessage> {
        let (tab, existing) = match agent {
            Some(agent) => {
                let (tab, handle) = self.open_agent(conn, &agent, tab.as_deref()).await?;
                (tab, Some(handle))
            }
            None => match tab {
                Some(tab) => (tab, None),
                None => (self.default_tab(conn)?, None),
            },
        };
        // Attachments are read before a new agent starts, so a bad path
        // leaves nothing behind.
        let cwd = match &existing {
            Some(handle) => handle.info().cwd.clone(),
            None => self.tab(&tab)?.1.cwd,
        };
        uploads::check(
            attachments
                .iter()
                .filter_map(|a| Some((a.path.as_str(), a.data.as_deref()?))),
        )?;
        let mut content = Vec::new();
        let mut attached = Vec::new();
        for input in attachments {
            let attachment = match input.data {
                // A file from a client whose disk this daemon cannot read,
                // shown under the client's name for it.
                Some(data) => {
                    let saved = tokio::task::spawn_blocking(move || {
                        uploads::save(&uploads::dir(), &input.path, &data)
                    })
                    .await??;
                    let folder = saved.parent().unwrap_or(&saved).to_owned();
                    attach::attach(&saved, &folder)?
                }
                None => attach::attach(&attach::resolve(&input.path, &cwd), &cwd)?,
            };
            content.extend(attachment.blocks.iter().cloned());
            attached.push(attachment);
        }
        for image in images {
            if let Some(name) = &image.name {
                let bytes = image.data.len() as u64 * 3 / 4;
                content.push(Block::text(attach::image_header(name, bytes)));
            }
            content.push(Block::Image {
                media_type: image.media_type,
                data: image.data,
            });
        }
        content.push(Block::text(text));
        let handle = match existing {
            Some(handle) => handle,
            None => self.start_agent(&tab, model, None).await?,
        };
        handle.send(content);
        self.activate_agent(&tab, &handle.id)?;
        Ok(ServerMessage::Ack {
            client_ref: None,
            agent: Some(handle.id),
            workspace: self.find_tab(&tab).map(|(w, _)| w),
            tab: Some(tab),
            attachments: attached,
        })
    }

    async fn start_agent(
        self: &Arc<Self>,
        tab: &str,
        model: Option<String>,
        label: Option<String>,
    ) -> Result<AgentHandle> {
        let runtime = self.runtime(tab).await?;
        let handle = runtime.spawn_agent(AgentSpec {
            model,
            label: label.unwrap_or_default(),
            ..Default::default()
        })?;
        if !self.is_attached() {
            self.update_tab(tab, |t| {
                t.add_agent(&handle.id);
                Ok(())
            })?;
        }
        Ok(handle)
    }

    /// Finds or loads an agent: live in some tab, listed by a tab, or a saved
    /// session opened in a tab for its directory.
    async fn open_agent(
        self: &Arc<Self>,
        conn: &Conn,
        agent: &str,
        tab: Option<&str>,
    ) -> Result<(TabId, AgentHandle)> {
        if let Some(found) = self.live_agent(agent) {
            return Ok(found);
        }
        let tab = match tab.map(str::to_owned).or_else(|| self.tab_of_agent(agent)) {
            Some(tab) => tab,
            None => {
                let summary =
                    session::summary(agent).with_context(|| format!("no session {agent}"))?;
                let workspace = self.ensure_workspace(conn)?;
                self.tab_for_dir(&workspace, &summary.meta.cwd, TabOptions::default())?
            }
        };
        let runtime = self.runtime(&tab).await?;
        let handle = runtime.resume_agent(agent)?;
        if handle.info().parent.is_none() && !self.is_attached() {
            self.update_tab(&tab, |t| {
                t.add_agent(&handle.id);
                Ok(())
            })?;
        }
        Ok((tab, handle))
    }

    fn activate_agent(&self, tab: &str, agent: &str) -> Result<()> {
        if self.is_attached() {
            return Ok(());
        }
        let (_, current) = self.tab(tab)?;
        if current.active_agent.as_deref() == Some(agent) {
            return Ok(());
        }
        self.update_tab(tab, |t| {
            if !t.agents.iter().any(|a| a == agent) {
                bail!("agent {agent} is not in this tab");
            }
            t.active_agent = Some(agent.to_owned());
            Ok(())
        })
    }

    /// Takes an agent out of its tab: a running one is cancelled with its
    /// subagents and unloaded; its transcript stays and can be opened again.
    fn close_agent(&self, tab: &str, agent: &str) -> Result<()> {
        self.refuse_attached()?;
        let (_, current) = self.tab(tab)?;
        let runtime = self.loaded_runtime(tab);
        let live = runtime.as_ref().is_some_and(|r| r.agent(agent).is_some());
        if !live && !current.agents.iter().any(|a| a == agent) {
            bail!("agent {agent} is not in this tab");
        }
        if let Some(runtime) = &runtime {
            runtime.remove_agent(agent);
        }
        self.update_tab(tab, |t| {
            if let Some(index) = t.agents.iter().position(|a| a == agent) {
                t.agents.remove(index);
                if t.active_agent.as_deref() == Some(agent) {
                    t.active_agent = t
                        .agents
                        .get(index.min(t.agents.len().saturating_sub(1)))
                        .cloned();
                }
            }
            Ok(())
        })
    }

    /// Sets one tab's mode, or that of every tab of the connection's workspace.
    fn set_mode(&self, conn: &Conn, mode: Mode, tab: Option<&str>) -> Result<()> {
        let (workspace, tabs): (Option<WorkspaceId>, Vec<TabId>) = match tab {
            Some(tab) => {
                let (workspace, tab) = self.tab(tab)?;
                (Some(workspace), vec![tab.id])
            }
            None => {
                let workspace = self.conn_workspace(conn);
                let tabs = workspace
                    .as_deref()
                    .and_then(|id| self.workspace(id))
                    .map(|w| w.tabs.iter().map(|t| t.id.clone()).collect())
                    .unwrap_or_default();
                (workspace, tabs)
            }
        };
        for id in &tabs {
            if let Some(runtime) = self.loaded_runtime(id) {
                runtime.policy.set_mode(mode);
            }
            if !self.is_attached() {
                self.update_tab(id, |t| {
                    t.options.mode = Some(mode);
                    Ok(())
                })?;
            }
        }
        self.publish(&ServerMessage::Mode {
            mode,
            tab: tab.map(str::to_owned),
            workspace,
        });
        Ok(())
    }

    async fn launch(self: &Arc<Self>, conn: &mut Conn, launch: Launch) -> Result<ServerMessage> {
        self.refuse_attached()?;
        let cwd = resolve_dir(&launch.cwd)?;
        let mut workspace = match &launch.workspace {
            WorkspaceChoice::New => {
                let name = match &launch.name {
                    Some(name) => name.clone(),
                    None => self.unique_workspace_name(&tab_name(&cwd, None)),
                };
                self.create_workspace(Some(&name))?
            }
            WorkspaceChoice::Open(selector) => self.resolve_workspace(selector)?,
            WorkspaceChoice::Last => self.ensure_workspace(&Conn::default())?,
        };
        conn.workspace = Some(workspace.clone());
        self.set_last(&workspace);

        let new_tab_name = match launch.workspace {
            WorkspaceChoice::New => None,
            _ => launch.name.clone(),
        };
        let session = match (&launch.session, launch.continue_last) {
            (Some(id), _) => Some(id.clone()),
            (None, true) => Some(
                session::latest_in(&cwd)?
                    .context("no previous session in this directory")?
                    .meta
                    .id,
            ),
            (None, false) => None,
        };

        let mut agent = None;
        let tab = if let Some(session) = session {
            // A session some tab already holds is opened there, in its workspace.
            let (tab, handle) = self.open_agent(conn, &session, None).await?;
            let (home, _) = self.tab(&tab)?;
            if home != workspace {
                workspace = home;
                conn.workspace = Some(workspace.clone());
                self.set_last(&workspace);
            }
            self.activate_agent(&tab, &handle.id)?;
            agent = Some(handle.id);
            Some(tab)
        } else if let Some(prompt) = launch.prompt.as_deref().filter(|p| !p.trim().is_empty()) {
            let name = new_tab_name.unwrap_or_else(|| tab_name(&cwd, Some(prompt)));
            let tab = self.add_tab(&workspace, &name, &cwd, launch.options.clone())?;
            let reply = self
                .prompt(
                    conn,
                    Some(tab.clone()),
                    None,
                    prompt.to_owned(),
                    Vec::new(),
                    launch.attachments,
                    None,
                )
                .await?;
            if let ServerMessage::Ack { agent: id, .. } = reply {
                agent = id;
            }
            Some(tab)
        } else {
            let current = self
                .workspace(&workspace)
                .context("the workspace vanished")?;
            match (current.active_tab.clone(), &new_tab_name) {
                (Some(tab), None) if current.tab(&tab).is_some() => Some(tab),
                _ if current.tabs.is_empty() || new_tab_name.is_some() => {
                    let name = new_tab_name.unwrap_or_else(|| tab_name(&cwd, None));
                    Some(self.add_tab(&workspace, &name, &cwd, launch.options.clone())?)
                }
                _ => current.tabs.first().map(|t| t.id.clone()),
            }
        };
        if let Some(tab) = &tab {
            self.activate_tab(tab)?;
            if let Some(mode) = launch.options.mode {
                self.set_mode(conn, mode, Some(tab))?;
            }
            if agent.is_none() {
                agent = self.tab(tab)?.1.active_agent;
            }
        }
        let view = self
            .workspace(&workspace)
            .map(|w| self.workspace_view(&w))
            .context("the workspace vanished")?;
        Ok(ServerMessage::Launched {
            client_ref: None,
            workspace: Box::new(view),
            tab,
            agent,
        })
    }

    fn open_workspace(&self, conn: &mut Conn, id: &str) -> Result<ServerMessage> {
        let workspace = self
            .workspace(id)
            .with_context(|| format!("no workspace {id}"))?;
        conn.workspace = Some(id.to_owned());
        self.set_last(id);
        Ok(ServerMessage::WorkspaceOpened {
            client_ref: None,
            workspace: Box::new(self.workspace_view(&workspace)),
        })
    }

    /// The connection's workspace, else the last one, else a new "default".
    fn ensure_workspace(&self, conn: &Conn) -> Result<WorkspaceId> {
        match self.conn_workspace(conn) {
            Some(id) => Ok(id),
            None => {
                let id = self.create_workspace(Some(DEFAULT_WORKSPACE))?;
                self.set_last(&id);
                Ok(id)
            }
        }
    }

    fn create_workspace(&self, name: Option<&str>) -> Result<WorkspaceId> {
        self.refuse_attached()?;
        let name = match name {
            Some(name) => clean_name(name)?,
            None => self.unique_workspace_name("workspace"),
        };
        let workspace = Workspace::new(&name);
        self.save(&workspace)?;
        let summary = self.summary(&workspace);
        let id = workspace.id.clone();
        self.state.lock().unwrap().workspaces.push(workspace);
        self.publish(&ServerMessage::WorkspaceCreated { workspace: summary });
        Ok(id)
    }

    fn unique_workspace_name(&self, base: &str) -> String {
        let state = self.state.lock().unwrap();
        let taken = |name: &str| state.workspaces.iter().any(|w| w.name == name);
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|name| !taken(name))
            .unwrap_or_else(|| base.to_owned())
    }

    async fn delete_workspace(&self, id: &str, force: bool) -> Result<()> {
        self.refuse_attached()?;
        let workspace = self
            .workspace(id)
            .with_context(|| format!("no workspace {id}"))?;
        let busy: Vec<&str> = workspace
            .tabs
            .iter()
            .filter(|t| self.loaded_runtime(&t.id).is_some_and(|r| r.is_busy()))
            .map(|t| t.name.as_str())
            .collect();
        if !busy.is_empty() && !force {
            bail!(
                "agents are working in {}; stop them first or force it",
                busy.join(", ")
            );
        }
        for tab in &workspace.tabs {
            self.unload(&tab.id).await;
        }
        if let Some(store) = &self.store {
            store.delete(id)?;
        }
        let next = {
            let mut state = self.state.lock().unwrap();
            state.workspaces.retain(|w| w.id != id);
            if state.last.as_deref() == Some(id) {
                state.last = state
                    .workspaces
                    .iter()
                    .max_by_key(|w| w.updated)
                    .map(|w| w.id.clone());
                Some(state.last.clone())
            } else {
                None
            }
        };
        if let (Some(next), Some(store)) = (next, &self.store) {
            store.set_last(next.as_deref())?;
        }
        self.publish(&ServerMessage::WorkspaceDeleted {
            workspace: id.to_owned(),
        });
        Ok(())
    }

    /// The active tab of the connection's workspace, created for the home
    /// directory when the workspace has none.
    fn default_tab(&self, conn: &Conn) -> Result<TabId> {
        let workspace = self.ensure_workspace(conn)?;
        let current = self
            .workspace(&workspace)
            .context("the workspace vanished")?;
        if let Some(tab) = current
            .active_tab
            .clone()
            .filter(|t| current.tab(t).is_some())
            .or_else(|| current.tabs.first().map(|t| t.id.clone()))
        {
            return Ok(tab);
        }
        self.tab_for_dir(&workspace, &home(), TabOptions::default())
    }

    /// Adds a tab, or with `reuse` finds one, and makes its workspace the
    /// connection's.
    fn create_tab(&self, conn: &mut Conn, spec: TabSpec) -> Result<TabId> {
        self.refuse_attached()?;
        let workspace = match &spec.workspace {
            Some(selector) => self.resolve_workspace(selector)?,
            None => self.ensure_workspace(conn)?,
        };
        conn.workspace = Some(workspace.clone());
        let cwd = resolve_dir(&spec.cwd)?;
        if spec.reuse {
            let existing = self
                .workspace(&workspace)
                .and_then(|w| w.tabs.iter().find(|t| t.cwd == cwd).map(|t| t.id.clone()));
            if let Some(tab) = existing {
                self.activate_tab(&tab)?;
                return Ok(tab);
            }
        }
        let name = match &spec.name {
            Some(name) => clean_name(name)?,
            None => tab_name(&cwd, None),
        };
        let tab = self.add_tab(&workspace, &name, &cwd, spec.options)?;
        self.activate_tab(&tab)?;
        Ok(tab)
    }

    /// An existing tab for `cwd` in the workspace, else a new one.
    fn tab_for_dir(&self, workspace: &str, cwd: &Path, options: TabOptions) -> Result<TabId> {
        let existing = self
            .workspace(workspace)
            .and_then(|w| w.tabs.iter().find(|t| t.cwd == cwd).map(|t| t.id.clone()));
        match existing {
            Some(tab) => Ok(tab),
            None => self.add_tab(workspace, &tab_name(cwd, None), cwd, options),
        }
    }

    fn add_tab(
        &self,
        workspace: &str,
        name: &str,
        cwd: &Path,
        options: TabOptions,
    ) -> Result<TabId> {
        self.refuse_attached()?;
        let tab = Tab::new(name, cwd, options);
        let id = tab.id.clone();
        let view = self.tab_view(&tab);
        let mut index = 0;
        let updated = self.update_workspace(workspace, |w| {
            w.tabs.push(tab);
            index = w.tabs.len() - 1;
            if w.active_tab.is_none() {
                w.active_tab = Some(id.clone());
            }
            Ok(())
        })?;
        self.publish(&ServerMessage::TabCreated {
            workspace: workspace.to_owned(),
            index,
            tab: Box::new(view),
        });
        self.publish(&ServerMessage::WorkspaceUpdated {
            workspace: self.summary(&updated),
        });
        Ok(id)
    }

    async fn close_tab(&self, tab: &str, force: bool) -> Result<()> {
        self.refuse_attached()?;
        let (workspace, _) = self.tab(tab)?;
        if !force && self.loaded_runtime(tab).is_some_and(|r| r.is_busy()) {
            bail!("agents are working in this tab; stop them first or force it");
        }
        self.unload(tab).await;
        let updated = self.update_workspace(&workspace, |w| {
            let index = w
                .tabs
                .iter()
                .position(|t| t.id == tab)
                .context("no such tab")?;
            w.tabs.remove(index);
            if w.active_tab.as_deref() == Some(tab) {
                w.active_tab = w
                    .tabs
                    .get(index.min(w.tabs.len().saturating_sub(1)))
                    .map(|t| t.id.clone());
            }
            Ok(())
        })?;
        self.publish(&ServerMessage::TabClosed {
            workspace: workspace.clone(),
            tab: tab.to_owned(),
        });
        self.publish(&ServerMessage::WorkspaceUpdated {
            workspace: self.summary(&updated),
        });
        Ok(())
    }

    fn move_tab(&self, tab: &str, index: usize) -> Result<()> {
        self.refuse_attached()?;
        let (workspace, _) = self.tab(tab)?;
        let updated = self.update_workspace(&workspace, |w| {
            let from = w
                .tabs
                .iter()
                .position(|t| t.id == tab)
                .context("no such tab")?;
            let moved = w.tabs.remove(from);
            w.tabs.insert(index.min(w.tabs.len()), moved);
            Ok(())
        })?;
        self.publish(&ServerMessage::TabsReordered {
            workspace,
            tabs: updated.tabs.iter().map(|t| t.id.clone()).collect(),
        });
        Ok(())
    }

    fn activate_tab(&self, tab: &str) -> Result<()> {
        if self.is_attached() {
            return Ok(());
        }
        let (workspace, _) = self.tab(tab)?;
        let current = self
            .workspace(&workspace)
            .context("the workspace vanished")?;
        if current.active_tab.as_deref() == Some(tab) {
            return Ok(());
        }
        let updated = self.update_workspace(&workspace, |w| {
            w.active_tab = Some(tab.to_owned());
            Ok(())
        })?;
        self.publish(&ServerMessage::WorkspaceUpdated {
            workspace: self.summary(&updated),
        });
        Ok(())
    }

    // State changes

    fn refuse_attached(&self) -> Result<()> {
        if self.is_attached() {
            bail!(
                "this web UI belongs to a terminal session; workspaces and tabs need the daemon \
                 (`malac web` or `malac daemon start`)"
            );
        }
        Ok(())
    }

    /// Applies `edit` to a workspace, saves it and returns the new state.
    fn update_workspace(
        &self,
        id: &str,
        edit: impl FnOnce(&mut Workspace) -> Result<()>,
    ) -> Result<Workspace> {
        let updated = {
            let mut state = self.state.lock().unwrap();
            let workspace = state
                .workspaces
                .iter_mut()
                .find(|w| w.id == id)
                .with_context(|| format!("no workspace {id}"))?;
            let mut copy = workspace.clone();
            edit(&mut copy)?;
            copy.touch();
            *workspace = copy.clone();
            copy
        };
        self.save(&updated)?;
        Ok(updated)
    }

    /// Applies `edit` to a tab, saves it and announces the change.
    fn update_tab(&self, tab: &str, edit: impl FnOnce(&mut Tab) -> Result<()>) -> Result<()> {
        let (workspace, _) = self.tab(tab)?;
        self.update_workspace(&workspace, |w| edit(w.tab_mut(tab).context("no such tab")?))?;
        self.tab_changed(tab);
        Ok(())
    }

    fn tab_changed(&self, tab: &str) {
        if let Some((workspace, tab)) = self.find_tab(tab) {
            self.publish(&ServerMessage::TabUpdated {
                workspace,
                tab: Box::new(self.tab_view(&tab)),
            });
        }
    }

    fn save(&self, workspace: &Workspace) -> Result<()> {
        match &self.store {
            Some(store) => store.save(workspace),
            None => Ok(()),
        }
    }

    fn set_last(&self, id: &str) {
        self.state.lock().unwrap().last = Some(id.to_owned());
        if let Some(store) = &self.store {
            if let Err(err) = store.set_last(Some(id)) {
                tracing::warn!("cannot save the last workspace: {err:#}");
            }
        }
    }
}

/// Keeps the client count while a connection is open.
pub struct ClientGuard<'a>(&'a Hub);

impl Drop for ClientGuard<'_> {
    fn drop(&mut self) {
        let clients = self.0.clients.fetch_sub(1, Ordering::SeqCst) - 1;
        self.0.publish(&ServerMessage::Clients { clients });
    }
}

/// The daemon's own configuration: the user's, without a project's.
fn base_config() -> Result<Config> {
    Config::load_user()
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// A directory on the daemon's machine. Clients elsewhere send `~`, `~/…`
/// or a path relative to the home directory, since theirs means nothing here.
fn resolve_dir(dir: &Path) -> Result<PathBuf> {
    let text = dir.to_string_lossy();
    let path = match text.as_ref() {
        "" | "~" => home(),
        text => attach::resolve(text, &home()),
    };
    let path = path
        .canonicalize()
        .with_context(|| format!("cannot open {} on the daemon's machine", dir.display()))?;
    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    Ok(path)
}

fn apply_options(mut config: Config, options: &TabOptions) -> Config {
    if let Some(model) = &options.model {
        config.model = Some(model.clone());
    }
    if let Some(mode) = options.mode {
        config.mode = mode;
    }
    if options.no_control {
        config.control.enabled = false;
    }
    config
}

fn configured_models(config: &Config) -> Vec<String> {
    config
        .providers
        .iter()
        .flat_map(|(name, p)| p.models.iter().map(move |m| format!("{name}/{m}")))
        .collect()
}

/// A saved session shown as an idle agent.
fn saved_agent(id: &str) -> Option<AgentView> {
    let summary = session::summary(id)?;
    let meta = summary.meta;
    Some(AgentView {
        snapshot: AgentSnapshot {
            info: AgentInfo {
                id: meta.id,
                label: meta.label,
                parent: meta.parent,
                depth: 0,
                read_only: false,
                cwd: meta.cwd,
                created: meta.created,
            },
            model: meta.model,
            status: AgentStatus::Idle,
            activity: String::new(),
            usage: Usage::default(),
            report: None,
        },
        live: false,
        title: summary.title,
    })
}

fn clean_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("the name is empty");
    }
    Ok(truncate_chars(name, 80))
}

/// A short tab name: the directory, plus the first words of the prompt.
pub fn tab_name(cwd: &Path, prompt: Option<&str>) -> String {
    let dir = cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.display().to_string());
    match prompt.map(str::trim).filter(|p| !p.is_empty()) {
        Some(prompt) => {
            let words: Vec<&str> = prompt.split_whitespace().take(4).collect();
            truncate_chars(&format!("{dir}: {}", words.join(" ")), 40)
        }
        None => dir,
    }
}

/// Holds permission requests until a client answers. Every client sees
/// them through the event stream; the first answer wins.
#[derive(Default)]
pub struct Approvals {
    waiting: Mutex<HashMap<String, (PermissionRequest, oneshot::Sender<Decision>)>>,
}

impl Approvals {
    pub fn pending(&self) -> Vec<PermissionRequest> {
        let mut pending: Vec<PermissionRequest> = self
            .waiting
            .lock()
            .unwrap()
            .values()
            .map(|(r, _)| r.clone())
            .collect();
        pending.sort_by(|a, b| a.id.cmp(&b.id));
        pending
    }

    /// Answers a request; `false` when it is not (or no longer) pending.
    pub fn resolve(&self, id: &str, decision: Decision) -> bool {
        match self.waiting.lock().unwrap().remove(id) {
            Some((_, tx)) => tx.send(decision).is_ok(),
            None => false,
        }
    }
}

#[async_trait]
impl Approver for Approvals {
    async fn approve(&self, request: &PermissionRequest) -> Decision {
        let (tx, rx) = oneshot::channel();
        self.waiting
            .lock()
            .unwrap()
            .insert(request.id.clone(), (request.clone(), tx));
        // Forget the request if another approver answers first.
        struct Forget<'a>(&'a Approvals, &'a str);
        impl Drop for Forget<'_> {
            fn drop(&mut self) {
                self.0.waiting.lock().unwrap().remove(self.1);
            }
        }
        let _forget = Forget(self, &request.id);
        rx.await.unwrap_or(Decision::Deny)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_tabs_after_the_directory_and_prompt() {
        let cwd = Path::new("/src/malac");
        assert_eq!(tab_name(cwd, None), "malac");
        assert_eq!(tab_name(cwd, Some("  ")), "malac");
        assert_eq!(
            tab_name(cwd, Some("fix the flaky test in the parser module")),
            "malac: fix the flaky test"
        );
        assert_eq!(tab_name(Path::new("/"), None), "/");
    }

    #[test]
    fn approvals_go_to_the_first_answer() {
        let approvals = Arc::new(Approvals::default());
        let request = PermissionRequest {
            id: "r1".into(),
            call_id: "c1".into(),
            agent: "a".into(),
            agent_label: "main".into(),
            tool: "bash".into(),
            summary: "ls".into(),
            input: serde_json::json!({}),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let waiting = {
                let approvals = approvals.clone();
                let request = request.clone();
                tokio::spawn(async move { approvals.approve(&request).await })
            };
            while approvals.pending().is_empty() {
                tokio::task::yield_now().await;
            }
            assert!(approvals.resolve("r1", Decision::Allow));
            assert!(!approvals.resolve("r1", Decision::Deny));
            assert_eq!(waiting.await.unwrap(), Decision::Allow);
            assert!(approvals.pending().is_empty());
        });
    }

    #[test]
    fn options_override_the_configuration() {
        let config = apply_options(
            Config::default(),
            &TabOptions {
                model: Some("lora/fast".into()),
                mode: Some(Mode::Autopilot),
                profile: None,
                no_control: true,
            },
        );
        assert_eq!(config.model.as_deref(), Some("lora/fast"));
        assert_eq!(config.mode, Mode::Autopilot);
        assert!(!config.control.enabled);
    }
}
