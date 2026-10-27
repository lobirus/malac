//! Voice and shared files against a fake OpenAI-style audio provider:
//! dictation, speech on the device used last, `send_to_user`, and a talk
//! mode conversation that hands a task to the agent and hears its report.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use base64::Engine;
use common::*;
use futures::{SinkExt, StreamExt};
use malac::client::Client;
use malac::protocol::{Command, ServerMessage, TabSpec};
use malac_core::Event;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as Frame;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const BASE64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Everything the fake provider received over the Realtime socket.
type Heard = Arc<Mutex<Vec<Value>>>;

async fn fake_provider(heard: Heard) -> u16 {
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { axum::Json(json!({ "data": [{ "id": "whisper-1" }, { "id": "tts-1" }, { "id": "gpt-realtime" }] })) }),
        )
        .route(
            "/v1/audio/transcriptions",
            post(|body: axum::body::Bytes| async move {
                let text = String::from_utf8_lossy(&body);
                assert!(text.contains("whisper-1") && text.contains("RIFF"), "a multipart upload with the model and the WAV");
                axum::Json(json!({ "text": " make the button green " }))
            }),
        )
        .route(
            "/v1/audio/speech",
            post(|axum::Json(body): axum::Json<Value>| async move {
                assert_eq!(body["model"], "tts-1");
                ([("content-type", "audio/mpeg")], format!("ID3:{}", body["input"].as_str().unwrap()))
            }),
        )
        .route(
            "/v1/realtime",
            get(move |ws: WebSocketUpgrade| {
                let heard = heard.clone();
                async move { ws.on_upgrade(move |socket| realtime(socket, heard)).into_response() }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// A voice model that asks malac to "say hi" when it hears audio, then
/// passes on what malac reported.
async fn realtime(mut socket: WebSocket, heard: Heard) {
    let send = |value: Value| WsMessage::Text(value.to_string().into());
    let mut asked = false;
    // malac's report can arrive with the tool output or after it.
    let mut reported = false;
    while let Some(Ok(WsMessage::Text(text))) = socket.next().await {
        let event: Value = serde_json::from_str(text.as_str()).unwrap();
        heard.lock().unwrap().push(event.clone());
        match event["type"].as_str().unwrap() {
            "session.update" => socket
                .send(send(json!({ "type": "session.updated" })))
                .await
                .unwrap(),
            "input_audio_buffer.append" if !asked => {
                asked = true;
                for reply in [
                    json!({ "type": "input_audio_buffer.speech_started" }),
                    json!({ "type": "response.created" }),
                    json!({ "type": "response.function_call_arguments.done", "call_id": "f1", "name": "ask_malac", "arguments": "{\"request\":\"say hi\"}" }),
                    json!({ "type": "response.done", "response": { "status": "completed" } }),
                ] {
                    socket.send(send(reply)).await.unwrap();
                }
            }
            "conversation.item.create" if event["item"]["role"] == "system" => reported = true,
            "response.create" => {
                let transcript = if reported {
                    "malac says hi there."
                } else {
                    "Asked malac."
                };
                for reply in [
                    json!({ "type": "response.created" }),
                    json!({ "type": "response.output_audio.delta", "delta": BASE64.encode([1u8, 2, 3, 4]) }),
                    json!({ "type": "response.output_audio_transcript.done", "transcript": transcript }),
                    json!({ "type": "response.done", "response": { "status": "completed" } }),
                ] {
                    socket.send(send(reply)).await.unwrap();
                }
            }
            _ => {}
        }
    }
}

/// A WAV header and a little silence.
fn wav() -> Vec<u8> {
    let mut wav = b"RIFF\x24\x00\x00\x00WAVEfmt ".to_vec();
    wav.extend([
        16, 0, 0, 0, 1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0, 0x7d, 0, 0, 2, 0, 16, 0,
    ]);
    wav.extend(b"data\x00\x00\x00\x00");
    wav
}

/// A command's reply, failing instead of waiting forever.
async fn ask(client: &Client, command: Command) -> ServerMessage {
    tokio::time::timeout(Duration::from_secs(20), client.request(command))
        .await
        .expect("the daemon did not answer")
        .unwrap()
}

async fn finished(events: &mut malac::client::Events) -> String {
    wait_for(events, |m| match m {
        ServerMessage::Event { envelope, .. } => match &envelope.event {
            Event::TurnFinished { text, .. } => Some(text.clone()),
            _ => None,
        },
        _ => None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn voice_features_work_through_the_daemon() {
    let _guard = exclusive().await;
    let heard = Heard::default();
    let port = fake_provider(heard.clone()).await;
    let config = home().join("config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    if !text.contains("[providers.audio]") {
        text.push_str(&format!(
            "\n[providers.audio]\nkind = \"openai\"\nbase_url = \"http://127.0.0.1:{port}/v1\"\nmodels = []\n\n[voice]\nspeech = true\n"
        ));
        std::fs::write(&config, text).unwrap();
    }

    let script = Scripted::new(vec![
        tool_call("s1", "say", json!({ "text": "Tests pass." })),
        tool_call(
            "t1",
            "send_to_user",
            json!({ "files": ["notes.txt"], "message": "the notes" }),
        ),
        answer("sent"),
        answer("hi there"),
    ]);
    let daemon = start_daemon(script.clone(), false).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let api = |path: &str| format!("{}{path}", file.url);

    // The offer comes from the provider's model list.
    eprintln!("step: voice status");
    let voice: Value = http
        .get(api("/api/voice?refresh=true"))
        .bearer_auth(&file.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(voice["dictation"]["model"], "audio/whisper-1");
    assert_eq!(voice["speech"]["model"], "audio/tts-1");
    assert_eq!(voice["talk"]["model"], "audio/gpt-realtime");
    assert_eq!(voice["speech"]["enabled"], true);

    eprintln!("step: dictation");
    // Dictation.
    let reply: Value = http
        .post(api("/api/voice/transcribe"))
        .bearer_auth(&file.token)
        .header("content-type", "audio/wav")
        .body(wav())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reply["text"], "make the button green");

    eprintln!("step: speech");
    // Speech plays on the listening device that was used last.
    let (phone, _phone_events, _) = Client::connect(&file).await.unwrap();
    let (laptop, mut events, _) = Client::connect(&file).await.unwrap();
    let listener = |reply: ServerMessage| match reply {
        ServerMessage::VoiceListening {
            listener: Some(id), ..
        } => id,
        other => panic!("voice_listening expected, got {other:?}"),
    };
    let phone_id = listener(ask(&phone, Command::VoiceListen { on: true }).await);
    let laptop_id = listener(ask(&laptop, Command::VoiceListen { on: true }).await);
    assert_ne!(phone_id, laptop_id);

    let project = project(&[("notes.txt", "remember the milk\n")]);
    let ServerMessage::Ack { tab: Some(tab), .. } = ask(
        &laptop,
        Command::TabCreate(TabSpec {
            cwd: project,
            ..Default::default()
        }),
    )
    .await
    else {
        panic!("ack with a tab expected");
    };
    ask(
        &laptop,
        Command::Prompt {
            tab: Some(tab.clone()),
            agent: None,
            text: "check and share the notes".into(),
            images: Vec::new(),
            attachments: Vec::new(),
            model: None,
        },
    )
    .await;
    let (id, said, to) = wait_for(&mut events, |m| match m {
        ServerMessage::Say {
            id, text, listener, ..
        } => Some((id.clone(), text.clone(), *listener)),
        _ => None,
    })
    .await;
    assert_eq!((said.as_str(), to), ("Tests pass.", laptop_id));
    let audio = http
        .get(api(&format!("/api/voice/say/{id}")))
        .bearer_auth(&file.token)
        .send()
        .await
        .unwrap();
    assert_eq!(audio.headers()["content-type"], "audio/mpeg");
    assert_eq!(audio.bytes().await.unwrap(), "ID3:Tests pass.");

    // send_to_user: the result names a copy clients can fetch.
    let output = wait_for(&mut events, |m| match m {
        ServerMessage::Event { envelope, .. } => match &envelope.event {
            Event::ToolFinished { tool, output, .. } if tool == "send_to_user" => {
                Some(output.clone())
            }
            _ => None,
        },
        _ => None,
    })
    .await;
    let shared = malac_core::shared::parse_tags(&output);
    assert_eq!(shared.len(), 1, "{output}");
    assert!(output.contains("<caption>the notes</caption>"));
    assert_eq!(shared[0].name, "notes.txt");
    let body = http
        .get(api(&format!("/api/shared/{}", shared[0].id)))
        .bearer_auth(&file.token)
        .send()
        .await
        .unwrap();
    assert_eq!(body.headers()["content-type"], "text/plain");
    assert_eq!(body.text().await.unwrap(), "remember the milk\n");
    assert_eq!(finished(&mut events).await, "sent");
    assert!(
        http.get(api("/api/shared/nope"))
            .bearer_auth(&file.token)
            .send()
            .await
            .unwrap()
            .status()
            .is_client_error()
    );

    eprintln!("step: talk");
    // Talk mode: audio in, the voice model asks malac, malac's report goes back.
    let url = format!(
        "{}/api/voice/talk?tab={tab}",
        file.url.replacen("http", "ws", 1)
    );
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", file.token).parse().unwrap(),
    );
    let (mut talk, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let mut seen: Vec<Value> = Vec::new();
    let mut audio_frames = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frame = tokio::time::timeout_at(deadline, talk.next())
            .await
            .unwrap_or_else(|_| panic!("talk mode timed out after {seen:?}"))
            .unwrap()
            .unwrap();
        match frame {
            Frame::Binary(pcm) => {
                assert_eq!(pcm.as_ref(), [1, 2, 3, 4]);
                audio_frames += 1;
            }
            Frame::Text(text) => {
                let message: Value = serde_json::from_str(text.as_str()).unwrap();
                if message["type"] == "ready" {
                    talk.send(Frame::Binary(vec![0u8; 960].into()))
                        .await
                        .unwrap();
                }
                let done =
                    message["type"] == "transcript" && message["text"] == "malac says hi there.";
                assert_ne!(message["type"], "error", "{message}");
                seen.push(message);
                if done {
                    break;
                }
            }
            _ => {}
        }
    }
    let kinds: Vec<&str> = seen.iter().filter_map(|m| m["type"].as_str()).collect();
    assert!(kinds.contains(&"listening"), "{kinds:?}");
    assert!(
        seen.iter()
            .any(|m| m["type"] == "tool" && m["summary"] == "Asked malac: say hi")
    );
    assert!(audio_frames >= 1);
    talk.send(Frame::Text(json!({ "type": "stop" }).to_string().into()))
        .await
        .unwrap();

    let heard = heard.lock().unwrap().clone();
    let session = &heard[0]["session"];
    assert_eq!(session["type"], "realtime");
    assert_eq!(
        session["audio"]["input"]["transcription"]["model"],
        "whisper-1"
    );
    assert!(
        heard
            .iter()
            .any(|e| e["item"]["type"] == "function_call_output" && e["item"]["call_id"] == "f1")
    );
    let report = heard
        .iter()
        .find(|e| e["item"]["role"] == "system")
        .expect("malac's report reached the voice model");
    assert!(
        report["item"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("hi there")
    );
    assert!(
        script
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| request_text(r).contains("say hi"))
    );

    hub.request_stop("done");
    server.await.unwrap().unwrap();
}
