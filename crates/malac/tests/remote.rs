//! Remote access: pairing, device tokens and revoking them, which hosts and
//! origins the daemon accepts, clients on a saved connection, tunnels, and
//! keep-awake following the work.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use futures::StreamExt;
use malac::client::{Client, DaemonFile};
use malac::connections::{self, Connection, Connections, Endpoint};
use malac::daemon::awake::{Hold, Runner};
use malac::daemon::tunnel::Spec;
use malac::protocol::{
    Command, Launch, Paired, Pairing, RemoteStatus, ServerMessage, TunnelState, UrlKind,
    WorkspaceChoice,
};
use malac_core::config::{KeepAwake, Tunnel};
use malac_core::event::Event;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const TUNNEL_HOST: &str = "quiet-river.trycloudflare.com";

fn forget_devices() {
    let _ = std::fs::remove_file(home().join("devices.json"));
    let _ = std::fs::remove_file(home().join("connections.json"));
}

/// A request that looks like it came through a tunnel: a public `Host`.
fn through_tunnel(
    http: &reqwest::Client,
    method: reqwest::Method,
    url: String,
) -> reqwest::RequestBuilder {
    http.request(method, url).header("Host", TUNNEL_HOST)
}

async fn pair(http: &reqwest::Client, base: &str, code: &str, name: &str) -> reqwest::Response {
    through_tunnel(http, reqwest::Method::POST, format!("{base}/api/pair"))
        .json(&json!({ "code": code, "name": name, "kind": "cli" }))
        .send()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_gives_devices_their_own_revocable_tokens() {
    let _guard = exclusive().await;
    forget_devices();
    let daemon = start_remote_daemon(Scripted::new(Vec::new())).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let base = file.url.clone();
    let http = reqwest::Client::new();

    // The master token works here, never through a tunnel.
    let refused = through_tunnel(&http, reqwest::Method::GET, format!("{base}/api/daemon"))
        .bearer_auth(&file.token)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 401);
    assert!(
        refused
            .text()
            .await
            .unwrap()
            .contains("master token works only")
    );
    let hello: Value = through_tunnel(&http, reqwest::Method::GET, format!("{base}/api/hello"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hello["app"], "malac");
    assert_eq!(hello["remote"], true);

    let pairing: Pairing = malac::client::post(&file, "/api/pair/codes", &json!({}))
        .await
        .unwrap();
    assert_eq!(pairing.code.len(), 11);
    let local = pairing.links.last().unwrap();
    assert_eq!(local.kind, UrlKind::Local);
    assert_eq!(local.url, format!("{base}/#pair={}", pairing.code));
    assert!(local.svg.starts_with("<svg"));

    let wrong = pair(&http, &base, "AAAAA-AAAAA", "intruder").await;
    assert_eq!(wrong.status(), 403);
    let paired = pair(&http, &base, &pairing.code.to_lowercase(), "laptop").await;
    assert_eq!(paired.status(), 200);
    let paired: Paired = paired.json().await.unwrap();
    assert_eq!(paired.device.name, "laptop");
    assert_eq!(paired.device.kind, "cli");
    let again = pair(&http, &base, &pairing.code, "second").await;
    assert_eq!(again.status(), 403, "a code pairs one device");

    // The device token works through the tunnel and here.
    let info = through_tunnel(&http, reqwest::Method::GET, format!("{base}/api/daemon"))
        .bearer_auth(&paired.token)
        .send()
        .await
        .unwrap();
    assert_eq!(info.status(), 200);
    let devices: Value = through_tunnel(&http, reqwest::Method::GET, format!("{base}/api/devices"))
        .bearer_auth(&paired.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(devices["devices"][0]["id"], paired.device.id.as_str());
    assert_eq!(devices["devices"][0]["current"], true);
    assert_eq!(hub.remote_status().devices, 1);

    // A device's socket closes as soon as it is revoked.
    let device_file = DaemonFile::remote("laptop", &base, &paired.token);
    assert_eq!(
        device_file.web_url(),
        format!("{base}/"),
        "browsers pair on their own"
    );
    let (_device, mut device_events, _) = Client::connect(&device_file).await.unwrap();
    let (owner, mut owner_events, _) = Client::connect(&file).await.unwrap();
    let revoked: Value = owner
        .delete(&format!("/api/devices/{}", paired.device.id))
        .await
        .unwrap();
    assert_eq!(revoked["revoked"]["name"], "laptop");
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        while device_events.recv().await.is_some() {}
    })
    .await;
    assert!(closed.is_ok(), "the revoked device was disconnected");
    let remote = wait_for(&mut owner_events, |m| match m {
        ServerMessage::Remote { remote } => Some(remote.clone()),
        _ => None,
    })
    .await;
    assert_eq!(remote.devices, 0);
    let after = through_tunnel(&http, reqwest::Method::GET, format!("{base}/api/daemon"))
        .bearer_auth(&paired.token)
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), 401);

    // Too many wrong codes withdraw the open ones.
    let pairing: Pairing = owner.post("/api/pair/codes", &json!({})).await.unwrap();
    let mut last = 0;
    for _ in 0..10 {
        last = pair(&http, &base, "AAAAA-AAAAA", "guess")
            .await
            .status()
            .as_u16();
    }
    assert_eq!(last, 429);
    assert_eq!(
        pair(&http, &base, &pairing.code, "late").await.status(),
        429,
        "until a new code is made"
    );

    hub.request_stop("test over");
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn without_remote_access_only_local_requests_get_in() {
    let _guard = exclusive().await;
    forget_devices();
    let daemon = start_daemon(Scripted::new(Vec::new()), false).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let base = file.url.clone();
    let http = reqwest::Client::new();

    for (host, forwarded) in [
        ("evil.example", None),
        (TUNNEL_HOST, None),
        ("127.0.0.1", Some("203.0.113.9")),
    ] {
        for path in ["/", "/api/hello", "/api/daemon"] {
            let mut request = http
                .get(format!("{base}{path}"))
                .bearer_auth(&file.token)
                .header("Host", host);
            if let Some(ip) = forwarded {
                request = request.header("X-Forwarded-For", ip);
            }
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), 403, "{host} {path}");
        }
    }
    let pairing = pair(&http, &base, "AAAAA-AAAAA", "x").await;
    assert_eq!(
        pairing.status(),
        403,
        "pairing through a tunnel is refused too"
    );

    // An SSH tunnel looks local: devices pair and connect without remote access.
    let code: Pairing = malac::client::post(&file, "/api/pair/codes", &json!({}))
        .await
        .unwrap();
    assert_eq!(code.links.len(), 1, "only the loopback address");
    let paired: Paired = http
        .post(format!("{base}/api/pair"))
        .json(&json!({ "code": code.code, "name": "over ssh" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device = DaemonFile::remote("ssh", &base, &paired.token);
    let (_client, _events, snapshot) = Client::connect(&device).await.unwrap();
    assert!(!snapshot.remote.enabled);
    assert_eq!(snapshot.remote.devices, 1);

    // Tunnels need remote access.
    let spec = Spec::with_program(Tunnel::Cloudflare, "/bin/true".into(), 1);
    let refused = hub.start_tunnel_with(spec).await.unwrap_err();
    assert!(refused.to_string().contains("remote access is off"));

    hub.request_stop("test over");
    server.await.unwrap().unwrap();
}

/// Upgrades `/api/ws` with the given headers and returns the HTTP status.
async fn upgrade(base: &str, token: &str, headers: &[(&str, &str)]) -> u16 {
    let url = format!(
        "{}/api/ws?token={token}",
        base.replacen("http://", "ws://", 1)
    );
    let mut request = url.into_client_request().unwrap();
    for (name, value) in headers {
        request.headers_mut().insert(
            reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    match tokio_tungstenite::connect_async(request).await {
        Ok((mut socket, response)) => {
            let first = socket.next().await.unwrap().unwrap();
            assert!(first.to_text().unwrap().contains("\"snapshot\""));
            response.status().as_u16()
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => response.status().as_u16(),
        Err(err) => panic!("{err}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sockets_accept_their_own_origin_only() {
    let _guard = exclusive().await;
    forget_devices();
    let daemon = start_remote_daemon(Scripted::new(Vec::new())).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let base = file.url.clone();
    let http = reqwest::Client::new();
    let (code, _) = hub.access().create_code();
    let paired: Paired = pair(&http, &base, &code, "phone")
        .await
        .json()
        .await
        .unwrap();
    let token = paired.token.as_str();

    let page = format!("https://{TUNNEL_HOST}");
    assert_eq!(
        upgrade(&base, token, &[("host", TUNNEL_HOST), ("origin", &page)]).await,
        101
    );
    assert_eq!(
        upgrade(
            &base,
            token,
            &[("host", TUNNEL_HOST), ("origin", "https://evil.example")]
        )
        .await,
        403
    );
    // A proxy that rewrites Host keeps the original in X-Forwarded-Host.
    let rewritten = [
        ("host", "127.0.0.1"),
        ("x-forwarded-host", TUNNEL_HOST),
        ("origin", page.as_str()),
    ];
    assert_eq!(upgrade(&base, token, &rewritten).await, 101);
    assert_eq!(
        upgrade(&base, token, &[("host", TUNNEL_HOST)]).await,
        101,
        "apps send no Origin"
    );
    assert_eq!(
        upgrade(&base, &file.token, &[("host", TUNNEL_HOST)]).await,
        401,
        "the master token stays local"
    );
    let local_page = base.clone();
    assert_eq!(
        upgrade(&base, &file.token, &[("origin", &local_page)]).await,
        101
    );
    assert_eq!(
        upgrade(&base, &file.token, &[("origin", "vscode-webview://abc")]).await,
        101
    );

    hub.request_stop("test over");
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_saved_connection_drives_another_daemon() {
    let _guard = exclusive().await;
    forget_devices();
    let script = Scripted::new(vec![answer("hello from the server")]);
    let daemon = start_remote_daemon(script.clone()).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let http = reqwest::Client::new();

    let (code, _) = hub.access().create_code();
    let link = format!("{}/#pair={code}", file.url);
    let (base, linked) = connections::parse_address(&link).unwrap();
    assert_eq!(base, file.url);
    let paired: Paired = http
        .post(format!("{base}/api/pair"))
        .json(&json!({ "code": linked.unwrap(), "name": "this mac", "kind": "cli" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut saved = Connections::default();
    saved.put(Connection {
        name: "server".into(),
        url: base.clone(),
        token: paired.token.clone(),
        device: Some(paired.device.id.clone()),
        added: chrono::Utc::now(),
    });
    saved.save().unwrap();
    connections::remember(Some("server"));
    assert_eq!(
        connections::remembered().map(|c| c.name),
        Some("server".to_owned())
    );
    assert!(connections::resolve(Some("nope")).is_err());
    assert_eq!(
        connections::resolve(Some("local")).unwrap(),
        Endpoint::Local
    );

    let endpoint = connections::resolve(Some("server")).unwrap();
    assert_eq!(endpoint.label(), "server");
    let (client, mut events, snapshot) = endpoint.connect().await.unwrap();
    assert_eq!(client.file().connection.as_deref(), Some("server"));
    assert!(snapshot.remote.enabled);
    let project = project(&[]);
    let launched = client
        .launch(Launch {
            workspace: WorkspaceChoice::New,
            cwd: project.clone(),
            name: Some("on the server".into()),
            prompt: Some("say hello".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(launched.workspace.tabs[0].cwd, project);
    let agent = launched.agent.unwrap();
    let text = wait_for(&mut events, |m| match m {
        ServerMessage::Event { envelope, .. } if envelope.agent == agent => match &envelope.event {
            Event::AssistantMessage { message } => Some(message.text()),
            _ => None,
        },
        _ => None,
    })
    .await;
    assert_eq!(text, "hello from the server");

    // Clients elsewhere name directories on the daemon's machine; `~` is its home.
    let tab = client
        .request(Command::TabCreate(malac::protocol::TabSpec {
            cwd: "~".into(),
            reuse: true,
            ..Default::default()
        }))
        .await
        .unwrap();
    assert!(matches!(tab, ServerMessage::Ack { tab: Some(_), .. }));
    let missing = client
        .request(Command::TabCreate(malac::protocol::TabSpec {
            cwd: "/no/such/dir/on/the/server".into(),
            ..Default::default()
        }))
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("on the daemon's machine"));

    hub.request_stop("test over");
    server.await.unwrap().unwrap();
    forget_devices();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_tunnel_runs_with_the_daemon_and_leads_the_pairing_links() {
    use std::os::unix::fs::PermissionsExt;
    let _guard = exclusive().await;
    forget_devices();
    let daemon = start_remote_daemon(Scripted::new(Vec::new())).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let (client, mut events, _) = Client::connect(&file).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("cloudflared");
    std::fs::write(
        &fake,
        format!("#!/bin/sh\necho \"INF |  https://{TUNNEL_HOST}  |\" >&2\nexec sleep 60\n"),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let spec = Spec::with_program(Tunnel::Cloudflare, fake, 1);
    hub.start_tunnel_with(spec).await.unwrap();
    let remote = wait_for(&mut events, |m| match m {
        ServerMessage::Remote { remote }
            if remote
                .tunnel
                .as_ref()
                .is_some_and(|t| t.state == TunnelState::Running) =>
        {
            Some(remote.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(remote.urls[0].kind, UrlKind::Tunnel);
    assert_eq!(remote.urls[0].url, format!("https://{TUNNEL_HOST}"));
    let pairing: Pairing = client.post("/api/pair/codes", &json!({})).await.unwrap();
    assert!(
        pairing.links[0]
            .url
            .starts_with(&format!("https://{TUNNEL_HOST}/#pair="))
    );
    let listed: RemoteStatus = client.get("/api/remote").await.unwrap();
    assert_eq!(listed.tunnel.unwrap().state, TunnelState::Running);

    let stopped: Value = client.delete("/api/tunnel").await.unwrap();
    assert_eq!(stopped["stopped"], true);
    let remote: RemoteStatus = client.get("/api/remote").await.unwrap();
    assert_eq!(remote.tunnel.unwrap().state, TunnelState::Stopped);
    assert_ne!(remote.urls[0].kind, UrlKind::Tunnel);
    let unknown = client
        .post::<Value>("/api/tunnel", &json!({ "provider": "carrier-pigeon" }))
        .await
        .unwrap_err();
    assert!(unknown.to_string().contains("unknown tunnel"));

    hub.request_stop("test over");
    server.await.unwrap().unwrap();
}

/// Records holds instead of running caffeinate or systemd-inhibit.
#[derive(Default)]
struct Recorder {
    log: Arc<Mutex<Vec<&'static str>>>,
}

struct Held(Arc<Mutex<Vec<&'static str>>>);

impl Hold for Held {
    fn check(&mut self) -> Result<(), String> {
        Ok(())
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.0.lock().unwrap().push("release");
    }
}

impl Runner for Recorder {
    fn method(&self) -> Option<&'static str> {
        Some("recorder")
    }

    fn hold(&self) -> Result<Box<dyn Hold>, String> {
        self.log.lock().unwrap().push("hold");
        Ok(Box::new(Held(self.log.clone())))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn keep_awake_holds_while_jobs_run() {
    let _guard = exclusive().await;
    let script = Scripted::new(vec![
        tool_call(
            "c1",
            "bash",
            json!({ "command": "sleep 2", "background": true }),
        ),
        answer("started"),
    ]);
    let daemon = start_daemon(script, false).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let recorder = Arc::new(Recorder::default());
    let log = recorder.log.clone();
    hub.set_awake_runner(recorder, Duration::ZERO);
    let server = tokio::spawn(daemon.run());
    let (client, mut events, snapshot) = Client::connect(&file).await.unwrap();
    assert_eq!(
        snapshot.awake.mode,
        KeepAwake::Off,
        "the test config turns it off"
    );
    assert!(!snapshot.awake.awake);

    client
        .request(Command::KeepAwake {
            mode: KeepAwake::Busy,
        })
        .await
        .unwrap();
    let idle = wait_for(&mut events, |m| match m {
        ServerMessage::Awake { awake } => Some(awake.clone()),
        _ => None,
    })
    .await;
    assert_eq!(idle.mode, KeepAwake::Busy);
    assert!(!idle.awake);
    assert_eq!(idle.method.as_deref(), Some("recorder"));

    client
        .launch(Launch {
            workspace: WorkspaceChoice::New,
            cwd: project(&[]),
            prompt: Some("start a job".into()),
            options: malac::protocol::TabOptions {
                mode: Some(malac_core::permission::Mode::Autopilot),
                ..Default::default()
            },
            ..Default::default()
        })
        .await
        .unwrap();
    let awake = wait_for(&mut events, |m| match m {
        ServerMessage::Awake { awake } if awake.awake => Some(awake.clone()),
        _ => None,
    })
    .await;
    assert!(awake.reason.contains("running"), "{}", awake.reason);
    let released = wait_for(&mut events, |m| match m {
        ServerMessage::Awake { awake } if !awake.awake => Some(awake.clone()),
        _ => None,
    })
    .await;
    assert_eq!(released.reason, "nothing is running");
    assert_eq!(*log.lock().unwrap(), ["hold", "release"]);

    client
        .request(Command::KeepAwake {
            mode: KeepAwake::Always,
        })
        .await
        .unwrap();
    wait_for(&mut events, |m| match m {
        ServerMessage::Awake { awake } if awake.awake => Some(()),
        _ => None,
    })
    .await;
    let info = client.info().await.unwrap();
    assert!(info.awake.awake);
    assert_eq!(info.awake.mode, KeepAwake::Always);

    hub.request_stop("test over");
    server.await.unwrap().unwrap();
    assert_eq!(
        log.lock().unwrap().last(),
        Some(&"release"),
        "stopping the daemon lets the machine sleep"
    );
}
