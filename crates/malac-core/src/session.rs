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
    Message {
        message: Message,
    },
    Compaction {
        summary: String,
    },
    Usage {
        usage: Usage,
    },
    /// An approval the user was asked for, and their answer.
    Decision(DecisionRecord),
}

/// A tool call that waited for approval: what was asked, and the answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub call_id: String,
    pub tool: String,
    pub summary: String,
    pub decision: crate::permission::Decision,
    /// Where it was answered, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    pub at: DateTime<Utc>,
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
        let log = Self { file };
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
    pub fn append(&self, record: &Record) {
        let result = serde_json::to_string(record)
            .map_err(anyhow::Error::from)
            .and_then(|line| Ok(writeln!(&self.file, "{line}")?));
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
    /// The approvals asked for, in order.
    pub decisions: Vec<DecisionRecord>,
}

pub fn load(id: &str) -> Result<Transcript> {
    let path = session_path(id)?;
    let file = File::open(&path).with_context(|| format!("no session {id}"))?;
    let mut meta = None;
    let mut messages = Vec::new();
    let mut history = Vec::new();
    let mut usage = Usage::default();
    let mut decisions = Vec::new();
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
            Record::Decision(decision) => decisions.push(decision),
        }
    }
    Ok(Transcript {
        meta: meta.with_context(|| format!("session {id} has no metadata"))?,
        messages,
        history,
        usage,
        decisions,
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
                let typed = crate::attach::typed_text(&message.content);
                title = match typed.lines().find(|l| !l.trim().is_empty()) {
                    Some(line) => crate::tools::truncate_chars(line.trim(), 80),
                    None => {
                        let attached: Vec<String> = message
                            .content
                            .iter()
                            .filter_map(|b| match b {
                                crate::message::Block::Text { text } => {
                                    crate::attach::attached_path(text)
                                }
                                _ => None,
                            })
                            .collect();
                        crate::tools::truncate_chars(&attached.join(", "), 80)
                    }
                };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Block;

    fn head_of(messages: Vec<Message>) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let meta = SessionMeta {
            id: "s".into(),
            label: "main".into(),
            parent: None,
            cwd: dir.path().to_path_buf(),
            model: "m/m".into(),
            created: Utc::now(),
        };
        let mut lines = vec![serde_json::to_string(&Record::Meta(meta)).unwrap()];
        for message in messages {
            lines.push(serde_json::to_string(&Record::Message { message }).unwrap());
        }
        std::fs::write(&path, lines.join("\n")).unwrap();
        read_head(&path).unwrap().1
    }

    #[test]
    fn titles_come_from_typed_text() {
        let attached = Block::text(
            "<attachment path=\"notes.txt\" size=\"5 B\">\n```txt\nhello\n```\n</attachment>",
        );
        let recalled = Block::text("<recalled-memories>\nold facts\n</recalled-memories>");
        let title = head_of(vec![Message::user(vec![
            recalled,
            attached.clone(),
            Block::text("\n  summarize the notes\nplease"),
        ])]);
        assert_eq!(title, "summarize the notes");
        let title = head_of(vec![Message::user(vec![attached])]);
        assert_eq!(title, "notes.txt");
    }
}
