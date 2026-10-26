//! Two clients drive one in-process daemon over its WebSocket protocol.

mod common;

use common::*;
use malac::client::{Client, DaemonFile};
use malac::protocol::{AttachmentInput, Command, ImageInput, ServerMessage, TabSpec};
use malac_core::attach::AttachmentKind;
use malac_core::event::{Event, TurnStatus};
use malac_core::permission::{Decision, Mode};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_share_tabs_agents_and_approvals() {
    let _guard = exclusive().await;
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
    wait_for(&mut a_events, |m| match m {
        ServerMessage::Clients { clients: 2 } => Some(()),
        _ => None,
    })
    .await;
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
    let work = workspace.id.clone();

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
            attachments: vec![AttachmentInput::path("notes.txt")],
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
    // A starting turn updates the workspace's running count.
    wait_for(&mut b_events, |m| match m {
        ServerMessage::WorkspaceUpdated { workspace } if workspace.id == work => {
            (workspace.running == 1).then_some(())
        }
        _ => None,
    })
    .await;

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
    assert_eq!(snapshot.pending[0].request.call_id, "c1");
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
    wait_for(&mut b_events, |m| match m {
        ServerMessage::WorkspaceUpdated { workspace } if workspace.running == 0 => Some(()),
        _ => None,
    })
    .await;
    let sessions: serde_json::Value = a
        .get(&format!("/api/sessions?cwd={}", project.display()))
        .await
        .unwrap();
    let sessions = sessions["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], agent.as_str());
    assert_eq!(sessions[0]["title"], "run it", "titles skip attached files");
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

    a.request(Command::Mode {
        mode: Mode::Edits,
        tab: None,
    })
    .await
    .unwrap();
    let scope = wait_for(&mut b_events, |m| match m {
        ServerMessage::Mode {
            mode: Mode::Edits,
            tab: None,
            workspace,
        } => Some(workspace.clone()),
        _ => None,
    })
    .await;
    assert_eq!(scope.as_deref(), Some(work.as_str()));

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

    // Closing takes an agent out of its tab, whether it ran or not.
    for (closing, left) in [(&agent, vec![helper.clone()]), (&helper, vec![])] {
        a.request(Command::AgentClose {
            tab: tab.clone(),
            agent: closing.clone(),
        })
        .await
        .unwrap();
        let after = wait_for(&mut b_events, |m| match m {
            ServerMessage::TabUpdated { tab, .. } if tab.agents == left => Some(tab.clone()),
            _ => None,
        })
        .await;
        assert!(
            after
                .snapshots
                .iter()
                .all(|s| s.snapshot.info.id != *closing)
        );
        assert_eq!(after.active_agent, left.first().cloned());
    }
    assert!(hub.live_agents().is_empty(), "closed agents are unloaded");
    let reopened = a
        .request(Command::AgentOpen {
            agent: agent.clone(),
            tab: Some(tab.clone()),
        })
        .await
        .unwrap();
    assert!(matches!(reopened, ServerMessage::Ack { agent: Some(ref id), .. } if *id == agent));

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
    let _guard = exclusive().await;
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

    let root = home().join("browse").canonicalize().unwrap();
    let listing: serde_json::Value = malac::client::get(&file, "/api/fs").await.unwrap();
    assert_eq!(
        std::path::Path::new(listing["root"].as_str().unwrap()),
        root
    );
    assert_eq!(listing["entries"][0]["name"], "app");
    assert_eq!(listing["entries"][0]["kind"], "directory");
    assert_eq!(listing["entries"][0]["markers"], json!(["Cargo.toml"]));
    let folders: serde_json::Value = malac::client::get(&file, "/api/fs?path=app").await.unwrap();
    let names: Vec<&str> = folders["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["src"], "folders only by default");
    let files: serde_json::Value = malac::client::get(&file, "/api/fs?path=app&files=true")
        .await
        .unwrap();
    let entries: Vec<(&str, &str, Option<u64>)> = files["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap(),
                e["kind"].as_str().unwrap(),
                e["size"].as_u64(),
            )
        })
        .collect();
    assert_eq!(
        entries,
        [
            ("src", "directory", None),
            ("Cargo.toml", "file", Some(10)),
            ("notes.txt", "file", Some(6)),
        ]
    );
    let outside = http
        .get(format!("{}/api/fs?path=/", file.url))
        .bearer_auth(&file.token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        outside.status(),
        403,
        "the picker stays inside its root, files or not"
    );
    let outside_files = http
        .get(format!("{}/api/fs?path=/etc&files=true", file.url))
        .bearer_auth(&file.token)
        .send()
        .await
        .unwrap();
    assert_eq!(outside_files.status(), 403);
    let sessions: serde_json::Value = malac::client::get(&file, "/api/sessions?limit=5")
        .await
        .unwrap();
    assert!(sessions["sessions"].is_array());

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

#[tokio::test(flavor = "multi_thread")]
async fn tab_create_switches_a_connection_to_its_workspace() {
    let _guard = exclusive().await;
    let daemon = start_daemon(Scripted::new(Vec::new()), false).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let (client, mut events, snapshot) = Client::connect(&file).await.unwrap();
    assert!(snapshot.workspace.is_none());

    let project = project(&[]);
    let reply = client
        .request(Command::TabCreate(TabSpec {
            cwd: project.clone(),
            ..Default::default()
        }))
        .await
        .unwrap();
    let ServerMessage::Ack {
        tab: Some(tab),
        workspace: Some(workspace),
        ..
    } = reply
    else {
        panic!("ack with a tab and its workspace expected, got {reply:?}");
    };
    // The announcement came before the reply.
    let mut announced = None;
    while let Ok(message) = events.try_recv() {
        if let ServerMessage::TabCreated {
            tab: created,
            workspace: home,
            ..
        } = message
        {
            announced = Some((created.id, home));
        }
    }
    assert_eq!(announced, Some((tab.clone(), workspace.clone())));

    let reply = client
        .request(Command::Snapshot { workspace: None })
        .await
        .unwrap();
    let ServerMessage::Snapshot(snapshot) = reply else {
        panic!("snapshot expected");
    };
    let view = snapshot
        .workspace
        .expect("the connection has a workspace now");
    assert_eq!(view.id, workspace);
    assert_eq!(view.name, "default");
    assert_eq!(view.active_tab.as_deref(), Some(tab.as_str()));

    hub.request_stop("done");
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_running_agent_cancels_it() {
    let _guard = exclusive().await;
    let script = Scripted::new(vec![tool_call(
        "c1",
        "bash",
        json!({ "command": "echo never" }),
    )]);
    let daemon = start_daemon(script, false).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let (client, mut events, _) = Client::connect(&file).await.unwrap();
    let project = project(&[]);
    let launched = client
        .launch(malac::protocol::Launch {
            workspace: malac::protocol::WorkspaceChoice::New,
            cwd: project.clone(),
            prompt: Some("run something".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let (tab, agent) = (launched.tab.unwrap(), launched.agent.unwrap());
    // Waiting for approval, so the turn is still running.
    wait_for(&mut events, |m| match m {
        ServerMessage::Event { envelope, .. } => {
            matches!(envelope.event, Event::PermissionRequested { .. }).then_some(())
        }
        _ => None,
    })
    .await;
    assert!(hub.agent_busy(&agent));

    client
        .request(Command::AgentClose {
            tab: tab.clone(),
            agent: agent.clone(),
        })
        .await
        .unwrap();
    let mut cancelled = false;
    let mut resolved = false;
    let mut removed = false;
    while !(cancelled && resolved && removed) {
        match wait_for(&mut events, |m| Some(m.clone())).await {
            ServerMessage::Event { envelope, .. } => match envelope.event {
                Event::TurnFinished { status, .. } => {
                    assert_eq!(status, TurnStatus::Cancelled);
                    cancelled = true;
                }
                Event::PermissionResolved { allowed, .. } => {
                    assert!(!allowed);
                    resolved = true;
                }
                _ => {}
            },
            ServerMessage::TabUpdated { tab: view, .. } if view.agents.is_empty() => removed = true,
            _ => {}
        }
    }
    assert!(hub.pending_requests().is_empty());
    assert!(
        malac_core::session::summary(&agent).is_some(),
        "its session stays"
    );

    hub.request_stop("done");
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn pasted_images_travel_with_the_prompt() {
    let _guard = exclusive().await;
    let script = Scripted::new(vec![answer("a red square")]);
    let daemon = start_daemon(script.clone(), false).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let (client, mut events, _) = Client::connect(&file).await.unwrap();
    let project = project(&[]);
    let reply = client
        .request(Command::TabCreate(TabSpec {
            cwd: project,
            ..Default::default()
        }))
        .await
        .unwrap();
    let ServerMessage::Ack { tab: Some(tab), .. } = reply else {
        panic!("ack expected");
    };
    client
        .request(Command::Prompt {
            tab: Some(tab),
            agent: None,
            text: "what is this?".into(),
            images: vec![ImageInput {
                media_type: "image/png".into(),
                data: "iVBORw0KGgo=".into(),
                name: Some("pasted-1.png".into()),
            }],
            attachments: Vec::new(),
            model: None,
        })
        .await
        .unwrap();
    let shown = wait_for(&mut events, |m| match m {
        ServerMessage::Event { envelope, .. } => match &envelope.event {
            Event::UserMessage { text } => Some(text.clone()),
            _ => None,
        },
        _ => None,
    })
    .await;
    assert_eq!(shown, "what is this?\n\n[attached: pasted-1.png]");
    wait_for(&mut events, |m| match m {
        ServerMessage::Event { envelope, .. } => {
            matches!(envelope.event, Event::TurnFinished { .. }).then_some(())
        }
        _ => None,
    })
    .await;
    let request = script.last_request();
    let images: Vec<&str> = request
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            malac_core::Block::Image { data, .. } => Some(data.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(images, ["iVBORw0KGgo="]);

    hub.request_stop("done");
    server.await.unwrap().unwrap();
}

/// What a client on another machine does: files go as base64 under their
/// name, and the `@` picker lists the daemon's files.
#[tokio::test(flavor = "multi_thread")]
async fn uploaded_files_travel_with_the_prompt() {
    use base64::Engine;
    let base64 = base64::engine::general_purpose::STANDARD;
    let _guard = exclusive().await;
    let script = Scripted::new(vec![answer("marmalade it is"), answer("a big one")]);
    let daemon = start_daemon(script.clone(), false).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let (client, mut events, _) = Client::connect(&file).await.unwrap();
    let reply = client
        .request(Command::TabCreate(TabSpec {
            cwd: project(&[]),
            ..Default::default()
        }))
        .await
        .unwrap();
    let ServerMessage::Ack { tab: Some(tab), .. } = reply else {
        panic!("ack expected");
    };
    let prompt = |attachments: Vec<AttachmentInput>| Command::Prompt {
        tab: Some(tab.clone()),
        agent: None,
        text: "look".into(),
        images: Vec::new(),
        attachments,
        model: None,
    };
    let upload = |name: &str, data: String| AttachmentInput {
        path: name.into(),
        data: Some(data),
    };
    async fn finished(events: &mut malac::client::Events) {
        wait_for(events, |m| match m {
            ServerMessage::Event { envelope, .. } => {
                matches!(envelope.event, Event::TurnFinished { .. }).then_some(())
            }
            _ => None,
        })
        .await
    }

    let reply = client
        .request(prompt(vec![upload(
            "notes.md",
            base64.encode("the upload word is marmalade\n"),
        )]))
        .await
        .unwrap();
    let ServerMessage::Ack { attachments, .. } = reply else {
        panic!("ack expected, got {reply:?}");
    };
    let saved = &attachments[0].path;
    assert!(saved.starts_with(home().join("uploads")), "{saved:?}");
    assert_eq!(saved.file_name().unwrap(), "notes.md");
    finished(&mut events).await;
    let text = request_text(&script.last_request());
    assert!(text.contains("<attachment path=\"notes.md\""), "{text}");
    assert!(text.contains("the upload word is marmalade"), "{text}");

    // A file at the cap is about 28 MB of base64, sent as one frame.
    const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;
    let full = base64.encode(vec![b'a'; MAX_UPLOAD_BYTES]);
    let reply = client
        .request(prompt(vec![upload("big.txt", full.clone())]))
        .await
        .unwrap();
    assert!(matches!(reply, ServerMessage::Ack { .. }), "{reply:?}");
    finished(&mut events).await;

    // Two of them and a little more are too much for one prompt, and
    // nothing of it is kept.
    let before = std::fs::read_dir(home().join("uploads")).unwrap().count();
    let err = client
        .request(prompt(vec![
            upload("one.txt", full.clone()),
            upload("two.txt", full),
            upload("three.txt", base64.encode("more")),
        ]))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("more than the 40.0 MB one prompt may carry"),
        "{err}"
    );
    let after = std::fs::read_dir(home().join("uploads")).unwrap().count();
    assert_eq!(after, before);

    // Prompts may come over REST too, past axum's 2 MB default.
    let three_mb = base64.encode(vec![b'b'; 3 * 1024 * 1024]);
    let reply: serde_json::Value = malac::client::post(
        &file,
        "/api/command",
        &json!({ "type": "prompt", "tab": tab, "text": "and this",
                 "attachments": [{ "path": "rest.txt", "data": three_mb }] }),
    )
    .await
    .unwrap();
    assert_eq!(reply["type"], "ack", "{reply}");
    finished(&mut events).await;

    let root = home().join("browse").canonicalize().unwrap();
    let all: serde_json::Value = malac::client::get(&file, "/api/files").await.unwrap();
    assert_eq!(std::path::Path::new(all["root"].as_str().unwrap()), root);
    assert_eq!(all["files"], json!(["app/Cargo.toml", "app/notes.txt"]));
    assert_eq!(all["truncated"], false);
    let app: serde_json::Value = malac::client::get(&file, "/api/files?path=app")
        .await
        .unwrap();
    assert_eq!(
        std::path::Path::new(app["root"].as_str().unwrap()),
        root.join("app")
    );
    assert_eq!(app["files"], json!(["Cargo.toml", "notes.txt"]));
    let outside = malac::client::get::<serde_json::Value>(&file, "/api/files?path=/etc").await;
    assert!(outside.is_err(), "the index stays inside the root");

    hub.request_stop("done");
    server.await.unwrap().unwrap();
}
