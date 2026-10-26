//! Connects configured MCP servers, including the built-in control service.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use futures::future::join_all;
use malac_mcp::Client;
use serde::{Deserialize, Serialize};

use crate::config::{self, Config, McpServerConfig, expand_env};
use crate::tools::{McpTool, Tool};

pub const CONTROL_SERVER: &str = "control";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub name: String,
    pub tools: usize,
    pub error: Option<String>,
    #[serde(skip)]
    pub instructions: Option<String>,
}

pub struct Connected {
    pub tools: Vec<Arc<dyn Tool>>,
    pub servers: Vec<ServerStatus>,
}

/// Connections to MCP servers, keyed by each server's configuration, so the
/// runtimes of one process (the daemon's tabs) start every server once and
/// share it: one control service, one browser per profile.
#[derive(Default)]
pub struct McpPool {
    slots: Mutex<HashMap<String, Slot>>,
}

type Slot = Arc<tokio::sync::Mutex<Option<Live>>>;

#[derive(Clone)]
struct Live {
    name: String,
    client: Arc<Client>,
    tools: Vec<malac_mcp::Tool>,
    prefix: bool,
}

impl McpPool {
    /// Connects the servers `config` enables, reusing live connections.
    /// Failed connections are retried on the next call.
    pub async fn connect_all(&self, config: &Config) -> Connected {
        let attempts = targets(config).into_iter().map(|(name, candidates)| {
            let key = format!(
                "{name}\n{}",
                serde_json::to_string(&candidates).unwrap_or_default()
            );
            let slot = self.slot(&key);
            async move {
                let mut slot = slot.lock().await;
                if let Some(live) = slot.as_ref() {
                    return (name, Ok(live.clone()));
                }
                let mut last_error = None;
                for server in candidates {
                    match connect(&server).await {
                        Ok((client, tools)) => {
                            let live = Live {
                                name: name.clone(),
                                client: Arc::new(client),
                                tools,
                                prefix: server.prefix,
                            };
                            *slot = Some(live.clone());
                            return (name, Ok(live));
                        }
                        Err(err) => last_error = Some(err),
                    }
                }
                (
                    name,
                    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no endpoint"))),
                )
            }
        });

        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        let mut servers = Vec::new();
        for (name, result) in join_all(attempts).await {
            match result {
                Ok(live) => {
                    for tool in &live.tools {
                        tools.push(Arc::new(McpTool::new(
                            &name,
                            live.client.clone(),
                            tool.clone(),
                            live.prefix,
                        )));
                    }
                    servers.push(ServerStatus {
                        instructions: live.client.server_info().instructions.clone(),
                        name,
                        tools: live.tools.len(),
                        error: None,
                    });
                }
                Err(err) => servers.push(ServerStatus {
                    name,
                    tools: 0,
                    error: Some(format!("{err:#}")),
                    instructions: None,
                }),
            }
        }
        Connected { tools, servers }
    }

    /// A connected server by name, e.g. [`CONTROL_SERVER`].
    pub fn client(&self, name: &str) -> Option<Arc<Client>> {
        let slots: Vec<Slot> = self.slots.lock().unwrap().values().cloned().collect();
        slots.iter().find_map(|slot| {
            let live = slot.try_lock().ok()?;
            live.as_ref()
                .filter(|l| l.name == name)
                .map(|l| l.client.clone())
        })
    }

    /// Closes every connection so servers can clean up (the control service
    /// closes the browsers it launched).
    pub async fn close(&self) {
        let slots: Vec<Slot> = self.slots.lock().unwrap().drain().map(|(_, s)| s).collect();
        let mut clients = Vec::new();
        for slot in slots {
            if let Some(live) = slot.lock().await.take() {
                clients.push(live.client);
            }
        }
        join_all(clients.iter().map(|c| c.close())).await;
    }

    fn slot(&self, key: &str) -> Slot {
        self.slots
            .lock()
            .unwrap()
            .entry(key.to_owned())
            .or_default()
            .clone()
    }
}

