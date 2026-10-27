//! Everything an agent does is published on the [`EventBus`]; the terminal,
//! the web UI and session logs are all subscribers.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::message::{Message, Usage};

pub type AgentId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub agent: AgentId,
    pub seq: u64,
    pub at: DateTime<Utc>,
    #[serde(flatten)]
    pub event: Event,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    AgentStarted {
        label: String,
        parent: Option<AgentId>,
        model: String,
        cwd: String,
    },
    ModelChanged {
        model: String,
    },
    UserMessage {
        text: String,
    },
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolCallStreaming {
        tool: String,
    },
    AssistantMessage {
        message: Message,
    },
    ToolStarted {
        call_id: String,
        tool: String,
        summary: String,
        input: Value,
    },
    ToolFinished {
        call_id: String,
        tool: String,
        output: String,
        is_error: bool,
        millis: u64,
    },
    PermissionRequested {
        request_id: String,
        call_id: String,
        tool: String,
        summary: String,
        input: Value,
    },
    PermissionResolved {
        request_id: String,
        allowed: bool,
    },
    Todos {
        items: Vec<Todo>,
    },
    Usage {
        turn: Usage,
        total: Usage,
        context_tokens: u64,
    },
    Compacted {
        summary: String,
    },
    Notice {
        level: Level,
        message: String,
    },
    TurnFinished {
        status: TurnStatus,
        text: String,
    },
    /// A sentence the agent wants read aloud (`say`).
    Say {
        text: String,
    },
    AgentFinished {
        status: TurnStatus,
        report: String,
    },
    /// Published with an empty agent id when a newer malac is available or installed.
    Update {
        status: crate::update::Status,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Arc<Envelope>>,
    seq: Arc<AtomicU64>,
}

impl Default for EventBus {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(8192);
        Self {
            tx,
            seq: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl EventBus {
    pub fn emit(&self, agent: &str, event: Event) {
        let envelope = Envelope {
            agent: agent.to_owned(),
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            at: Utc::now(),
            event,
        };
        let _ = self.tx.send(Arc::new(envelope));
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Envelope>> {
        self.tx.subscribe()
    }

    /// A bus with its own subscribers that shares this one's sequence
    /// numbers, so events from several runtimes can be ordered.
    pub fn sibling(&self) -> Self {
        let (tx, _) = broadcast::channel(8192);
        Self {
            tx,
            seq: self.seq.clone(),
        }
    }
}
