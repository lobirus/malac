//! Shared pieces of the daemon tests: an isolated malac home, a scripted
//! provider and helpers to wait for messages.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use malac::client::Events;
use malac::daemon::{Daemon, ServeArgs};
use malac::protocol::ServerMessage;
use malac_core::message::{Block, Message, StopReason, Usage};
use malac_core::provider::{Completion, Delta, DeltaSink, Provider, Request};
use serde_json::Value;

/// A malac home for this test binary, with a mock model and no control
/// service, whose pickers browse `browse/`. Each test binary runs in its
/// own process.
pub fn home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap().keep();
        let browse = dir.join("browse");
        std::fs::create_dir_all(browse.join("app/src")).unwrap();
        std::fs::write(browse.join("app/Cargo.toml"), "[package]\n").unwrap();
        std::fs::write(browse.join("app/notes.txt"), "hello\n").unwrap();
        std::fs::write(
            dir.join("config.toml"),
            format!(
                r#"
model = "mock/test"
mode = "ask"

[providers.mock]
kind = "openai"
base_url = "http://127.0.0.1:9/v1"
models = ["test", "fast"]

[models."mock/fast"]
description = "fast and cheap"

[control]
enabled = false

[update]
check = false

[memory]
enabled = false

[daemon]
idle_minutes = 0
browse_root = "{}"
"#,
                browse.display()
            ),
        )
        .unwrap();
        unsafe { std::env::set_var("MALAC_HOME", &dir) };
        dir
    })
}

/// Runs one test at a time in this binary, on a home without workspaces,
/// so tests do not see each other's.
pub async fn exclusive() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let guard = LOCK.lock().await;
    let home = home();
    let _ = std::fs::remove_dir_all(home.join("workspaces"));
    let _ = std::fs::remove_file(home.join("state.json"));
    guard
}

/// A project directory outside the malac home.
pub fn project(files: &[(&str, &str)]) -> PathBuf {
    let dir = tempfile::tempdir().unwrap().keep();
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    dir.canonicalize().unwrap()
}

/// A daemon on a free port. Only one test per binary may `publish` it
/// (take the lock and write `daemon.json`) at a time.
pub async fn start_daemon(script: Arc<Scripted>, publish: bool) -> Daemon {
    home();
    let args = ServeArgs {
        port: Some(0),
        host: Some("127.0.0.1".into()),
        ..Default::default()
    };
    let daemon = Daemon::start(&args, publish).await.unwrap();
    daemon.hub().insert_provider("mock", script);
    daemon
}

/// Replies with queued messages and records every request it receives.
#[derive(Default)]
pub struct Scripted {
    replies: Mutex<VecDeque<Message>>,
    pub requests: Mutex<Vec<Vec<Message>>>,
    pub systems: Mutex<Vec<String>>,
}

impl Scripted {
    pub fn new(replies: Vec<Message>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            ..Default::default()
        })
    }

    pub fn push(&self, reply: Message) {
        self.replies.lock().unwrap().push_back(reply);
    }

    pub fn last_request(&self) -> Vec<Message> {
        self.requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait]
impl Provider for Scripted {
    async fn complete(&self, req: &Request<'_>, on_delta: DeltaSink<'_>) -> Result<Completion> {
        self.requests.lock().unwrap().push(req.messages.to_vec());
        self.systems.lock().unwrap().push(req.system.to_owned());
        let message = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Message::assistant(vec![Block::text("(script exhausted)")]));
        let text = message.text();
        if !text.is_empty() {
            on_delta(Delta::Text(&text));
        }
        let stop = if message.tool_uses().next().is_some() {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        Ok(Completion {
            message,
            usage: Usage::default(),
            stop,
        })
    }
}

pub fn tool_call(id: &str, name: &str, input: Value) -> Message {
    Message::assistant(vec![Block::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
    }])
}

pub fn answer(text: &str) -> Message {
    Message::assistant(vec![Block::text(text)])
}

/// All text in a request, tool results included.
pub fn request_text(messages: &[Message]) -> String {
    let mut out = String::new();
    for message in messages {
        for block in &message.content {
            match block {
                Block::Text { text } => out.push_str(text),
                Block::ToolResult { content, .. } => {
                    for c in content {
                        if let Block::Text { text } = c {
                            out.push_str(text);
                        }
                    }
                }
                _ => {}
            }
            out.push('\n');
        }
    }
    out
}

/// The first message matching `pick`, skipping others; fails after 10 s.
pub async fn wait_for<T>(
    events: &mut Events,
    mut pick: impl FnMut(&ServerMessage) -> Option<T>,
) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let message = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("timed out waiting for a message")
            .expect("the daemon closed the connection");
        if let Some(found) = pick(&message) {
            return found;
        }
    }
}
