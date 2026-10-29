//! Push notifications: a paired phone registers a Web Push endpoint, a
//! waiting approval reaches it encrypted and signed, an answer sends a
//! follow-up, and endpoints that are gone or belong to revoked devices are
//! dropped.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::*;
use malac::client::Client;
use malac::protocol::{Command, Paired, ServerMessage, TabSpec};
use malac_core::event::Event;
use malac_core::permission::Decision;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};
use ring::{aead, agreement, hkdf};
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// What a push service received.
struct Received {
    headers: HeaderMap,
    body: Vec<u8>,
}

#[derive(Clone)]
struct PushService {
    received: mpsc::UnboundedSender<Received>,
    status: Arc<Mutex<StatusCode>>,
}

async fn receive(
    State(service): State<PushService>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let _ = service.received.send(Received {
        headers,
        body: body.to_vec(),
    });
    *service.status.lock().unwrap()
}

/// A push service on a free port: its base URL, what it receives, and the
/// status it answers with.
async fn push_service() -> (
    String,
    mpsc::UnboundedReceiver<Received>,
    Arc<Mutex<StatusCode>>,
) {
    let (received, inbox) = mpsc::unbounded_channel();
    let status = Arc::new(Mutex::new(StatusCode::CREATED));
    let service = PushService {
        received,
        status: status.clone(),
    };
    let app = Router::new()
        .route("/up/{topic}", post(receive))
        .with_state(service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, inbox, status)
}

/// A phone's push keys: its private key (good for one decryption), public
/// key and auth secret, both base64url.
fn device_keys() -> (agreement::EphemeralPrivateKey, String, Vec<u8>) {
    let rng = SystemRandom::new();
    let private = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
    let public = URL_SAFE_NO_PAD.encode(private.compute_public_key().unwrap().as_ref());
    let mut auth = vec![0u8; 16];
    rng.fill(&mut auth).unwrap();
    (private, public, auth)
}

fn derive(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Vec<u8> {
    struct Len(usize);
    impl hkdf::KeyType for Len {
        fn len(&self) -> usize {
            self.0
        }
    }
    let mut out = vec![0; len];
    hkdf::Salt::new(hkdf::HKDF_SHA256, salt)
        .extract(ikm)
        .expand(&[info], Len(len))
        .unwrap()
        .fill(&mut out)
        .unwrap();
    out
}

/// RFC 8291 decryption, as the phone's push library does it.
fn decrypt(
    private: agreement::EphemeralPrivateKey,
    public: &str,
    auth: &[u8],
    body: &[u8],
) -> Value {
    let (salt, rest) = body.split_at(16);
    let key_len = rest[4] as usize;
    let sender = &rest[5..5 + key_len];
    let mut record = rest[5 + key_len..].to_vec();
    let device_key = URL_SAFE_NO_PAD.decode(public).unwrap();
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, sender);
    let shared = agreement::agree_ephemeral(private, &peer, |s| s.to_vec()).unwrap();
    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(&device_key);
    info.extend_from_slice(sender);
    let ikm = derive(auth, &shared, &info, 32);
    let key = derive(salt, &ikm, b"Content-Encoding: aes128gcm\0", 16);
    let nonce = derive(salt, &ikm, b"Content-Encoding: nonce\0", 12);
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &key).unwrap());
    let nonce = aead::Nonce::try_assume_unique_for_key(&nonce).unwrap();
    let plain = key
        .open_in_place(nonce, aead::Aad::empty(), &mut record)
        .unwrap();
    serde_json::from_slice(&plain[..plain.len() - 1]).unwrap()
}

