//! `malac voice` against a daemon and a fake OpenAI-style audio provider,
//! with a WAV file for the microphone and a file for the speaker: the
//! status, dictation with `--json`, and a talk mode conversation that asks
//! the agent something and hears its report.

mod common;

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use base64::Engine;
use common::*;
use futures::StreamExt;
use malac::protocol::{Command, ServerMessage, TabSpec};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const BASE64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
/// Half a second of talk mode's audio.
const HALF_SECOND: usize = 24_000;
/// What the voice says each time: 100 ms of a quiet tone.
const REPLY_BYTES: usize = 4_800;

async fn fake_provider(uploads: Arc<Mutex<Vec<usize>>>) -> u16 {
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { axum::Json(json!({ "data": [{ "id": "whisper-1" }, { "id": "tts-1" }, { "id": "gpt-realtime" }] })) }),
        )
        .route(
            "/v1/audio/transcriptions",
            post(move |body: axum::body::Bytes| {
                let uploads = uploads.clone();
                async move {
                    let text = String::from_utf8_lossy(&body);
                    assert!(text.contains("RIFF") && text.contains("WAVE"), "a WAV upload");
                    uploads.lock().unwrap().push(body.len());
                    axum::Json(json!({ "text": " make the button green " }))
                }
            }),
        )
        .route("/v1/realtime", get(|ws: WebSocketUpgrade| async move { ws.on_upgrade(realtime).into_response() }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// A voice model that, after half a second of audio, asks malac to "say
/// hi", then says that it asked, and what malac reported once it hears it.
async fn realtime(mut socket: WebSocket) {
    let send = |value: Value| WsMessage::Text(value.to_string().into());
    let mut heard = 0;
    let mut asked = false;
    // What it was told since it last spoke: its tool's answer, malac's report.
    let (mut answered, mut reported) = (false, false);
    while let Some(Ok(WsMessage::Text(text))) = socket.next().await {
        let event: Value = serde_json::from_str(text.as_str()).unwrap();
        let replies = match event["type"].as_str().unwrap() {
            "session.update" => vec![json!({ "type": "session.updated" })],
            "input_audio_buffer.append" => {
                heard += BASE64
                    .decode(event["audio"].as_str().unwrap())
                    .unwrap()
                    .len();
                if asked || heard < HALF_SECOND {
                    continue;
                }
                asked = true;
                vec![
                    json!({ "type": "input_audio_buffer.speech_started" }),
                    json!({ "type": "input_audio_buffer.speech_stopped" }),
                    json!({ "type": "conversation.item.input_audio_transcription.completed", "transcript": "make it say hi" }),
                    json!({ "type": "response.created" }),
                    json!({ "type": "response.function_call_arguments.done", "call_id": "f1", "name": "ask_malac", "arguments": "{\"request\":\"say hi\"}" }),
                    json!({ "type": "response.done", "response": { "status": "completed" } }),
                ]
            }
            "conversation.item.create" => {
                match event["item"]["type"].as_str() {
                    Some("function_call_output") => answered = true,
                    _ if event["item"]["role"] == "system" => reported = true,
                    _ => {}
                }
                continue;
            }
            "response.create" => {
                let mut said = Vec::new();
                if std::mem::take(&mut answered) {
                    said.push("Asked malac.");
                }
                if std::mem::take(&mut reported) {
                    said.push("malac says hi there.");
                }
                said.into_iter().flat_map(spoken).collect()
            }
            _ => continue,
        };
        for reply in replies {
            if socket.send(send(reply)).await.is_err() {
                return;
            }
        }
    }
}

/// A reply that says `transcript` over 100 ms of a quiet tone.
fn spoken(transcript: &str) -> Vec<Value> {
    let tone: Vec<u8> = (0..REPLY_BYTES / 2)
        .flat_map(|i| (((i % 48) as i16 - 24) * 200).to_le_bytes())
        .collect();
    vec![
        json!({ "type": "response.created" }),
        json!({ "type": "response.output_audio.delta", "delta": BASE64.encode(tone) }),
        json!({ "type": "response.output_audio_transcript.done", "transcript": transcript }),
        json!({ "type": "response.done", "response": { "status": "completed" } }),
    ]
}

/// A second of a 440 Hz tone, 16-bit stereo at 44.1 kHz.
fn tone_wav() -> Vec<u8> {
    let rate = 44_100u32;
    let samples: Vec<u8> = (0..rate)
        .flat_map(|i| {
            let t = i as f32 / rate as f32;
            let s = ((t * 440.0 * std::f32::consts::TAU).sin() * 8_000.0) as i16;
            [s.to_le_bytes(), s.to_le_bytes()].concat()
        })
        .collect();
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + samples.len() as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * 4).to_le_bytes());
    wav.extend(4u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((samples.len() as u32).to_le_bytes());
    wav.extend(samples);
    wav
}

fn malac(args: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_malac"));
    command
        .args(args)
        .env("MALAC_HOME", home())
        .env("NO_COLOR", "1")
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

#[tokio::test(flavor = "multi_thread")]
async fn the_voice_command_dictates_and_talks() {
    let _guard = exclusive().await;
    let uploads = Arc::new(Mutex::new(Vec::new()));
    let port = fake_provider(uploads.clone()).await;
    let config = home().join("config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    if !text.contains("[providers.audio]") {
        text.push_str(&format!(
            "\n[providers.audio]\nkind = \"openai\"\nbase_url = \"http://127.0.0.1:{port}/v1\"\nmodels = []\n"
        ));
        std::fs::write(&config, text).unwrap();
    }
    let microphone = home().join("microphone.wav");
    std::fs::write(&microphone, tone_wav()).unwrap();
    let speaker = home().join("speaker.pcm");
    let _ = std::fs::remove_file(&speaker);

    let script = Scripted::new(vec![answer("hi there")]);
    let daemon = start_daemon(script.clone(), true).await;
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());
    // The model lists are read in the background at the start.
    let http = reqwest::Client::new();
    http.get(format!("{}/api/voice?refresh=true", file.url))
        .bearer_auth(&file.token)
        .send()
        .await
        .unwrap();

    eprintln!("step: status");
    let output = malac(&["voice", "status", "--json"])
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["dictation"]["model"], "audio/whisper-1");
    assert_eq!(status["talk"]["model"], "audio/gpt-realtime");
    let output = malac(&["voice"]).output().await.unwrap();
    let shown = String::from_utf8_lossy(&output.stdout);
    assert!(
        shown.contains("dictation  ready       audio/whisper-1"),
        "{shown}"
    );
    assert!(shown.contains("speech     off"), "{shown}");

    eprintln!("step: dictation");
    // Recording stops at the limit while stdin stays open.
    let mut child = malac(&["voice", "dictate", "--json", "--max-seconds", "1"])
        .env("MALAC_AUDIO_INPUT", &microphone)
        .spawn()
        .unwrap();
    let stdin = child.stdin.take();
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .expect("dictation finished")
        .unwrap();
    drop(stdin);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "{\"text\":\"make the button green\"}\n",
        "only the answer on stdout"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("transcribing"));
    // A second at 16 kHz, 16-bit, in a WAV inside the upload.
    let sent = uploads.lock().unwrap()[0];
    assert!((32_000..40_000).contains(&sent), "{sent} bytes");

    // A setting the daemon refuses comes back as JSON too.
    let output = malac(&["voice", "dictate", "--json"])
        .env("MALAC_AUDIO_INPUT", home().join("missing.wav"))
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        error["error"].as_str().unwrap().contains("missing.wav"),
        "{error}"
    );

    eprintln!("step: talk");
    let project = project(&[("notes.txt", "hello\n")]);
    let (client, _events, _) = malac::client::Client::connect(&file).await.unwrap();
    let ServerMessage::Ack { tab: Some(tab), .. } = client
        .request(Command::TabCreate(TabSpec {
            cwd: project,
            ..Default::default()
        }))
        .await
        .unwrap()
    else {
        panic!("ack with a tab expected");
    };
    let mut child = malac(&["voice", "talk", "--tab", &tab])
        .env("MALAC_AUDIO_INPUT", &microphone)
        .env("MALAC_AUDIO_OUTPUT", &speaker)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    let heard_back = async {
        while let Some(line) = lines.next_line().await.unwrap() {
            let done = line == "malac: malac says hi there.";
            seen.push(line);
            if done {
                return;
            }
        }
        panic!("talk mode ended early");
    };
    if tokio::time::timeout(Duration::from_secs(20), heard_back)
        .await
        .is_err()
    {
        let mut stderr = String::new();
        let _ =
            tokio::io::AsyncReadExt::read_to_string(&mut child.stderr.take().unwrap(), &mut stderr)
                .await;
        panic!("talk mode timed out after {seen:?}; stderr: {stderr}");
    }
    assert_eq!(
        seen,
        [
            "talk mode · audio/gpt-realtime · voice alloy",
            "you: make it say hi",
            "  Asked malac: say hi",
            "malac: Asked malac.",
            "malac: malac says hi there.",
        ]
    );
    // Typed lines go to the voice model; the end of input ends talk mode.
    stdin.write_all(b"\n").await.unwrap();
    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("talk mode ended")
        .unwrap();
    assert!(status.success());
    assert_eq!(
        std::fs::metadata(&speaker).unwrap().len(),
        2 * REPLY_BYTES as u64,
        "both replies played"
    );
    assert!(
        script
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| request_text(r).contains("say hi"))
    );

    let _ = std::fs::remove_file(&microphone);
    let _ = std::fs::remove_file(&speaker);
    hub.request_stop("done");
    server.await.unwrap().unwrap();
}
