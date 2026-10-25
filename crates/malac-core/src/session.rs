//! Append-only JSONL transcripts under `~/.malac/sessions`, one per agent.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config;
use crate::event::AgentId;
use crate::message::{Message, Role, Usage};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: AgentId,
    pub label: String,
    pub parent: Option<AgentId>,
    pub cwd: PathBuf,
    pub model: String,
    pub created: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    Meta(SessionMeta),
    Message { message: Message },
    Compaction { summary: String },
    Usage { usage: Usage },
}

pub fn sessions_dir() -> PathBuf {
    config::home_dir().join("sessions")
}

fn session_path(id: &str) -> Result<PathBuf> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("invalid session id `{id}`");
    }
    Ok(sessions_dir().join(format!("{id}.jsonl")))
}

pub struct SessionLog {
    file: File,
}

impl SessionLog {
    pub fn create(meta: &SessionMeta) -> Result<Self> {
        std::fs::create_dir_all(sessions_dir())?;
        let path = session_path(&meta.id)?;
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("cannot create {}", path.display()))?;
        let mut log = Self { file };
        log.append(&Record::Meta(meta.clone()));
        Ok(log)
    }

    pub fn reopen(id: &str) -> Result<Self> {
        let path = session_path(id)?;
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .with_context(|| format!("no session {id}"))?;
        Ok(Self { file })
    }

    /// Persistence failures must not stop the agent, so they are only logged.
    pub fn append(&mut self, record: &Record) {
        let result = serde_json::to_string(record)
            .map_err(anyhow::Error::from)
            .and_then(|line| Ok(writeln!(self.file, "{line}")?));
        if let Err(err) = result {
            tracing::warn!("failed to write session log: {err}");
        }
    }
}

pub struct Transcript {
    pub meta: SessionMeta,
    /// Messages since the last compaction, ready to continue from.
    pub messages: Vec<Message>,
    /// Every message ever recorded, for display.
    pub history: Vec<Message>,
    pub usage: Usage,
}

pub fn load(id: &str) -> Result<Transcript> {
    let path = session_path(id)?;
    let file = File::open(&path).with_context(|| format!("no session {id}"))?;
    let mut meta = None;
    let mut messages = Vec::new();
    let mut history = Vec::new();
    let mut usage = Usage::default();
    for line in BufReader::new(file).lines() {
        let line = line?;
        let Ok(record) = serde_json::from_str::<Record>(&line) else {
            continue;
        };
        match record {
            Record::Meta(m) => meta = Some(m),
            Record::Message { message } => {
                history.push(message.clone());
                messages.push(message);
            }
            Record::Compaction { summary } => messages = vec![compaction_message(&summary)],
            Record::Usage { usage: u } => usage.add(&u),
        }
    }
    Ok(Transcript {
        meta: meta.with_context(|| format!("session {id} has no metadata"))?,
        messages,
        history,
        usage,
    })
}

pub fn compaction_message(summary: &str) -> Message {
    Message::user_text(format!(
        "This conversation was compacted. Summary of everything before this point:\n\n{summary}\n\n\
         Continue the work from here."
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    #[serde(flatten)]
    pub meta: SessionMeta,
    pub title: String,
    pub updated: DateTime<Utc>,
}

/// Recent sessions, newest first. Only the head of each file is read.
pub fn list(include_subagents: bool, limit: usize) -> Result<Vec<SessionSummary>> {
    let Ok(dir) = std::fs::read_dir(sessions_dir()) else {
        return Ok(Vec::new());
    };
    let mut paths: Vec<(std::time::SystemTime, PathBuf)> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    paths.sort_by(|a, b| b.0.cmp(&a.0));

    let mut out = Vec::new();
    for (modified, path) in paths {
        if out.len() == limit {
            break;
        }
        let Some((meta, title)) = read_head(&path) else {
            continue;
        };
        if meta.parent.is_some() && !include_subagents {
            continue;
        }
        out.push(SessionSummary {
            meta,
            title,
            updated: modified.into(),
        });
    }
    Ok(out)
}

/// One session's summary, read from the head of its file.
pub fn summary(id: &str) -> Option<SessionSummary> {
    let path = session_path(id).ok()?;
    let updated = std::fs::metadata(&path).ok()?.modified().ok()?;
    let (meta, title) = read_head(&path)?;
    Some(SessionSummary {
        meta,
        title,
        updated: updated.into(),
    })
}

fn read_head(path: &PathBuf) -> Option<(SessionMeta, String)> {
    let mut head = String::new();
    File::open(path)
        .ok()?
        .take(256 * 1024)
        .read_to_string(&mut head)
        .ok()?;
    let mut meta = None;
    let mut title = String::new();
    for line in head.lines() {
        match serde_json::from_str::<Record>(line) {
            Ok(Record::Meta(m)) => meta = Some(m),
            Ok(Record::Message { message }) if message.role == Role::User => {
                title = crate::tools::truncate_chars(
                    message.text().lines().next().unwrap_or_default(),
                    80,
                );
                break;
            }
            _ => {}
        }
    }
    Some((meta?, title))
}

/// The most recent top-level session started in `cwd`.
pub fn latest_in(cwd: &std::path::Path) -> Result<Option<SessionSummary>> {
    Ok(list(false, 200)?.into_iter().find(|s| s.meta.cwd == cwd))
}