/// Each enabled server with its candidate endpoints, tried in order; only
/// the control service has more than one.
fn targets(config: &Config) -> Vec<(String, Vec<McpServerConfig>)> {
    let mut targets: Vec<(String, Vec<McpServerConfig>)> = config
        .mcp
        .iter()
        .filter(|(_, server)| server.enabled)
        .map(|(name, server)| (name.clone(), vec![server.clone()]))
        .collect();
    if config.control.enabled && !config.mcp.contains_key(CONTROL_SERVER) {
        let candidates = control_candidates(config);
        if !candidates.is_empty() {
            targets.push((CONTROL_SERVER.into(), candidates));
        }
    }
    targets
}

async fn connect(server: &McpServerConfig) -> Result<(Client, Vec<malac_mcp::Tool>)> {
    let token = server
        .token_env
        .as_ref()
        .and_then(|var| std::env::var(var).ok());
    let client = match (&server.command, &server.url) {
        (Some(command), _) => {
            let expand = |value: &str| {
                if server.literal {
                    value.to_owned()
                } else {
                    expand_env(value)
                }
            };
            let args: Vec<String> = server.args.iter().map(|a| expand(a)).collect();
            let env: HashMap<String, String> = server
                .env
                .iter()
                .map(|(k, v)| (k.clone(), expand(v)))
                .collect();
            Client::stdio(&expand(command), &args, &env, server.cwd.as_deref()).await?
        }
        (None, Some(url)) if url.starts_with("ws://") || url.starts_with("wss://") => {
            Client::websocket(url, token.as_deref()).await?
        }
        (None, Some(url)) => {
            let mut headers: HashMap<String, String> = server
                .headers
                .iter()
                .map(|(k, v)| {
                    let value = if server.literal {
                        v.clone()
                    } else {
                        expand_env(v)
                    };
                    (k.clone(), value)
                })
                .collect();
            if let Some(token) = token {
                headers.insert("Authorization".into(), format!("Bearer {token}"));
            }
            Client::http(url, &headers).await?
        }
        (None, None) => bail!("set `command` or `url`"),
    };
    let tools = client.list_tools().await.context("tools/list failed")?;
    Ok((client, tools))
}

/// Discovery file written by `malac-control serve`.
#[derive(Debug, Deserialize)]
struct ControlEndpoint {
    url: String,
    token: String,
}

/// Ways to reach the control service, in order: an explicit URL, a running
/// service announced in `~/.malac/control.json`, a spawned `malac-control stdio`.
fn control_candidates(config: &Config) -> Vec<McpServerConfig> {
    let control = &config.control;
    let base = McpServerConfig {
        prefix: false,
        ..Default::default()
    };
    if let Some(url) = &control.url {
        return vec![McpServerConfig {
            url: Some(url.clone()),
            token_env: control.token_env.clone(),
            ..base
        }];
    }
    let mut candidates = Vec::new();
    let discovery = config::home_dir().join("control.json");
    if let Some(endpoint) = std::fs::read_to_string(discovery)
        .ok()
        .and_then(|text| serde_json::from_str::<ControlEndpoint>(&text).ok())
    {
        let separator = if endpoint.url.contains('?') { '&' } else { '?' };
        candidates.push(McpServerConfig {
            url: Some(format!(
                "{}{separator}token={}",
                endpoint.url, endpoint.token
            )),
            ..base.clone()
        });
    }
    if let Some(command) = control
        .command
        .clone()
        .map(PathBuf::from)
        .or_else(find_control_binary)
    {
        candidates.push(McpServerConfig {
            command: Some(command.display().to_string()),
            args: vec!["stdio".into()],
            ..base
        });
    }
    candidates
}

/// `malac-control` next to the running executable, else on PATH.
pub fn find_control_binary() -> Option<PathBuf> {
    let name = format!("malac-control{}", std::env::consts::EXE_SUFFIX);
    let sibling = std::env::current_exe().ok()?.parent()?.join(&name);
    if sibling.is_file() {
        return Some(sibling);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(&name))
            .find(|candidate| candidate.is_file())
    })
}
