//! Process-wide state shared by every agent: configuration, providers,
//! tools, memory, the event bus and the registry of live agents.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result};
use tokio::sync::Semaphore;

use crate::agent::{self, AgentHandle, AgentSnapshot};
use crate::catalog::{self, AgentType, Skill};
use crate::commands::{self, Command};
use crate::config::{self, Config};
use crate::context::instructions::Instructions;
use crate::context::{self, ProjectContext};
use crate::event::{AgentId, Event, EventBus};
use crate::jobs::Jobs;
use crate::mcp::{McpPool, ServerStatus};
use crate::memory::Memory;
use crate::permission::{Approver, DenyAll, Policy};
use crate::plugins::{self, Plugin, hooks::Hooks};
use crate::provider::{self, Provider};
use crate::session;
use crate::tools::{self, Tool};
use crate::update;

pub struct Runtime {
    pub config: Config,
    pub cwd: PathBuf,
    pub project_root: PathBuf,
    pub bus: EventBus,
    pub memory: Memory,
    pub policy: Policy,
    /// The instruction files in the system prompt, from `context`.
    pub instructions: Vec<Instructions>,
    /// Instruction files, docs, the project index and knowledge.
    pub context: ProjectContext,
    pub agent_types: Vec<AgentType>,
    pub skills: Vec<Skill>,
    /// Custom slash commands, skills among them.
    pub commands: Vec<Command>,
    /// The enabled plugins.
    pub plugins: Vec<Plugin>,
    /// Their approved hooks.
    pub hooks: Hooks,
    /// Problems with plugins and skills found while loading.
    pub warnings: Vec<String>,
    /// Background processes started by agents.
    pub jobs: Arc<Jobs>,
    /// Limits how many subagents run at once.
    pub(crate) agent_slots: Semaphore,
    providers: Mutex<HashMap<String, Arc<dyn Provider>>>,
    /// The providers' settings: `config.providers`, kept current as the
    /// user's configuration changes (see [`Runtime::update_providers`]).
    provider_configs: RwLock<BTreeMap<String, crate::config::ProviderConfig>>,
    tools: RwLock<Vec<Arc<dyn Tool>>>,
    servers: RwLock<Vec<ServerStatus>>,
    mcp: Arc<McpPool>,
    /// Whether `shutdown` closes the MCP connections, which a [`Host`] owns otherwise.
    owns_mcp: bool,
    approver: RwLock<Arc<dyn Approver>>,
    agents: Mutex<BTreeMap<AgentId, AgentHandle>>,
}

/// What the runtimes of one process share, such as the daemon's tabs: event
/// sequence numbers, the job table and MCP connections. Each runtime still
/// has its own configuration, working directory, agents and event bus.
#[derive(Clone)]
pub struct Host {
    /// Runtime buses are siblings of this one. Events that belong to no
    /// runtime, like update notices, are published here.
    pub bus: EventBus,
    pub jobs: Arc<Jobs>,
    pub mcp: Arc<McpPool>,
}

impl Host {
    /// A host whose job table is `~/.malac/jobs`.
    pub fn new() -> Self {
        Self {
            bus: EventBus::default(),
            jobs: Jobs::in_home(),
            mcp: Arc::default(),
        }
    }
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

/// What to start a new agent with.
#[derive(Debug, Clone, Default)]
pub struct AgentSpec {
    pub label: String,
    pub parent: Option<AgentId>,
    pub depth: u32,
    pub model: Option<String>,
    pub cwd: Option<PathBuf>,
    pub agent_type: Option<String>,
    /// Restrict the agent to tools that never need approval.
    pub read_only: bool,
}

impl Runtime {
    /// Builds the runtime without connecting MCP servers; see [`Runtime::connect_mcp`].
    pub fn new(config: Config, cwd: PathBuf) -> Result<Arc<Self>> {
        let host = Host::new();
        Self::build(config, cwd, host.bus, host.jobs, host.mcp, true)
    }