async fn next(inbox: &mut mpsc::UnboundedReceiver<Received>) -> Received {
    tokio::time::timeout(Duration::from_secs(10), inbox.recv())
        .await
        .expect("timed out waiting for a push")
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn waiting_approvals_reach_registered_phones() {
    let _guard = exclusive().await;
    let config = home().join("config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    if !text.contains("[push]") {
        std::fs::write(&config, format!("{text}\n[push]\nwait_seconds = 0\n")).unwrap();
    }
    let _ = std::fs::remove_file(home().join("push.json"));
    let script = Scripted::new(vec![
        tool_call("c1", "bash", json!({ "command": "echo one" })),
        answer("ran it"),
        tool_call("c2", "bash", json!({ "command": "echo two" })),
        answer("ran that too"),
    ]);
    let daemon = start_daemon(script, false).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let http = reqwest::Client::new();
    let base = file.url.clone();
    let (push_base, mut inbox, push_status) = push_service().await;

    // Pair a phone and register its endpoint.
    let (code, _) = hub.access().create_code();
    let paired: Paired = http
        .post(format!("{base}/api/pair"))
        .json(&json!({ "code": code, "name": "phone", "kind": "android" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let phone = |request: reqwest::RequestBuilder| request.bearer_auth(&paired.token);
    let status: Value = phone(http.get(format!("{base}/api/push")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["registered"], false);
    let vapid_key = status["vapid_key"].as_str().unwrap().to_owned();
    assert_eq!(URL_SAFE_NO_PAD.decode(&vapid_key).unwrap().len(), 65);

    let (private, public, auth) = device_keys();
    let endpoint = format!("{push_base}/up/phone1");
    let registered = phone(http.put(format!("{base}/api/push")))
        .json(&json!({ "endpoint": endpoint, "p256dh": public, "auth": URL_SAFE_NO_PAD.encode(&auth) }))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), 200);
    let owner = http
        .put(format!("{base}/api/push"))
        .bearer_auth(&file.token)
        .json(&json!({ "endpoint": endpoint, "p256dh": public, "auth": URL_SAFE_NO_PAD.encode(&auth) }))
        .send()
        .await
        .unwrap();
    assert_eq!(owner.status(), 400, "only paired devices register");
    assert!(hub.push().is_registered(&paired.device.id));

    // An approval waits: the phone gets the request id, encrypted and signed.
    let (client, mut events, _) = Client::connect(&file).await.unwrap();
    let project = project(&[]);
    let ServerMessage::Ack { tab: Some(tab), .. } = client
        .request(Command::TabCreate(TabSpec {
            cwd: project,
            ..Default::default()
        }))
        .await
        .unwrap()
    else {
        panic!("a tab expected");
    };
    client
        .request(Command::Prompt {
            tab: Some(tab),
            agent: None,
            text: "run it".into(),
            images: Vec::new(),
            attachments: Vec::new(),
            model: None,
        })
        .await
        .unwrap();
    let request = wait_for(&mut events, |m| match m {
        ServerMessage::Event { envelope, .. } => match &envelope.event {
            Event::PermissionRequested { request_id, .. } => Some(request_id.clone()),
            _ => None,
        },
        _ => None,
    })
    .await;
    let push = next(&mut inbox).await;
    assert_eq!(push.headers["content-encoding"], "aes128gcm");
    assert_eq!(push.headers["urgency"], "high");
    assert_eq!(push.headers["ttl"], "3600");
    let authorization = push.headers["authorization"].to_str().unwrap();
    let (token, key) = authorization
        .strip_prefix("vapid t=")
        .unwrap()
        .split_once(", k=")
        .unwrap();
    assert_eq!(key, vapid_key);
    let (signed, signature) = token.rsplit_once('.').unwrap();
    UnparsedPublicKey::new(
        &ECDSA_P256_SHA256_FIXED,
        URL_SAFE_NO_PAD.decode(key).unwrap(),
    )
    .verify(
        signed.as_bytes(),
        &URL_SAFE_NO_PAD.decode(signature).unwrap(),
    )
    .expect("signed with the daemon's VAPID key");
    let claims: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(signed.split_once('.').unwrap().1)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(claims["aud"], push_base);
    // It carries what the notification shows, for a phone that cannot
    // reach the daemon yet.
    let message = decrypt(private, &public, &auth, &push.body);
    assert_eq!(message["type"], "approvals");
    let sent = &message["requests"][0];
    assert_eq!(sent["id"], request.as_str());
    assert_eq!(sent["tool"], "bash");
    assert_eq!(sent["summary"], "echo one");
    assert_eq!(sent["agent_label"], "main");
    assert!(sent["agent"].is_string() && sent["tab"].is_string());
    assert!(!sent["place"].as_str().unwrap().is_empty());

    // Answering it takes the notification down.
    client
        .request(Command::Approve {
            request_id: request.clone(),
            decision: Decision::Allow,
        })
        .await
        .unwrap();
    let follow_up = next(&mut inbox).await;
    assert!(follow_up.body.len() > 86, "an encrypted follow-up");

    // A push service that no longer knows the endpoint gets it dropped.
    *push_status.lock().unwrap() = StatusCode::GONE;
    let agent = wait_for(&mut events, |m| match m {
        ServerMessage::Event { envelope, .. } => match &envelope.event {
            Event::TurnFinished { .. } => Some(envelope.agent.clone()),
            _ => None,
        },
        _ => None,
    })
    .await;
    client
        .request(Command::Prompt {
            tab: None,
            agent: Some(agent),
            text: "and again".into(),
            images: Vec::new(),
            attachments: Vec::new(),
            model: None,
        })
        .await
        .unwrap();
    next(&mut inbox).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while hub.push().is_registered(&paired.device.id) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the gone endpoint stays"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Revoking a device forgets its endpoint too.
    let (_, public, auth) = device_keys();
    phone(http.put(format!("{base}/api/push")))
        .json(&json!({ "endpoint": endpoint, "p256dh": public, "auth": URL_SAFE_NO_PAD.encode(&auth) }))
        .send()
        .await
        .unwrap();
    assert!(hub.push().is_registered(&paired.device.id));
    hub.access().revoke(&paired.device.id).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while hub.push().is_registered(&paired.device.id) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "a revoked device keeps its endpoint"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    drop(client);
    hub.request_stop("test done");
    server.await.unwrap().unwrap();
}
