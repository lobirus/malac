//! Workspaces, tabs, agents and jobs survive a daemon restart.

mod common;

use common::*;
use malac::client::Client;
use malac::protocol::{Command, Launch, ServerMessage, TabOptions, TabSpec, WorkspaceChoice};
use malac_core::event::Event;
use malac_core::jobs::JobStatus;
use malac_core::permission::Mode;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_restores_workspaces_lazily() {
    let other = project(&[]);
    let project = project(&[("README.md", "# demo\n")]);

    // First daemon: a workspace from `malac -n -y <prompt>`, a second tab
    // and a background job that outlives the daemon.
    let script = Scripted::new(vec![
        tool_call(
            "c1",
            "bash",
            json!({ "command": "echo from-the-job; sleep 30", "background": true }),
        ),
        answer("started the server"),
    ]);
    let daemon = start_daemon(script.clone(), true).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let (client, mut events, _) = Client::connect(&file).await.unwrap();
    let launched = client
        .launch(Launch {
            workspace: WorkspaceChoice::New,
            cwd: project.clone(),
            name: Some("demo".into()),
            prompt: Some("start the dev server".into()),
            options: TabOptions {
                mode: Some(Mode::Autopilot),
                ..Default::default()
            },
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(launched.workspace.name, "demo");
    let tab = launched.tab.clone().unwrap();
    let agent = launched.agent.clone().unwrap();
    let tab_name = &launched.workspace.tab(&tab).unwrap().name;
    assert!(tab_name.ends_with(": start the dev server"), "{tab_name}");
    // The job and the agent's events travel separately, so the turn can end
    // before the job's broadcast arrives: wait for both, in either order.
    let (mut started, mut finished) = (None, false);
    wait_for(&mut events, |m| {
        match m {
            ServerMessage::JobStarted { job } => started = Some(job.clone()),
            ServerMessage::Event { envelope, .. } if envelope.agent == agent => {
                finished |= matches!(envelope.event, Event::TurnFinished { .. });
            }
            _ => {}
        }
        (started.is_some() && finished).then_some(())
    })
    .await;
    let job = started.unwrap();
    assert_eq!(job.tab.as_deref(), Some(tab.as_str()));
    assert_eq!(job.job.agent.as_deref(), Some(agent.as_str()));
    let reply = client
        .request(Command::TabCreate(TabSpec {
            cwd: other.clone(),
            ..Default::default()
        }))
        .await
        .unwrap();
    let ServerMessage::Ack {
        tab: Some(second), ..
    } = reply
    else {
        panic!("ack expected");
    };
    client
        .request(Command::TabMove {
            tab: second.clone(),
            index: 0,
        })
        .await
        .unwrap();
    client
        .request(Command::TabActivate { tab: tab.clone() })
        .await
        .unwrap();
    let output = client
        .request(Command::JobOutput {
            job: job.job.id.clone(),
            tail: Some(10),
        })
        .await
        .unwrap();
    let ServerMessage::JobOutput { output, .. } = output else {
        panic!("job output expected");
    };
    assert!(output.job.is_running());
    drop(client);
    hub.request_stop("restart");
    server.await.unwrap().unwrap();

    // Second daemon: the same home, a fresh process state.
    let script = Scripted::new(vec![answer("resumed")]);
    let daemon = start_daemon(script.clone(), true).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let (client, mut events, snapshot) = Client::connect(&file).await.unwrap();

    let workspace = snapshot.workspace.expect("the last workspace is restored");
    assert_eq!(workspace.name, "demo");
    let order: Vec<&str> = workspace.tabs.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(order, [second.as_str(), tab.as_str()], "tab order is kept");
    assert_eq!(workspace.active_tab.as_deref(), Some(tab.as_str()));
    let restored = workspace.tab(&tab).unwrap();
    assert!(!restored.loaded, "tabs load lazily");
    assert_eq!(restored.mode, Mode::Autopilot);
    assert_eq!(restored.agents, [agent.clone()]);
    let saved = &restored.snapshots[0];
    assert!(!saved.live, "agents come back idle, from their transcript");
    assert_eq!(saved.title, "start the dev server");

    let restored_job = snapshot
        .jobs
        .iter()
        .find(|j| j.job.id == job.job.id)
        .expect("the job is restored");
    assert_eq!(
        restored_job.job.status,
        JobStatus::Running,
        "its process still runs"
    );

    // Prompting the saved agent resumes it with its history.
    client
        .request(Command::Prompt {
            tab: None,
            agent: Some(agent.clone()),
            text: "are you still there?".into(),
            images: Vec::new(),
            attachments: Vec::new(),
            model: None,
        })
        .await
        .unwrap();
    let text = wait_for(&mut events, |m| match m {
        ServerMessage::Event { envelope, .. } if envelope.agent == agent => match &envelope.event {
            Event::TurnFinished { text, .. } => Some(text.clone()),
            _ => None,
        },
        _ => None,
    })
    .await;
    assert_eq!(text, "resumed");
    let history = request_text(&script.last_request());
    assert!(history.contains("start the dev server"));
    assert!(history.contains("are you still there?"));

    // The job started by the first daemon can be stopped from the second.
    client
        .request(Command::JobKill {
            job: job.job.id.clone(),
        })
        .await
        .unwrap();
    let finished = wait_for(&mut events, |m| match m {
        ServerMessage::JobFinished { job: done } if done.job.id == job.job.id => Some(done.clone()),
        _ => None,
    })
    .await;
    assert_eq!(finished.job.status, JobStatus::Killed);

    // `malac`, `malac -w demo` and `malac -c` in the project land here too.
    let last = client
        .launch(Launch {
            cwd: project.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(last.workspace.id, workspace.id);
    assert_eq!(last.tab.as_deref(), Some(tab.as_str()));
    let named = client
        .launch(Launch {
            workspace: WorkspaceChoice::Open("demo".into()),
            cwd: other.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(named.workspace.id, workspace.id);
    let continued = client
        .launch(Launch {
            cwd: project.clone(),
            continue_last: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(continued.agent.as_deref(), Some(agent.as_str()));
    assert_eq!(continued.tab.as_deref(), Some(tab.as_str()));

    // `-n` again starts a clean workspace; the old one stays.
    let fresh = client
        .launch(Launch {
            workspace: WorkspaceChoice::New,
            cwd: project.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_ne!(fresh.workspace.id, workspace.id);
    assert_eq!(fresh.workspace.tabs.len(), 1);
    let listed = client.request(Command::WorkspaceList).await.unwrap();
    let ServerMessage::Workspaces { workspaces, .. } = listed else {
        panic!("workspaces expected");
    };
    assert_eq!(workspaces.len(), 2);
    let refused = client
        .request(Command::WorkspaceDelete {
            workspace: "demo".into(),
            force: false,
        })
        .await;
    assert!(
        refused.is_ok(),
        "an idle workspace can be deleted: {refused:?}"
    );

    drop(client);
    hub.request_stop("done");
    server.await.unwrap().unwrap();
}