    /// Builds a runtime that shares `host` with others. Its MCP connections
    /// stay open on [`Runtime::shutdown`]; close them with `host.mcp`.
    pub fn hosted(config: Config, cwd: PathBuf, host: &Host) -> Result<Arc<Self>> {
        Self::build(
            config,
            cwd,
            host.bus.sibling(),
            host.jobs.clone(),
            host.mcp.clone(),
            false,
        )
    }

    fn build(
        mut config: Config,
        cwd: PathBuf,
        bus: EventBus,
        jobs: Arc<Jobs>,
        mcp: Arc<McpPool>,
        owns_mcp: bool,
    ) -> Result<Arc<Self>> {
        let home = config::home_dir();
        let project_root = config::project_root(&cwd);
        let policy = Policy::new(config.mode, &config.permissions)?;

        let loaded = plugins::load(&home, &project_root, &config.plugins);
        for (name, server) in loaded.mcp {
            // A server the user configured under the same name wins.
            config.mcp.entry(name).or_insert(server);
        }
        let user_home = dirs::home_dir();
        let context = ProjectContext::load(&config.context, &home, &project_root, &cwd);
        let (skills, skill_warnings) =
            catalog::skills(&home, user_home.as_deref(), &project_root, &loaded.plugins);
        let commands = commands::discover(
            &home,
            user_home.as_deref(),
            &project_root,
            &loaded.plugins,
            &skills,
        );
        let mut warnings = loaded.warnings;
        warnings.extend(skill_warnings);

        let embedder = match &config.memory.embeddings {
            Some(embeddings) if config.memory.vector.is_some() => {
                let provider = build_provider(&config, &embeddings.provider)
                    .context("invalid [memory.embeddings] provider")?;
                Some((provider, embeddings.model.clone()))
            }
            _ => None,
        };
        let memory = Memory::new(config.memory.clone(), &home, &project_root, embedder);

        Ok(Arc::new(Self {
            instructions: context.instructions.loaded.clone(),
            agent_types: catalog::agent_types(&home, &project_root, &loaded.plugins),
            context,
            skills,
            commands,
            plugins: loaded.plugins,
            hooks: loaded.hooks,
            warnings,
            agent_slots: Semaphore::new(config.agents.max_concurrent.max(1)),
            providers: Mutex::new(HashMap::new()),
            provider_configs: RwLock::new(config.providers.clone()),
            tools: RwLock::new(tools::builtin()),
            servers: RwLock::new(Vec::new()),
            mcp,
            owns_mcp,
            approver: RwLock::new(Arc::new(DenyAll)),
            agents: Mutex::new(BTreeMap::new()),
            bus,
            jobs,
            memory,
            policy,
            project_root,
            cwd,
            config,
        }))
    }

    /// Connects MCP servers and adds their tools. Returns per-server status.
    pub async fn connect_mcp(&self) -> Vec<ServerStatus> {
        let connected = self.mcp.connect_all(&self.config).await;
        self.tools.write().unwrap().extend(connected.tools);
        *self.servers.write().unwrap() = connected.servers.clone();
        connected.servers
    }

    /// Cancels every agent and, unless a [`Host`] owns them, closes MCP
    /// servers so they can clean up (the control service closes the
    /// browsers it launched). The agents are forgotten, so their tasks end
    /// once their turns do and the runtime can be dropped.
    pub async fn shutdown(&self) {
        let ids: Vec<AgentId> = self.agents.lock().unwrap().keys().cloned().collect();
        for id in &ids {
            self.cancel_tree(id);
        }
        self.agents.lock().unwrap().clear();
        if self.owns_mcp {
            self.mcp.close().await;
        }
    }

    /// Whether any agent is working or has work queued.
    pub fn is_busy(&self) -> bool {
        self.agents
            .lock()
            .unwrap()
            .values()
            .any(AgentHandle::is_busy)
    }

    pub fn servers(&self) -> Vec<ServerStatus> {
        self.servers.read().unwrap().clone()
    }

    pub fn mcp_instructions(&self) -> Vec<(String, String)> {
        self.servers
            .read()
            .unwrap()
            .iter()
            .filter_map(|s| Some((s.name.clone(), s.instructions.clone()?)))
            .collect()
    }

    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.read().unwrap().clone()
    }

