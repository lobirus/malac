//! Plugins: packages of skills, slash commands, agent types, MCP servers
//! and hooks, in the Claude Code or Agent Plugins format, installed from
//! marketplaces. See `docs/plugins.md`.

pub mod builtin;
pub mod hooks;
mod manage;
pub mod manifest;
pub mod marketplace;
pub mod source;
pub mod store;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::config::{McpServerConfig, PluginsConfig};
pub use manage::{Manager, MarketplaceInfo, NeedsApproval};
pub use manifest::{CommandSource, Components, Format, Metadata};
pub use store::Scope;

/// The parts of a plugin that run code on this machine, each approved
/// separately when the plugin is enabled.
pub const EXECUTABLES: &[&str] = &["hooks", "mcp", "scripts"];

#[derive(Debug, Clone)]
pub struct Plugin {
    /// `plugin@marketplace`.
    pub id: String,
    /// The manifest name; its components are namespaced under it (`name:skill`).
    pub name: String,
    pub marketplace: String,
    pub root: PathBuf,
    /// Its writable data directory (`PLUGIN_DATA`).
    pub data: PathBuf,
    pub meta: Metadata,
    pub formats: Vec<Format>,
    pub components: Components,
    pub warnings: Vec<String>,
}

impl Plugin {
    pub fn load(id: &str, root: &Path, data: &Path, entry: Option<&Value>) -> Result<Self> {
        let parsed = manifest::parse(root, data, entry)?;
        let marketplace = id.rsplit_once('@').map_or("", |(_, m)| m).to_owned();
        Ok(Self {
            id: id.to_owned(),
            name: parsed.meta.name.clone(),
            marketplace,
            root: std::fs::canonicalize(root)?,
            data: data.to_path_buf(),
            meta: parsed.meta,
            formats: parsed.formats,
            components: parsed.components,
            warnings: parsed.warnings,
        })
    }

    /// Writes the plugin's directories in for `${CLAUDE_PLUGIN_ROOT}` and friends.
    pub fn substitute(&self, text: &str) -> String {
        crate::catalog::substitute_plugin(text, &self.root, &self.data)
    }

    /// Which of [`EXECUTABLES`] it has.
    pub fn executables(&self) -> Vec<&'static str> {
        let c = &self.components;
        let has = [
            !c.hooks.is_empty(),
            !c.mcp.is_empty(),
            !c.scripts.is_empty(),
        ];
        EXECUTABLES
            .iter()
            .zip(has)
            .filter(|(_, has)| *has)
            .map(|(name, _)| *name)
            .collect()
    }

    pub fn command_names(&self) -> Vec<String> {
        self.components
            .commands
            .iter()
            .map(|c| match c {
                CommandSource::File(path) => path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                CommandSource::Inline { name, .. } => name.clone(),
            })
            .collect()
    }

    fn component_names(paths: &[PathBuf], parent: bool) -> Vec<String> {
        paths
            .iter()
            .filter_map(|p| {
                if parent {
                    p.parent()?.file_name()
                } else {
                    p.file_stem()
                }
            })
            .map(|n| n.to_string_lossy().into_owned())
            .collect()
    }

    /// What it contains and what of it runs code, for a confirmation prompt.
    pub fn describe(&self) -> String {
        let c = &self.components;
        let mut lines = vec![format!(
            "{} {}{}",
            self.id,
            self.meta.version.as_deref().unwrap_or(""),
            self.meta
                .description
                .as_deref()
                .map(|d| format!(": {d}"))
                .unwrap_or_default()
        )];
        let mut list = |label: &str, names: Vec<String>| {
            if !names.is_empty() {
                lines.push(format!("  {label}: {}", names.join(", ")));
            }
        };
        list("skills", Self::component_names(&c.skills, true));
        list("commands", self.command_names());
        list("agents", Self::component_names(&c.agents, false));
        list(
            "MCP servers (start programs)",
            c.mcp
                .iter()
                .map(|(name, s)| match (&s.command, &s.url) {
                    (Some(command), _) => {
                        let call: Vec<&str> = std::iter::once(command.as_str())
                            .chain(s.args.iter().map(String::as_str))
                            .collect();
                        format!("{name} ({})", call.join(" "))
                    }
                    (None, Some(url)) => format!("{name} ({url})"),
                    _ => name.clone(),
                })
                .collect(),
        );
        list(
            "hooks (run commands)",
            c.hooks
                .iter()
                .map(|h| format!("{}: {}", h.event, h.command))
                .collect(),
        );
        list(
            "scripts",
            c.scripts
                .iter()
                .map(|p| {
                    p.strip_prefix(&self.root)
                        .unwrap_or(p)
                        .display()
                        .to_string()
                })
                .collect(),
        );
        list("not supported by malac", c.unsupported.clone());
        lines.join("\n")
    }

    fn info(&self, enabled: bool, scope: Option<Scope>, approved: &[String]) -> PluginInfo {
        let c = &self.components;
        let executables = self.executables();
        PluginInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            marketplace: self.marketplace.clone(),
            version: self.meta.version.clone(),
            description: self.meta.description.clone(),
            enabled,
            scope,
            installed: true,
            path: Some(self.root.clone()),
            formats: self.formats.clone(),
            skills: Self::component_names(&c.skills, true),
            commands: self.command_names(),
            agents: Self::component_names(&c.agents, false),
            mcp: c.mcp.keys().cloned().collect(),
            hooks: {
                let mut events: Vec<String> = c.hooks.iter().map(|h| h.event.clone()).collect();
                events.dedup();
                events
            },
            scripts: !c.scripts.is_empty(),
            pending: executables
                .iter()
                .filter(|e| **e != "scripts" && !approved.iter().any(|a| a == *e))
                .map(|e| e.to_string())
                .collect(),
            approved: approved.to_vec(),
            unsupported: c.unsupported.clone(),
            warnings: self.warnings.clone(),
            error: None,
        }
    }
}

