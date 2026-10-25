//! Background jobs: started by an agent, followed, stopped, and restored by
//! a later process.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use malac_core::config::{Config, ProviderConfig};
use malac_core::jobs::{JobEvent, JobInfo, JobStatus, Jobs};
use malac_core::message::{Block, Message, Role, StopReason, Usage};
use malac_core::permission::Mode;
use malac_core::provider::{Completion, DeltaSink, Provider, Request};
use malac_core::{AgentSpec, Runtime};
use serde_json::{Value, json};

#[derive(Default)]
struct Scripted {
    replies: Mutex<VecDeque<Message>>,
    requests: Mutex<Vec<Vec<Message>>>,
}

#[async_trait]
impl Provider for Scripted {
    async fn complete(&self, req: &Request<'_>, _: DeltaSink<'_>) -> Result<Completion> {
        self.requests.lock().unwrap().push(req.messages.to_vec());
        let message = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Message::assistant(vec![Block::text("done")]));
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

fn call(id: &str, name: &str, input: Value) -> Message {
    Message::assistant(vec![Block::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
    }])
}

fn result_text(messages: &[Message], id: &str) -> String {
    messages
        .iter()
        .filter(|m| m.role == Role::User)
        .flat_map(|m| &m.content)
        .find_map(|b| match b {
            Block::ToolResult {
                tool_use_id,
                content,
                ..
            } if tool_use_id == id => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        Block::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// A job id from the bash tool's reply: "Started job <id> in the background".
fn job_id(text: &str) -> String {
    text.split_whitespace().nth(2).unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn agents_start_follow_and_stop_jobs() {
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MALAC_HOME", home.path()) };
    let dir = tempfile::tempdir().unwrap();
    let script = Arc::new(Scripted::default());
    script.replies.lock().unwrap().extend([
        call(
            "c1",
            "bash",
            json!({ "command": "echo ready; sleep 30", "background": true }),
        ),
        Message::assistant(vec![Block::text("started")]),
    ]);
    let mut config = Config {
        model: Some("mock/test".into()),
        mode: Mode::Autopilot,
        ..Default::default()
    };
    config
        .providers
        .insert("mock".into(), ProviderConfig::default());
    config.control.enabled = false;
    let runtime = Runtime::new(config, dir.path().to_path_buf()).unwrap();
    runtime.insert_provider("mock", script.clone());
    let mut events = runtime.jobs.subscribe();
    let agent = runtime.spawn_agent(AgentSpec::default()).unwrap();

    agent.prompt(vec![Block::text("start it")]).await.unwrap();
    let started = result_text(&script.requests.lock().unwrap()[1], "c1");
    assert!(started.contains("(pid "), "{started}");
    assert!(started.contains("Output is written to "), "{started}");
    let id = job_id(&started);
    let JobEvent::Started { job } = events.recv().await.unwrap() else {
        panic!("job_started expected");
    };
    assert_eq!(job.id, id);
    assert_eq!(job.agent.as_deref(), Some(agent.id.as_str()));
    assert!(job.log.starts_with(home.path()));

    // Give the shell a moment to write its first line.
    for _ in 0..50 {
        if runtime.jobs.output(&id, 10).unwrap().text.contains("ready") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    script.replies.lock().unwrap().extend([
        call("c2", "job_list", json!({})),
        call("c3", "job_output", json!({ "id": id, "tail": 5 })),
        call("c4", "job_kill", json!({ "id": id })),
        Message::assistant(vec![Block::text("stopped")]),
    ]);
    agent
        .prompt(vec![Block::text("check and stop it")])
        .await
        .unwrap();
    let last = script.requests.lock().unwrap().last().cloned().unwrap();
    assert!(result_text(&last, "c2").contains(&format!("- {id} running")));
    let output = result_text(&last, "c3");
    assert!(
        output.contains("running") && output.contains("ready"),
        "{output}"
    );
    assert!(result_text(&last, "c4").contains(&format!("Stopping job {id}")));

    let JobEvent::Finished { job } = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("job_finished expected");
    };
    assert_eq!(job.status, JobStatus::Killed);
    assert!(job.finished.is_some());
    let saved: JobInfo = serde_json::from_str(
        &std::fs::read_to_string(home.path().join(format!("jobs/{id}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(
        saved.status,
        JobStatus::Killed,
        "the record on disk is updated"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_later_process_restores_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let record = |id: &str, pid: u32| JobInfo {
        id: id.into(),
        command: "sleep 30".into(),
        cwd: dir.path().to_path_buf(),
        pid: Some(pid),
        log: dir.path().join(format!("{id}.log")),
        started: Utc::now(),
        finished: None,
        exit_code: None,
        status: JobStatus::Running,
        agent: Some("a1".into()),
    };
    let write = |job: &JobInfo| {
        std::fs::write(
            dir.path().join(format!("{}.json", job.id)),
            serde_json::to_vec(job).unwrap(),
        )
        .unwrap();
        std::fs::write(&job.log, "line one\nline two\n").unwrap();
    };

    // A job whose process ended while nobody watched.
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let gone_pid = gone.id();
    gone.wait().unwrap();
    write(&record("gone0001", gone_pid));

    // A job that still runs in its own process group.
    #[cfg(unix)]
    let alive = {
        use std::os::unix::process::CommandExt;
        std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap()
    };
    #[cfg(unix)]
    write(&record("alive001", alive.id()));

    // An old finished job is forgotten.
    let mut old = record("old00001", 1);
    old.status = JobStatus::Exited;
    old.exit_code = Some(0);
    old.finished = Some(Utc::now() - chrono::TimeDelta::days(30));
    write(&old);

    let jobs = Jobs::new(dir.path().to_path_buf());
    let mut events = jobs.subscribe();
    jobs.restore().unwrap();

    let gone = jobs.get("gone0001").unwrap();
    assert_eq!(gone.status, JobStatus::Unknown);
    assert!(gone.finished.is_some());
    assert!(jobs.get("old00001").is_none());
    assert!(!dir.path().join("old00001.json").exists());
    assert_eq!(jobs.output("gone0001", 1).unwrap().text, "line two");

    #[cfg(unix)]
    {
        let mut alive = alive;
        assert!(jobs.get("alive001").unwrap().is_running());
        assert_eq!(jobs.running(), 1);
        jobs.kill("alive001").unwrap();
        alive.wait().unwrap();
        let JobEvent::Finished { job } =
            tokio::time::timeout(Duration::from_secs(10), events.recv())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("job_finished expected");
        };
        assert_eq!(job.id, "alive001");
        assert_eq!(job.status, JobStatus::Killed);
        assert!(
            jobs.kill("alive001").is_err(),
            "a finished job cannot be killed"
        );
    }
}