    pub fn set_approver(&self, approver: Arc<dyn Approver>) {
        *self.approver.write().unwrap() = approver;
    }

    pub fn approver(&self) -> Arc<dyn Approver> {
        self.approver.read().unwrap().clone()
    }

    pub fn provider(&self, name: &str) -> Result<Arc<dyn Provider>> {
        let mut cache = self.providers.lock().unwrap();
        if let Some(provider) = cache.get(name) {
            return Ok(provider.clone());
        }
        let provider = {
            let configs = self.provider_configs.read().unwrap();
            let config = configs
                .get(name)
                .with_context(|| format!("unknown provider `{name}`"))?;
            provider::build(config, config::api_key(name, config))
        };
        cache.insert(name.to_owned(), provider.clone());
        Ok(provider)
    }

    /// Takes the providers of a changed user configuration without a
    /// reload: new ones become usable, changed ones (or all, when keys
    /// changed) are rebuilt on next use. Providers only the project
    /// configures stay. Requests under way finish with the old client.
    pub fn update_providers(
        &self,
        providers: &BTreeMap<String, crate::config::ProviderConfig>,
        keys_changed: bool,
    ) {
        let mut configs = self.provider_configs.write().unwrap();
        let mut cache = self.providers.lock().unwrap();
        for (name, provider) in providers {
            if configs.get(name) != Some(provider) {
                configs.insert(name.clone(), provider.clone());
                cache.remove(name);
            }
        }
        if keys_changed {
            cache.retain(|name, _| !providers.contains_key(name));
        }
    }