/// A plugin as the UIs and `malac plugin list` show it.
#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub marketplace: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub enabled: bool,
    /// The file that enables or disables it; `builtin` when it is on by default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub formats: Vec<Format>,
    pub skills: Vec<String>,
    pub commands: Vec<String>,
    pub agents: Vec<String>,
    pub mcp: Vec<String>,
    /// Hook events.
    pub hooks: Vec<String>,
    pub scripts: bool,
    /// Executable parts the user approved.
    pub approved: Vec<String>,
    /// Hooks or MCP servers it has that were not approved, so they do not run.
    pub pending: Vec<String>,
    pub unsupported: Vec<String>,
    pub warnings: Vec<String>,
    /// Why it could not be loaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The enabled plugins of a project, ready for a runtime.
#[derive(Debug, Clone, Default)]
pub struct Loaded {
    pub plugins: Vec<Plugin>,
    pub hooks: hooks::Hooks,
    /// MCP servers to add to the configuration, keyed by runtime name.
    pub mcp: BTreeMap<String, McpServerConfig>,
    pub warnings: Vec<String>,
}

/// The name a plugin's MCP server gets in the runtime: `<plugin>` when it
/// has the plugin's name, else `<plugin>-<server>`.
pub fn mcp_name(plugin: &str, server: &str) -> String {
    let name = if server == plugin {
        plugin.to_owned()
    } else {
        format!("{plugin}-{server}")
    };
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Loads the plugins enabled for `project_root`. Hooks and MCP servers of a
/// plugin run only if the user approved them.
pub fn load(malac_home: &Path, project_root: &Path, config: &PluginsConfig) -> Loaded {
    let mut loaded = Loaded::default();
    if !config.enabled {
        return loaded;
    }
    let manager = Manager::new(
        malac_home,
        Some(project_root),
        project_root,
        config.defaults,
    );
    let states = match manager.states() {
        Ok(states) => states,
        Err(err) => {
            loaded.warnings.push(format!("plugins: {err:#}"));
            return loaded;
        }
    };
    for state in states.into_iter().filter(|s| s.enabled) {
        let (mut plugin, approved) = match manager.load_installed(&state.id) {
            Ok(found) => found,
            Err(err) => {
                loaded
                    .warnings
                    .push(format!("plugin {}: {err:#}", state.id));
                continue;
            }
        };
        for warning in &plugin.warnings {
            loaded
                .warnings
                .push(format!("plugin {}: {warning}", plugin.id));
        }
        let allowed = |part: &str| approved.iter().any(|a| a == part);
        if !plugin.components.hooks.is_empty() && !allowed("hooks") {
            loaded.warnings.push(format!(
                "plugin {}: hooks not approved, so they do not run; `malac plugin enable {}` reviews them",
                plugin.id, plugin.id
            ));
            plugin.components.hooks.clear();
        }
        if !plugin.components.mcp.is_empty() && !allowed("mcp") {
            loaded.warnings.push(format!(
                "plugin {}: MCP servers not approved, so they do not start; `malac plugin enable {}` reviews them",
                plugin.id, plugin.id
            ));
            plugin.components.mcp.clear();
        }
        loaded.warnings.extend(loaded.hooks.add(
            &plugin.name,
            &plugin.root,
            &plugin.data,
            &plugin.components.hooks,
        ));
        for (server, config) in &plugin.components.mcp {
            loaded
                .mcp
                .insert(mcp_name(&plugin.name, server), config.clone());
        }
        loaded.plugins.push(plugin);
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_mcp_servers() {
        assert_eq!(mcp_name("ponytail", "ponytail"), "ponytail");
        assert_eq!(mcp_name("reports.tools", "db"), "reports_tools-db");
    }
}
