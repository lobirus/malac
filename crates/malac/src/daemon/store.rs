//! Workspaces on disk: `~/.malac/workspaces/<id>.json`, plus
//! `~/.malac/state.json`, which remembers the last used workspace.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use malac_core::event::AgentId;
use serde::{Deserialize, Serialize};

use crate::protocol::{TabId, TabOptions, WorkspaceId};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub tabs: Vec<Tab>,
    pub active_tab: Option<TabId>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
}

/// A project: a directory with its own runtime, and the top-level agents
/// working in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub name: String,
    pub cwd: PathBuf,
    #[serde(default)]
    pub agents: Vec<AgentId>,
    #[serde(default)]
    pub active_agent: Option<AgentId>,
    #[serde(default, flatten)]
    pub options: TabOptions,
}

impl Workspace {
    pub fn new(name: &str) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            name: name.to_owned(),
            tabs: Vec::new(),
            active_tab: None,
            created: now,
            updated: now,
        }
    }

    pub fn tab(&self, id: &str) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn tab_mut(&mut self, id: &str) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn touch(&mut self) {
        self.updated = Utc::now();
    }
}

impl Tab {
    pub fn new(name: &str, cwd: &Path, options: TabOptions) -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            name: name.to_owned(),
            cwd: cwd.to_path_buf(),
            agents: Vec::new(),
            active_agent: None,
            options,
        }
    }

    /// Adds a top-level agent and makes it the active one.
    pub fn add_agent(&mut self, id: &str) {
        if !self.agents.iter().any(|a| a == id) {
            self.agents.push(id.to_owned());
        }
        self.active_agent = Some(id.to_owned());
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    last_workspace: Option<WorkspaceId>,
}

/// Reads and writes workspace files under a malac home directory.
#[derive(Debug, Clone)]
pub struct Store {
    home: PathBuf,
}

impl Store {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    fn dir(&self) -> PathBuf {
        self.home.join("workspaces")
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            bail!("invalid workspace id `{id}`");
        }
        Ok(self.dir().join(format!("{id}.json")))
    }

    /// Every readable workspace, oldest first. Unreadable files are skipped
    /// with a warning rather than failing the daemon.
    pub fn load_all(&self) -> Vec<Workspace> {
        let Ok(entries) = fs::read_dir(self.dir()) else {
            return Vec::new();
        };
        let mut workspaces: Vec<Workspace> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .filter_map(|path| {
                let parsed = fs::read_to_string(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|text| Ok(serde_json::from_str::<Workspace>(&text)?));
                match parsed {
                    Ok(workspace) => Some(workspace),
                    Err(err) => {
                        tracing::warn!("skipping {}: {err:#}", path.display());
                        None
                    }
                }
            })
            .collect();
        workspaces.sort_by(|a, b| a.created.cmp(&b.created));
        workspaces
    }

    pub fn save(&self, workspace: &Workspace) -> Result<()> {
        let path = self.path(&workspace.id)?;
        write_atomic(&path, &serde_json::to_vec_pretty(workspace)?)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let path = self.path(id)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err).with_context(|| format!("cannot delete {}", path.display())),
        }
    }

    pub fn last(&self) -> Option<WorkspaceId> {
        fs::read_to_string(self.home.join("state.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<State>(&text).ok())?
            .last_workspace
    }

    pub fn set_last(&self, id: Option<&str>) -> Result<()> {
        let state = State {
            last_workspace: id.map(str::to_owned),
        };
        write_atomic(
            &self.home.join("state.json"),
            &serde_json::to_vec_pretty(&state)?,
        )
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_workspaces_and_the_last_one() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().to_path_buf());
        assert!(store.load_all().is_empty());
        assert_eq!(store.last(), None);

        let mut workspace = Workspace::new("default");
        let mut tab = Tab::new(
            "app",
            Path::new("/src/app"),
            TabOptions {
                model: Some("lora/fast".into()),
                ..Default::default()
            },
        );
        tab.add_agent("a1");
        tab.add_agent("a2");
        tab.add_agent("a1");
        workspace.active_tab = Some(tab.id.clone());
        workspace.tabs.push(tab);
        store.save(&workspace).unwrap();
        store.set_last(Some(&workspace.id)).unwrap();

        let loaded = store.load_all();
        assert_eq!(loaded, vec![workspace.clone()]);
        let tab = &loaded[0].tabs[0];
        assert_eq!(tab.agents, ["a1", "a2"]);
        assert_eq!(tab.active_agent.as_deref(), Some("a1"));
        assert_eq!(tab.options.model.as_deref(), Some("lora/fast"));
        assert_eq!(store.last().as_deref(), Some(workspace.id.as_str()));

        std::fs::write(home.path().join("workspaces/broken.json"), "{").unwrap();
        assert_eq!(store.load_all().len(), 1);
        store.delete(&workspace.id).unwrap();
        assert!(store.load_all().is_empty());
        assert!(store.delete("../evil").is_err());
    }
}