    /// Every provider's name, as the configuration stands now.
    pub fn provider_names(&self) -> Vec<String> {
        self.provider_configs
            .read()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    /// Checks for a newer release in the background (at most once a day) and,
    /// with `update.auto`, installs it. The outcome is published as an
    /// [`Event::Update`](crate::event::Event::Update) with an empty agent id.
    pub fn start_update_check(self: &Arc<Self>) {
        if !self.config.update.check {
            return;
        }
        let runtime = self.clone();
        tokio::spawn(async move {
            let Ok(mut status) = update::check(false).await else {
                return;
            };
            if status.available
                && runtime.config.update.auto
                && status.method == update::Method::SelfUpdate
            {
                match update::install().await {
                    Ok(_) => status = update::status(),
                    Err(err) => tracing::warn!("automatic update failed: {err:#}"),
                }
            }
            if status.available || status.installed.is_some() {
                runtime.bus.emit("", Event::Update { status });
            }
        });
    }

    /// Installs the latest release now and announces it.
    pub async fn install_update(&self) -> Result<update::Status> {
        update::install().await?;
        let status = update::status();
        self.bus.emit(
            "",
            Event::Update {
                status: status.clone(),
            },
        );
        Ok(status)
    }

    /// Drops the cached client for `name`, e.g. after its API key changed.
    pub fn forget_provider(&self, name: &str) {
        self.providers.lock().unwrap().remove(name);
    }

    /// Registers a provider under `name`, replacing the configured one.
    /// Useful for embedding malac with a custom backend, and in tests.
    pub fn insert_provider(&self, name: &str, provider: Arc<dyn Provider>) {
        self.providers
            .lock()
            .unwrap()
            .insert(name.to_owned(), provider);
    }

    /// An agent type by name, or by the part after `plugin:` when that is unique.
    pub fn agent_type(&self, name: &str) -> Option<&AgentType> {
        if let Some(exact) = self.agent_types.iter().find(|t| t.name == name) {
            return Some(exact);
        }
        let mut matches = self
            .agent_types
            .iter()
            .filter(|t| t.name.rsplit(':').next() == Some(name));
        match (matches.next(), matches.next()) {
            (Some(one), None) => Some(one),
            _ => None,
        }
    }

    /// The custom command `/name` runs; `Err` when the name is ambiguous.
    pub fn command(&self, name: &str) -> Result<Option<&Command>, String> {
        commands::find(&self.commands, name)
    }

    /// What `/context` shows: instruction files, docs, index, knowledge,
    /// skills, commands, agent types and plugins.
    pub fn context_report(&self) -> context::Report {
        let manager = plugins::Manager::new(
            &config::home_dir(),
            Some(&self.project_root),
            &self.cwd,
            self.config.plugins.defaults,
        );
        let mut warnings = self.warnings.clone();
        let plugins = match manager.list() {
            Ok(list) => list,
            Err(err) => {
                warnings.push(format!("plugins: {err:#}"));
                Vec::new()
            }
        };
        context::Report {
            layout: self.context.layout.clone(),
            instructions: self.context.instructions.clone(),
            docs: self.context.docs.clone(),
            index: self.context.index.as_ref().map(|i| i.status()),
            knowledge: context::KnowledgeStatus::of(&self.project_root),
            skills: self.skills.clone(),
            commands: self.commands.iter().map(|c| c.info.clone()).collect(),
            agent_types: self.agent_types.clone(),
            plugins: if self.config.plugins.enabled {
                plugins
            } else {
                Vec::new()
            },
            warnings,
        }
    }

    /// The small part of the context front ends show with a tab: where it
    /// sits in its repository, and its custom commands for completion.
    pub fn project_summary(&self) -> context::Summary {
        context::Summary {
            layout: self.context.layout.clone(),
            commands: self.commands.iter().map(|c| c.info.clone()).collect(),
        }
    }

    pub fn spawn_agent(self: &Arc<Self>, spec: AgentSpec) -> Result<AgentHandle> {
        let handle = agent::spawn(self.clone(), spec, None)?;
        self.register(handle.clone());
        Ok(handle)
    }

    /// Continues a saved session, or returns the agent if it is still live.
    pub fn resume_agent(self: &Arc<Self>, id: &str) -> Result<AgentHandle> {
        if let Some(handle) = self.agent(id) {
            return Ok(handle);
        }
        let transcript = session::load(id)?;
        let spec = AgentSpec {
            label: transcript.meta.label.clone(),
            parent: transcript.meta.parent.clone(),
            model: Some(transcript.meta.model.clone()),
            cwd: Some(transcript.meta.cwd.clone()),
            ..Default::default()
        };
        let handle = agent::spawn(self.clone(), spec, Some(transcript))?;
        self.register(handle.clone());
        Ok(handle)
    }

    fn register(&self, handle: AgentHandle) {
        self.agents
            .lock()
            .unwrap()
            .insert(handle.id.clone(), handle);
    }

    pub fn agent(&self, id: &str) -> Option<AgentHandle> {
        self.agents.lock().unwrap().get(id).cloned()
    }

    pub fn agents(&self) -> Vec<AgentSnapshot> {
        self.agents
            .lock()
            .unwrap()
            .values()
            .map(AgentHandle::snapshot)
            .collect()
    }

    pub fn children(&self, parent: &str) -> Vec<AgentHandle> {
        self.agents
            .lock()
            .unwrap()
            .values()
            .filter(|h| h.info().parent.as_deref() == Some(parent))
            .cloned()
            .collect()
    }

    /// Cancels an agent and its subagents and forgets them; their tasks end
    /// with their turns. Their transcripts stay on disk.
    pub fn remove_agent(&self, id: &str) {
        self.cancel_tree(id);
        let mut agents = self.agents.lock().unwrap();
        let mut doomed = vec![id.to_owned()];
        while let Some(next) = doomed.pop() {
            let children: Vec<AgentId> = agents
                .values()
                .filter(|h| h.info().parent.as_deref() == Some(next.as_str()))
                .map(|h| h.id.clone())
                .collect();
            agents.remove(&next);
            doomed.extend(children);
        }
    }

    /// Cancels the agent's turn and those of all its descendants.
    pub fn cancel_tree(&self, id: &str) {
        if let Some(handle) = self.agent(id) {
            handle.cancel();
        }
        for child in self.children(id) {
            self.cancel_tree(&child.id);
        }
    }
}

fn build_provider(config: &Config, name: &str) -> Result<Arc<dyn Provider>> {
    let provider_config = config
        .providers
        .get(name)
        .with_context(|| format!("unknown provider `{name}`"))?;
    let key = config::api_key(name, provider_config);
    Ok(provider::build(provider_config, key))
}
