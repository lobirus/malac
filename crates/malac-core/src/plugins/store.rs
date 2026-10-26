//! Where plugin state lives.
//!
//! User scope, `~/.malac/plugins/`:
//!
//! - `plugins.toml`: the marketplaces you added and the plugins you enabled
//! - `installed.json`: what is installed, from where, and which executable
//!   parts (hooks, MCP servers, scripts) you approved; written by malac
//! - `marketplaces/<name>/`: clones of git marketplaces, or their downloaded `marketplace.json`
//! - `cache/<marketplace>/<plugin>/`: installed plugins
//! - `data/<plugin>@<marketplace>/`: each plugin's `PLUGIN_DATA`
//! - `builtin/<plugin>/`: plugins that ship with malac
//!
//! Project scope, `<project>/.malac/plugins.toml`: the same format,
//! committed, so a team shares its marketplaces and plugins. Its plugins
//! still have to be installed on each machine, and their hooks and MCP
//! servers run only once the user approved them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const FILE: &str = "plugins.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    User,
    Project,
    /// Shipped with malac.
    Builtin,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Project => "project",
            Scope::Builtin => "builtin",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn new(malac_home: &Path) -> Self {
        Self {
            dir: malac_home.join("plugins"),
        }
    }

    pub fn user_file(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    pub fn installed_file(&self) -> PathBuf {
        self.dir.join("installed.json")
    }

    pub fn marketplace_dir(&self, name: &str) -> PathBuf {
        self.dir.join("marketplaces").join(name)
    }

    pub fn cache_dir(&self, marketplace: &str, plugin: &str) -> PathBuf {
        self.dir.join("cache").join(marketplace).join(plugin)
    }

    pub fn data_dir(&self, id: &str) -> PathBuf {
        let safe: String = id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "-_.@".contains(c) {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        self.dir.join("data").join(safe)
    }

    pub fn builtin_dir(&self) -> PathBuf {
        self.dir.join("builtin")
    }

    pub fn installed(&self) -> Result<Installed> {
        match std::fs::read_to_string(self.installed_file()) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid", self.installed_file().display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Installed::default()),
            Err(err) => Err(err.into()),
        }
    }

    pub fn save_installed(&self, installed: &Installed) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.installed_file();
        std::fs::write(&path, serde_json::to_string_pretty(installed)? + "\n")
            .with_context(|| format!("cannot write {}", path.display()))
    }
}

pub fn project_file(project_root: &Path) -> PathBuf {
    project_root.join(".malac").join(FILE)
}

/// `plugins.toml`, in either scope:
///
/// ```toml
/// [marketplaces]
/// lobirus = "lobirus/agent-plugins"
///
/// [plugins]
/// "server-setup@lobirus" = true
/// "ponytail@malac" = false
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginsFile {
    /// Marketplace name to source.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub marketplaces: BTreeMap<String, String>,
    /// `plugin@marketplace` to whether it is enabled.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins: BTreeMap<String, bool>,
}

const HEADER: &str = "# Plugins and marketplaces for malac; `malac plugin --help` manages this file.\n# Docs: https://github.com/lobirus/malac/blob/main/docs/plugins.md\n\n";

impl PluginsFile {
    pub fn read(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("{} is not valid", path.display()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, format!("{HEADER}{}", toml::to_string_pretty(self)?))
            .with_context(|| format!("cannot write {}", path.display()))
    }

    /// Reads, changes and writes the file at `path`.
    pub fn edit(path: &Path, change: impl FnOnce(&mut Self)) -> Result<()> {
        let mut file = Self::read(path)?;
        change(&mut file);
        file.write(path)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Installed {
    #[serde(default)]
    pub plugins: BTreeMap<String, Record>,
}

/// An installed plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    pub marketplace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Its files: in the cache, or in place for a local marketplace.
    pub path: PathBuf,
    /// Where it came from, for display and updates.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The marketplace entry, which may declare its components.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub entry: serde_json::Value,
    pub installed: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    /// Executable parts the user agreed to run: `hooks`, `mcp`, `scripts`.
    #[serde(default)]
    pub approved: Vec<String>,
}
