//! Drives real agents against a scripted provider.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Once};

use anyhow::Result;
use async_trait::async_trait;
use malac_core::config::{Config, ProviderConfig};
use malac_core::message::{Block, Message, Role, StopReason, Usage};
use malac_core::permission::Mode;
use malac_core::provider::{Completion, Delta, DeltaSink, Provider, Request};
use malac_core::{AgentSpec, Runtime};
use serde_json::{Value, json};

/// Replies with queued messages and records every request it receives.
#[derive(Default)]
struct Scripted {
    replies: Mutex<VecDeque<Message>>,
    requests: Mutex<Vec<Vec<Message>>>,
}

impl Scripted {
    fn new(replies: Vec<Message>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            requests: Mutex::default(),
        })
    }

    fn last_request(&self) -> Vec<Message> {
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

fn tool_call(id: &str, name: &str, input: Value) -> Message {
    Message::assistant(vec![Block::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
    }])
}

fn answer(text: &str) -> Message {
    Message::assistant(vec![Block::text(text)])
}

fn tool_result_text(messages: &[Message], id: &str) -> Option<(String, bool)> {
    messages
        .iter()
        .filter(|m| m.role == Role::User)
        .find_map(|m| {
            m.content.iter().find_map(|b| match b {
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } if tool_use_id == id => Some((
                    content
                        .iter()
                        .filter_map(|c| match c {
                            Block::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect(),
                    *is_error,
                )),
                _ => None,
            })
        })
}

fn isolated_home() {
    static HOME: Once = Once::new();
    HOME.call_once(|| {
        let dir = tempfile::tempdir().unwrap().keep();
        unsafe { std::env::set_var("MALAC_HOME", dir) };
    });
}

fn runtime(mode: Mode, script: Arc<Scripted>, cwd: &std::path::Path) -> Arc<Runtime> {
    isolated_home();
    let mut config = Config {
        model: Some("mock/test".into()),
        mode,
        ..Default::default()
    };
    config
        .providers
        .insert("mock".into(), ProviderConfig::default());
    config.control.enabled = false;
    let runtime = Runtime::new(config, cwd.to_path_buf()).unwrap();
    runtime.insert_provider("mock", script);
    runtime
}

#[tokio::test]
async fn runs_tools_and_feeds_results_back() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "the answer is 42\n").unwrap();
    let script = Scripted::new(vec![
        tool_call("c1", "read", json!({ "path": "notes.txt" })),
        answer("It says 42."),
    ]);
    let runtime = runtime(Mode::Ask, script.clone(), dir.path());
    let agent = runtime.spawn_agent(AgentSpec::default()).unwrap();

    let reply = agent
        .prompt(vec![Block::text("what does notes.txt say?")])
        .await
        .unwrap();

    assert_eq!(reply, "It says 42.");
    let (text, is_error) = tool_result_text(&script.last_request(), "c1").unwrap();
    assert!(!is_error);
    assert!(text.contains("the answer is 42"));
}

#[tokio::test]
async fn denied_commands_report_back_to_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let script = Scripted::new(vec![
        tool_call("c1", "bash", json!({ "command": "touch created.txt" })),
        answer("Okay, I will not run it."),
    ]);
    let runtime = runtime(Mode::Ask, script.clone(), dir.path());
    let agent = runtime.spawn_agent(AgentSpec::default()).unwrap();

    agent
        .prompt(vec![Block::text("create a file")])
        .await
        .unwrap();

    let (text, is_error) = tool_result_text(&script.last_request(), "c1").unwrap();
    assert!(is_error);
    assert!(text.contains("denied"));
    assert!(!dir.path().join("created.txt").exists());
}

#[tokio::test]
async fn autopilot_runs_commands_and_edits() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello world\n").unwrap();
    let script = Scripted::new(vec![
        tool_call(
            "c1",
            "edit",
            json!({ "path": "a.txt", "old_string": "world", "new_string": "malac" }),
        ),
        tool_call("c2", "read", json!({ "path": "a.txt" })),
        tool_call(
            "c3",
            "edit",
            json!({ "path": "a.txt", "old_string": "hello", "new_string": "hi" }),
        ),
        tool_call("c4", "bash", json!({ "command": "cat a.txt" })),
        answer("done"),
    ]);
    let runtime = runtime(Mode::Autopilot, script.clone(), dir.path());
    let agent = runtime.spawn_agent(AgentSpec::default()).unwrap();

    agent.prompt(vec![Block::text("edit it")]).await.unwrap();

    let history = script.last_request();
    let (unread, is_error) = tool_result_text(&history, "c1").unwrap();
    assert!(is_error, "editing an unread file must fail");
    assert!(unread.contains("read"));
    let (cat, _) = tool_result_text(&history, "c4").unwrap();
    assert_eq!(cat.trim(), "hi world");
}

#[tokio::test]
async fn delegates_to_a_subagent() {
    let dir = tempfile::tempdir().unwrap();
    let script = Scripted::new(vec![
        tool_call(
            "c1",
            "agent_spawn",
            json!({ "task": "count to three", "label": "counter" }),
        ),
        answer("one, two, three"),
        answer("The subagent counted to three."),
    ]);
    let runtime = runtime(Mode::Ask, script.clone(), dir.path());
    let agent = runtime.spawn_agent(AgentSpec::default()).unwrap();

    let reply = agent
        .prompt(vec![Block::text("delegate counting")])
        .await
        .unwrap();

    assert_eq!(reply, "The subagent counted to three.");
    let children = runtime.children(&agent.id);
    assert_eq!(children.len(), 1);
    let child = children[0].snapshot();
    assert_eq!(child.info.label, "counter");
    assert_eq!(child.report.as_deref(), Some("one, two, three"));
    let (report, _) = tool_result_text(&script.last_request(), "c1").unwrap();
    assert!(report.contains("one, two, three"));
}
