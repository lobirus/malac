//! Connects configured MCP servers, including the built-in control service.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use futures::future::join_all;
use malac_mcp::Client;
use serde::{Deserialize, Serialize};

use crate::config::{self, Config, McpServerConfig, expand_env};
use crate::tools::{McpTool, Tool};

pub const CONTROL_SERVER: &str = "control";

#[derive(Debug, Clone, Serialize)]
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
    pub clients: Vec<Arc<Client>>,
}

pub async fn connect_all(config: &Config) -> Connected {
    // Each server has candidate endpoints tried in order; only the control
    // service has more than one.
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

    let attempts = targets.into_iter().map(|(name, candidates)| async move {
        let mut last_error = None;
        for server in candidates {
            match connect(&server).await {
                Ok(connected) => return (name, Ok((server.prefix, connected))),
                Err(err) => last_error = Some(err),
            }
        }
        (
            name,
            Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no endpoint"))),
        )
    });

    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    let mut servers = Vec::new();
    let mut clients = Vec::new();
    for (name, result) in join_all(attempts).await {
        match result {
            Ok((prefix, (client, remote_tools))) => {
                let client = Arc::new(client);
                clients.push(client.clone());
                let count = remote_tools.len();
                for tool in remote_tools {
                    tools.push(Arc::new(McpTool::new(&name, client.clone(), tool, prefix)));
                }
                servers.push(ServerStatus {
                    instructions: client.server_info().instructions.clone(),
                    name,
                    tools: count,
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
    Connected {
        tools,
        servers,
        clients,
    }
}

async fn connect(server: &McpServerConfig) -> Result<(Client, Vec<malac_mcp::Tool>)> {
    let token = server
        .token_env
        .as_ref()
        .and_then(|var| std::env::var(var).ok());
    let client = match (&server.command, &server.url) {
        (Some(command), _) => {
            let args: Vec<String> = server.args.iter().map(|a| expand_env(a)).collect();
            let env: HashMap<String, String> = server
                .env
                .iter()
                .map(|(k, v)| (k.clone(), expand_env(v)))
                .collect();
            Client::stdio(&expand_env(command), &args, &env, None).await?
        }
        (None, Some(url)) if url.starts_with("ws://") || url.starts_with("wss://") => {
            Client::websocket(url, token.as_deref()).await?
        }
        (None, Some(url)) => {
            let mut headers: HashMap<String, String> = server
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), expand_env(v)))
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
