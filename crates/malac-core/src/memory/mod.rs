//! Long-term memory. Markdown files are the source of truth; an optional
//! Milvus collection indexes them for semantic recall.

mod milvus;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::config::{MemoryConfig, VectorConfig};
use crate::provider::Provider;

pub use milvus::Milvus;

const INDEX_FILE: &str = "MEMORY.md";
const EMBED_CHARS: usize = 8000;
/// Cosine similarity below which recalled memories are not attached.
const RECALL_MIN_SCORE: f64 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Project,
}

impl Scope {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "global" | "user" => Ok(Scope::Global),
            "project" => Ok(Scope::Project),
            other => bail!("unknown memory scope `{other}` (expected global or project)"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Project => "project",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub description: String,
    pub scope: Scope,
    pub body: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub entry: Entry,
    pub score: f64,
}

pub struct Memory {
    config: MemoryConfig,
    global_dir: PathBuf,
    project_dir: PathBuf,
    project_key: String,
    vector: Option<VectorIndex>,
}

impl Memory {
    pub fn new(
        config: MemoryConfig,
        home: &Path,
        project_root: &Path,
        embedder: Option<(Arc<dyn Provider>, String)>,
    ) -> Self {
        let vector = match (&config.vector, embedder) {
            (Some(vector), Some((provider, model))) => {
                Some(VectorIndex::new(vector, provider, model))
            }
            _ => None,
        };
        Self {
            global_dir: home.join("memory"),
            project_dir: project_root.join(".malac").join("memory"),
            project_key: project_root.display().to_string(),
            vector,
            config,
        }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn has_vector_store(&self) -> bool {
        self.vector.is_some()
    }

    fn dir(&self, scope: Scope) -> &Path {
        match scope {
            Scope::Global => &self.global_dir,
            Scope::Project => &self.project_dir,
        }
    }

    pub fn list(&self) -> Vec<Entry> {
        let mut entries = Vec::new();
        for scope in [Scope::Global, Scope::Project] {
            let Ok(dir) = std::fs::read_dir(self.dir(scope)) else {
                continue;
            };
            for file in dir.flatten() {
                let path = file.path();
                let is_entry = path.extension().is_some_and(|e| e == "md")
                    && path.file_name().is_some_and(|n| n != INDEX_FILE);
                if !is_entry {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&path) {
                    entries.push(parse_entry(&text, scope, path));
                }
            }
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }

    /// Short index for the system prompt; bodies are read on demand.
    pub fn prompt_index(&self) -> String {
        let entries = self.list();
        if entries.is_empty() {
            return String::new();
        }
        entries
            .iter()
            .map(|e| {
                format!(
                    "- [{}] {}: {} ({})",
                    e.scope.as_str(),
                    e.name,
                    e.description,
                    e.path.display()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub async fn save(
        &self,
        scope: Scope,
        name: &str,
        description: &str,
        body: &str,
    ) -> Result<Entry> {
        let name = slug(name);
        if name.is_empty() {
            bail!("memory name must contain letters or digits");
        }
        let dir = self.dir(scope);
        tokio::fs::create_dir_all(dir).await?;
        let path = dir.join(format!("{name}.md"));
        let description = description.lines().next().unwrap_or_default().trim();
        let text = format!(
            "---\nname: {name}\ndescription: {description}\nupdated: {}\n---\n\n{}\n",
            Utc::now().to_rfc3339(),
            body.trim()
        );
        tokio::fs::write(&path, text).await?;
        self.write_index(scope)?;
        let entry = Entry {
            name,
            description: description.to_owned(),
            scope,
            body: body.trim().to_owned(),
            path,
        };
        if let Some(vector) = &self.vector {
            vector.upsert(&[&entry], &self.project_key).await?;
        }
        Ok(entry)
    }

    pub async fn forget(&self, scope: Scope, name: &str) -> Result<()> {
        let name = slug(name);
        let path = self.dir(scope).join(format!("{name}.md"));
        tokio::fs::remove_file(&path)
            .await
            .with_context(|| format!("no {} memory named `{name}`", scope.as_str()))?;
        self.write_index(scope)?;
        if let Some(vector) = &self.vector {
            vector
                .delete(&vector_id(scope, &self.project_key, &name))
                .await?;
        }
        Ok(())
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>> {
        match &self.vector {
            Some(vector) => {
                vector
                    .search(query, &self.project_key, limit, &self.list())
                    .await
            }
            None => Ok(keyword_search(self.list(), query, limit)),
        }
    }

    /// Semantic matches worth attaching to a prompt; empty without a vector store.
    pub async fn recall(&self, query: &str) -> Result<Vec<Hit>> {
        if !self.config.auto_recall || self.vector.is_none() || query.trim().len() < 8 {
            return Ok(Vec::new());
        }
        let hits = self.search(query, self.config.recall_limit).await?;
        Ok(hits
            .into_iter()
            .filter(|h| h.score >= RECALL_MIN_SCORE)
            .collect())
    }

    /// Re-embeds every memory file into the vector store.
    pub async fn reindex(&self) -> Result<usize> {
        let vector = self
            .vector
            .as_ref()
            .context("no vector store configured ([memory.vector] and [memory.embeddings])")?;
        let entries = self.list();
        let refs: Vec<&Entry> = entries.iter().collect();
        for chunk in refs.chunks(32) {
            vector.upsert(chunk, &self.project_key).await?;
        }
        Ok(entries.len())
    }

    fn write_index(&self, scope: Scope) -> Result<()> {
        let dir = self.dir(scope);
        let mut lines = vec!["# Memory".to_owned(), String::new()];
        for entry in self.list().into_iter().filter(|e| e.scope == scope) {
            lines.push(format!(
                "- [{}]({}.md) — {}",
                entry.name, entry.name, entry.description
            ));
        }
        std::fs::write(dir.join(INDEX_FILE), lines.join("\n") + "\n")?;
        Ok(())
    }
}

fn parse_entry(text: &str, scope: Scope, path: PathBuf) -> Entry {
    let fallback_name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (front, body) = split_frontmatter(text);
    let field = |key: &str| {
        front.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_owned())
        })
    };
    Entry {
        name: field("name").unwrap_or(fallback_name),
        description: field("description").unwrap_or_default(),
        scope,
        body: body.trim().to_owned(),
        path,
    }
}

/// Splits `---` delimited frontmatter from the body.
pub fn split_frontmatter(text: &str) -> (&str, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return ("", text);
    };
    let Some(end) = rest.find("\n---") else {
        return ("", text);
    };
    let body = rest[end + 4..]
        .split_once('\n')
        .map_or("", |(_, body)| body);
    (&rest[..end], body)
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

fn keyword_search(entries: Vec<Entry>, query: &str, limit: usize) -> Vec<Hit> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
        .map(str::to_lowercase)
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<Hit> = entries
        .into_iter()
        .filter_map(|entry| {
            let haystack =
                format!("{} {} {}", entry.name, entry.description, entry.body).to_lowercase();
            let matched = terms
                .iter()
                .filter(|t| haystack.contains(t.as_str()))
                .count();
            (matched > 0).then(|| Hit {
                score: matched as f64 / terms.len() as f64,
                entry,
            })
        })
        .collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    hits.truncate(limit);
    hits
}

fn vector_id(scope: Scope, project: &str, name: &str) -> String {
    match scope {
        Scope::Global => format!("global:{name}"),
        Scope::Project => format!("project:{project}:{name}"),
    }
}

struct VectorIndex {
    milvus: Milvus,
    embedder: Arc<dyn Provider>,
    model: String,
    ready: OnceCell<()>,
}

impl VectorIndex {
    fn new(config: &VectorConfig, embedder: Arc<dyn Provider>, model: String) -> Self {
        let token = config
            .token_env
            .as_ref()
            .and_then(|var| std::env::var(var).ok());
        Self {
            milvus: Milvus::new(
                &config.url,
                token,
                config.database.clone(),
                &config.collection,
            ),
            embedder,
            model,
            ready: OnceCell::new(),
        }
    }

    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.embedder
            .embed(&self.model, &texts)
            .await
            .context("embedding request failed")
    }

    async fn ensure_collection(&self, dimension: usize) -> Result<()> {
        self.ready
            .get_or_try_init(|| self.milvus.ensure_collection(dimension))
            .await
            .map(|_| ())
    }

    async fn upsert(&self, entries: &[&Entry], project: &str) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let texts = entries
            .iter()
            .map(|e| {
                let text = format!("{}: {}\n\n{}", e.name, e.description, e.body);
                super::tools::truncate_chars(&text, EMBED_CHARS)
            })
            .collect();
        let vectors = self.embed(texts).await?;
        self.ensure_collection(vectors[0].len()).await?;
        let rows: Vec<Value> = entries
            .iter()
            .zip(vectors)
            .map(|(e, vector)| {
                json!({
                    "id": vector_id(e.scope, project, &e.name),
                    "vector": vector,
                    "scope": e.scope.as_str(),
                    "project": if e.scope == Scope::Project { project } else { "" },
                    "name": e.name,
                })
            })
            .collect();
        self.milvus.upsert(rows).await
    }

    async fn delete(&self, id: &str) -> Result<()> {
        self.milvus.delete(id).await
    }

    async fn search(
        &self,
        query: &str,
        project: &str,
        limit: usize,
        entries: &[Entry],
    ) -> Result<Vec<Hit>> {
        let vector = self.embed(vec![query.to_owned()]).await?.remove(0);
        self.ensure_collection(vector.len()).await?;
        let filter = format!(
            "scope == \"global\" or project == \"{}\"",
            project.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let rows = self.milvus.search(vector, limit, &filter).await?;
        // Bodies come from the files, so the index never serves stale text.
        Ok(rows
            .into_iter()
            .filter_map(|(name, scope, score)| {
                let scope = Scope::parse(&scope).ok()?;
                let entry = entries
                    .iter()
                    .find(|e| e.name == name && e.scope == scope)?;
                Some(Hit {
                    entry: entry.clone(),
                    score,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Prefers Tabs!"), "prefers-tabs");
        assert_eq!(slug("  a__b  "), "a-b");
        assert_eq!(slug("!!!"), "");
    }

    #[test]
    fn frontmatter() {
        let (front, body) = split_frontmatter("---\nname: x\n---\n\nhello\n");
        assert_eq!(front, "name: x");
        assert_eq!(body.trim(), "hello");
        assert_eq!(split_frontmatter("plain"), ("", "plain"));
    }

    #[tokio::test]
    async fn save_list_search_forget() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let memory = Memory::new(MemoryConfig::default(), home.path(), project.path(), None);
        memory
            .save(
                Scope::Project,
                "Build command",
                "how to build",
                "Run `make release` for builds.",
            )
            .await
            .unwrap();
        memory
            .save(
                Scope::Global,
                "editor",
                "user editor preference",
                "Uses Helix.",
            )
            .await
            .unwrap();
        assert_eq!(memory.list().len(), 2);
        assert!(memory.prompt_index().contains("build-command"));
        let hits = memory.search("how do I build a release", 5).await.unwrap();
        assert_eq!(hits[0].entry.name, "build-command");
        let index =
            std::fs::read_to_string(project.path().join(".malac/memory/MEMORY.md")).unwrap();
        assert!(index.contains("build-command"));
        memory
            .forget(Scope::Project, "build-command")
            .await
            .unwrap();
        assert_eq!(memory.list().len(), 1);
    }
}
