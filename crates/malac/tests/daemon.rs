//! Two clients drive one in-process daemon over its WebSocket protocol.

mod common;

use common::*;
use malac::client::{Client, DaemonFile};
use malac::protocol::{AttachmentInput, Command, ServerMessage, TabSpec};
use malac_core::attach::AttachmentKind;
use malac_core::event::{Event, TurnStatus};
use malac_core::permission::Decision;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_share_tabs_agents_and_approvals() {
    let script = Scripted::new(vec![
        tool_call("c1", "bash", json!({ "command": "echo approved-run" })),
        answer("ran it"),
    ]);
    let daemon = start_daemon(script.clone(), true).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    assert_eq!(
        DaemonFile::read().as_ref(),
        Some(&file),
        "daemon.json is published"
    );

    let (a, mut a_events, snapshot) = Client::connect(&file).await.unwrap();
    let (b, mut b_events, _) = Client::connect(&file).await.unwrap();
    assert_eq!(snapshot.daemon.version, env!("CARGO_PKG_VERSION"));
    assert!(!snapshot.daemon.version_mismatch);
    assert!(snapshot.workspace.is_none(), "nothing exists yet");
    assert_eq!(snapshot.models, ["mock/test", "mock/fast"]);

    let opened = a
        .request(Command::WorkspaceCreate {
            name: Some("work".into()),
        })
        .await
        .unwrap();
    let ServerMessage::WorkspaceOpened { workspace, .. } = opened else {
        panic!("workspace_opened expected, got {opened:?}");
    };
    assert_eq!(workspace.name, "work");

    let project = project(&[("notes.txt", "the secret word is pancake\n")]);
    let reply = a
        .request(Command::TabCreate(TabSpec {
            cwd: project.clone(),
            name: Some("proj".into()),
            ..Default::default()
        }))
        .await
        .unwrap();
    let ServerMessage::Ack { tab: Some(tab), .. } = reply else {
        panic!("ack with a tab expected, got {reply:?}");
    };
    let created = wait_for(&mut b_events, |m| match m {
        ServerMessage::TabCreated { tab, .. } => Some(tab.clone()),
        _ => None,
    })
    .await;
    assert_eq!(created.id, tab);
    assert_eq!(created.cwd, project);
    assert!(!created.loaded, "tabs load on first use");

    let reply = a
        .request(Command::Prompt {
            tab: Some(tab.clone()),
            agent: None,
            text: "run it".into(),
            images: Vec::new(),
            attachments: vec![AttachmentInput {
                path: "notes.txt".into(),
            }],
            model: None,
        })
        .await
        .unwrap();
    let ServerMessage::Ack {
        agent: Some(agent),
        attachments,
        ..
    } = reply
    else {
        panic!("ack with an agent expected, got {reply:?}");
    };
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].kind, AttachmentKind::Text);

    // Both clients see the question, tagged with the tab.
    let (request_id, event_tab) = wait_for(&mut a_events, |m| match m {
        ServerMessage::Event { tab, envelope, .. } => match &envelope.event {
            Event::PermissionRequested { request_id, .. } => {
                Some((request_id.clone(), tab.clone()))
            }
            _ => None,
        },
        _ => None,
    })
    .await;
    assert_eq!(event_tab.as_deref(), Some(tab.as_str()));
    wait_for(&mut b_events, |m| match m {
        ServerMessage::Event { envelope, .. } => {
            matches!(envelope.event, Event::PermissionRequested { .. }).then_some(())
        }
        _ => None,
    })
    .await;

    // A client connecting now gets it in its snapshot.
    let (_late, _, snapshot) = Client::connect(&file).await.unwrap();
    assert_eq!(snapshot.pending.len(), 1);
    assert_eq!(snapshot.pending[0].request.id, request_id);
    assert_eq!(snapshot.pending[0].tab.as_deref(), Some(tab.as_str()));
    let view = snapshot.workspace.expect("the last workspace");
    assert_eq!(view.name, "work");
    let tab_view = view.tab(&tab).unwrap();
    assert!(tab_view.loaded);
    assert_eq!(tab_view.agents, [agent.clone()]);

    // The first answer wins; the second is refused.
    b.request(Command::Approve {
        request_id: request_id.clone(),
        decision: Decision::Allow,
    })
    .await
    .unwrap();
    let late = a
        .request(Command::Approve {
            request_id,
            decision: Decision::Deny,
        })
        .await;
    assert!(late.unwrap_err().to_string().contains("already answered"));

    let (status, text) = wait_for(&mut a_events, |m| match m {
        ServerMessage::Event { envelope, .. } if envelope.agent == agent => match &envelope.event {
            Event::TurnFinished { status, text } => Some((*status, text.clone())),
            _ => None,
        },
        _ => None,
    })
    .await;
    assert_eq!(status, TurnStatus::Completed);
    assert_eq!(text, "ran it");
    let requests = script.requests.lock().unwrap().clone();
    let first = request_text(&requests[0]);
    assert!(
        first.contains("pancake"),
        "the attachment reaches the model"
    );
    assert!(
        request_text(&requests[1]).contains("approved-run"),
        "the approved command ran"
    );
    let system = script.systems.lock().unwrap()[0].clone();
    assert!(system.contains("# Available models"));
    assert!(system.contains("- mock/fast: fast and cheap"));
    assert!(system.contains("- mock/test (yours)"));

    // Tab changes made by one client reach the other.
    b.request(Command::TabRename {
        tab: tab.clone(),
        name: "renamed".into(),
    })
    .await
    .unwrap();
    let renamed = wait_for(&mut a_events, |m| match m {
        ServerMessage::TabUpdated { tab, .. } if tab.name == "renamed" => Some(tab.clone()),
        _ => None,
    })
    .await;
    assert_eq!(renamed.active_agent.as_deref(), Some(agent.as_str()));

    let second = a
        .request(Command::AgentStart {
            tab: tab.clone(),
            model: Some("mock/fast".into()),
            label: Some("helper".into()),
        })
        .await
        .unwrap();
    let ServerMessage::Ack {
        agent: Some(helper),
        ..
    } = second
    else {
        panic!("ack expected");
    };
    let updated = wait_for(&mut b_events, |m| match m {
        ServerMessage::TabUpdated { tab, .. } if tab.agents.len() == 2 => Some(tab.clone()),
        _ => None,
    })
    .await;
    assert_eq!(updated.agents, [agent.clone(), helper.clone()]);
    let helper_view = updated
        .snapshots
        .iter()
        .find(|s| s.snapshot.info.id == helper)
        .unwrap();
    assert_eq!(helper_view.snapshot.model, "mock/fast");

    let busy_close = b
        .request(Command::TabClose {
            tab: tab.clone(),
            force: false,
        })
        .await;
    assert!(busy_close.is_ok(), "idle tabs close: {busy_close:?}");
    wait_for(&mut a_events, |m| match m {
        ServerMessage::TabClosed { tab: closed, .. } if *closed == tab => Some(()),
        _ => None,
    })
    .await;

    let jobs = a.request(Command::JobList).await.unwrap();
    assert!(matches!(jobs, ServerMessage::Jobs { .. }));
    let info = a.info().await.unwrap();
    assert_eq!(info.status.clients, 3);
    assert!(!info.busy());

    hub.request_stop("test over");
    wait_for(&mut a_events, |m| match m {
        ServerMessage::Shutdown { reason } => Some(reason.clone()),
        _ => None,
    })
    .await;
    server.await.unwrap().unwrap();
    assert!(
        DaemonFile::read().is_none(),
        "daemon.json is removed on exit"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rest_api_lists_directories_and_runs_commands() {
    let script = Scripted::new(Vec::new());
    let daemon = start_daemon(script, false).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());

    let http = reqwest::Client::new();
    let unauthorized = http
        .get(format!("{}/api/daemon", file.url))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);
    let foreign_host = http
        .get(format!("{}/api/daemon", file.url))
        .bearer_auth(&file.token)
        .header("Host", "attacker.example:80")
        .send()
        .await
        .unwrap();
    assert_eq!(foreign_host.status(), 403, "DNS rebinding is refused");

    let home = dirs::home_dir().unwrap();
    let listing: serde_json::Value = malac::client::get(&file, "/api/fs").await.unwrap();
    assert_eq!(
        std::path::Path::new(listing["root"].as_str().unwrap()),
        home.canonicalize().unwrap()
    );
    assert!(listing["entries"].is_array());
    let outside = http
        .get(format!("{}/api/fs?path=/", file.url))
        .bearer_auth(&file.token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        outside.status(),
        403,
        "the picker stays inside the home directory"
    );

    let reply: serde_json::Value = malac::client::post(
        &file,
        "/api/command",
        &json!({ "type": "workspace_create", "name": "via rest", "client_ref": "x1" }),
    )
    .await
    .unwrap();
    assert_eq!(reply["type"], "workspace_opened");
    assert_eq!(reply["client_ref"], "x1");
    let listed: serde_json::Value = malac::client::get(&file, "/api/workspaces").await.unwrap();
    assert!(
        listed["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["name"] == "via rest")
    );
    let error = malac::client::post::<serde_json::Value>(
        &file,
        "/api/command",
        &json!({ "type": "workspace_open", "workspace": "nope" }),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("no workspace `nope`"));

    let stopped: serde_json::Value = malac::client::post(&file, "/api/daemon/stop", &json!({}))
        .await
        .unwrap();
    assert_eq!(stopped["stopping"], true);
    assert_eq!(hub.stopped().await, "stopped by a client");
    server.await.unwrap().unwrap();
}
