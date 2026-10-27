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
    /// Each reply, with the stop reason to report when it is not the usual one.
    replies: Mutex<VecDeque<(Message, Option<StopReason>)>>,
    requests: Mutex<Vec<Vec<Message>>>,
}

impl Scripted {
    fn new(replies: Vec<Message>) -> Arc<Self> {
        Self::with_stops(replies.into_iter().map(|m| (m, None)).collect())
    }

    fn with_stops(replies: Vec<(Message, Option<StopReason>)>) -> Arc<Self> {
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
        let (message, stop) = self.replies.lock().unwrap().pop_front().unwrap_or_else(|| {
            (
                Message::assistant(vec![Block::text("(script exhausted)")]),
                None,
            )
        });
        let text = message.text();
        if !text.is_empty() {
            on_delta(Delta::Text(&text));
        }
        let stop = stop.unwrap_or(if message.tool_uses().next().is_some() {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        });
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

fn thinking(text: &str) -> Message {
    Message::assistant(vec![Block::Thinking {
        text: text.into(),
        signature: None,
    }])
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
    runtime_with(mode, script, cwd, |_| {})
}

fn runtime_with(
    mode: Mode,
    script: Arc<Scripted>,
    cwd: &std::path::Path,
    configure: impl FnOnce(&mut Config),
) -> Arc<Runtime> {
    isolated_home();
    let mut config = Config {
        model: Some("mock/test".into()),
        mode,
        ..Default::default()
    };
    configure(&mut config);
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

#[tokio::test]
async fn configures_malac_through_its_own_tools() {
    let dir = tempfile::tempdir().unwrap();
    let script = Scripted::new(vec![
        tool_call(
            "c1",
            "malac_configure",
            json!({ "action": "set", "key": "ui.show_thinking", "value": true }),
        ),
        tool_call("c2", "malac_settings", json!({ "show": "config" })),
        answer("Reasoning will now be shown."),
    ]);
    let runtime = runtime(Mode::Autopilot, script.clone(), dir.path());
    let agent = runtime.spawn_agent(AgentSpec::default()).unwrap();

    agent
        .prompt(vec![Block::text("show your reasoning from now on")])
        .await
        .unwrap();

    let saved = std::fs::read_to_string(malac_core::config::user_config_path()).unwrap();
    let table: toml::Table = saved.parse().unwrap();
    assert_eq!(table["ui"]["show_thinking"].as_bool(), Some(true));
    let (shown, _) = tool_result_text(&script.last_request(), "c2").unwrap();
    assert!(shown.contains("show_thinking = true"));
}

#[tokio::test]
async fn read_only_agents_spawn_only_read_only_subagents() {
    let dir = tempfile::tempdir().unwrap();
    let script = Scripted::new(vec![
        tool_call(
            "c1",
            "agent_spawn",
            json!({ "task": "look around", "label": "scout", "read_only": true }),
        ),
        tool_call(
            "c2",
            "agent_spawn",
            json!({ "task": "delete things", "label": "helper" }),
        ),
        tool_call("c3", "bash", json!({ "command": "echo escaped" })),
        answer("helper done"),
        answer("scout done"),
        answer("all done"),
    ]);
    let runtime = runtime(Mode::Autopilot, script.clone(), dir.path());
    let agent = runtime.spawn_agent(AgentSpec::default()).unwrap();

    agent.prompt(vec![Block::text("explore")]).await.unwrap();

    let helper = runtime
        .agents()
        .into_iter()
        .find(|a| a.info.label == "helper")
        .expect("helper spawned");
    assert!(
        helper.info.read_only,
        "a read-only parent must produce read-only children"
    );
    let requests = script.requests.lock().unwrap();
    let helper_turn = requests
        .iter()
        .find(|r| tool_result_text(r, "c3").is_some())
        .unwrap();
    let (result, is_error) = tool_result_text(helper_turn, "c3").unwrap();
    assert!(is_error);
    assert!(result.contains("unknown tool"));
}

#[tokio::test]
async fn continues_responses_cut_off_at_the_output_limit() {
    let dir = tempfile::tempdir().unwrap();
    let script = Scripted::with_stops(vec![
        (
            thinking("plan: write index.html first"),
            Some(StopReason::MaxTokens),
        ),
        (
            tool_call(
                "c1",
                "write",
                json!({ "path": "index.html", "content": "<h1>Plants</h1>\n" }),
            ),
            None,
        ),
        (
            answer("Wrote index.html. The first half"),
            Some(StopReason::MaxTokens),
        ),
        (answer(" and the second half."), None),
    ]);
    let runtime = runtime(Mode::Autopilot, script.clone(), dir.path());
    let agent = runtime.spawn_agent(AgentSpec::default()).unwrap();

    let reply = agent
        .prompt(vec![Block::text("make a page")])
        .await
        .unwrap();

    assert_eq!(
        reply,
        "Wrote index.html. The first half and the second half."
    );
    assert!(dir.path().join("index.html").exists());
    let requests = script.requests.lock().unwrap();
    // Unfinished thinking cannot be replayed, so it comes back as notes,
    // followed by a plain request to act.
    let after_thinking = &requests[1];
    let notes = &after_thinking[after_thinking.len() - 2];
    assert_eq!(notes.role, Role::Assistant);
    assert!(notes.text().contains("plan: write index.html first"));
    assert!(
        after_thinking
            .last()
            .unwrap()
            .text()
            .contains("Stop deliberating")
    );
    assert!(
        requests[3]
            .last()
            .unwrap()
            .text()
            .contains("Continue exactly where it stopped")
    );
}

#[tokio::test]
async fn gives_up_after_a_few_cut_offs_in_a_row() {
    let dir = tempfile::tempdir().unwrap();
    let cut = |text: &str| (thinking(text), Some(StopReason::MaxTokens));
    let script = Scripted::with_stops(vec![
        cut("one"),
        cut("two"),
        cut("three"),
        cut("four"),
        (answer("never asked for"), None),
    ]);
    let runtime = runtime(Mode::Autopilot, script.clone(), dir.path());
    let agent = runtime.spawn_agent(AgentSpec::default()).unwrap();

    let reply = agent.prompt(vec![Block::text("think")]).await.unwrap();

    assert_eq!(reply, "");
    assert_eq!(script.requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn reports_after_the_step_limit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "the answer is 42\n").unwrap();
    let read = |id: &str| tool_call(id, "read", json!({ "path": "notes.txt" }));
    let script = Scripted::new(vec![
        read("c1"),
        read("c2"),
        answer("Report: I read notes.txt once; the second read was cut."),
    ]);
    let runtime = runtime_with(Mode::Autopilot, script.clone(), dir.path(), |config| {
        config.max_steps = Some(1)
    });
    let agent = runtime.spawn_agent(AgentSpec::default()).unwrap();

    let reply = agent
        .prompt(vec![Block::text("read it twice")])
        .await
        .unwrap();

    assert_eq!(
        reply,
        "Report: I read notes.txt once; the second read was cut."
    );
    let last = script.last_request();
    let (text, is_error) = tool_result_text(&last, "c2").unwrap();
    assert!(is_error && text.contains("step limit"));
    assert!(
        last.last()
            .unwrap()
            .text()
            .contains("Write your final report")
    );
}

#[tokio::test]
async fn tool_calls_in_the_report_turn_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "the answer is 42\n").unwrap();
    let read = |id: &str| tool_call(id, "read", json!({ "path": "notes.txt" }));
    let script = Scripted::new(vec![
        read("c1"),
        read("c2"),
        read("c3"),
        answer("Still here."),
    ]);
    let runtime = runtime_with(Mode::Autopilot, script.clone(), dir.path(), |config| {
        config.max_steps = Some(1)
    });
    let agent = runtime.spawn_agent(AgentSpec::default()).unwrap();

    let reply = agent.prompt(vec![Block::text("read it")]).await.unwrap();
    assert_eq!(reply, "");

    // The refused call has its result, so the next request is valid.
    let next = agent
        .prompt(vec![Block::text("anything else?")])
        .await
        .unwrap();
    assert_eq!(next, "Still here.");
    let (text, is_error) = tool_result_text(&script.last_request(), "c3").unwrap();
    assert!(is_error && text.contains("tools are off"));
}
