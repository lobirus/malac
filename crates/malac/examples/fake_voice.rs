//! A stand-in OpenAI-style voice provider for trying dictation, speech and
//! talk mode without a real model:
//!
//! ```sh
//! cargo run -p malac --example fake_voice -- 7799
//! ```
//!
//! then add it to a scratch malac home's config:
//!
//! ```toml
//! [providers.fake]
//! kind = "openai"
//! base_url = "http://127.0.0.1:7799/v1"
//! models = ["whisper-1", "tts-1", "gpt-realtime"]
//! ```
//!
//! Transcription returns a fixed sentence with the recording's length.
//! Speech is the system voice (`say` or `espeak-ng`), or a beep. The
//! Realtime socket treats every second of audio as an utterance: the first
//! asks malac to say hello, later ones get a spoken reply, and malac's
//! reports are read back.

use std::process::Command;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use base64::Engine;
use futures::StreamExt;
use serde_json::{Value, json};

const BASE64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
/// One second of 24 kHz 16-bit mono audio.
const SECOND: usize = 48_000;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(7799);
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { axum::Json(json!({ "data": [{ "id": "whisper-1" }, { "id": "tts-1" }, { "id": "gpt-realtime" }] })) }),
        )
        .route(
            "/v1/audio/transcriptions",
            post(|body: axum::body::Bytes| async move {
                let seconds = body.len() as f64 / 32_000.0;
                axum::Json(json!({ "text": format!("This is fake dictation of about {seconds:.1} seconds.") }))
            }),
        )
        .route(
            "/v1/audio/speech",
            post(|axum::Json(body): axum::Json<Value>| async move {
                let text = body["input"].as_str().unwrap_or("hello").to_owned();
                let wav = tokio::task::spawn_blocking(move || speech_wav(&text, 22_050)).await.unwrap();
                ([("content-type", "audio/wav")], wav)
            }),
        )
        .route("/v1/realtime", get(|ws: WebSocketUpgrade| async move { ws.on_upgrade(realtime).into_response() }));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("port in use");
    println!("fake voice provider on http://127.0.0.1:{port}/v1");
    axum::serve(listener, app).await.unwrap();
}

async fn realtime(mut socket: WebSocket) {
    let mut heard = 0usize;
    let mut turns = 0;
    while let Some(Ok(message)) = socket.next().await {
        let Message::Text(text) = message else {
            continue;
        };
        let event: Value = serde_json::from_str(text.as_str()).unwrap_or_default();
        let replies = match event["type"].as_str().unwrap_or_default() {
            "session.update" => vec![json!({ "type": "session.updated" })],
            "input_audio_buffer.append" => {
                heard += BASE64
                    .decode(event["audio"].as_str().unwrap_or_default())
                    .map(|a| a.len())
                    .unwrap_or(0);
                if heard < SECOND {
                    continue;
                }
                heard = 0;
                turns += 1;
                let mut replies = vec![
                    json!({ "type": "input_audio_buffer.speech_started" }),
                    json!({ "type": "input_audio_buffer.speech_stopped" }),
                    json!({ "type": "conversation.item.input_audio_transcription.completed", "transcript": format!("(fake utterance {turns})") }),
                ];
                if turns == 1 {
                    replies.extend([
                        json!({ "type": "response.created" }),
                        json!({ "type": "response.function_call_arguments.done", "call_id": "call1", "name": "ask_malac",
                                "arguments": "{\"request\":\"Say hello in one short sentence.\"}" }),
                        json!({ "type": "response.done", "response": { "status": "completed" } }),
                    ]);
                } else {
                    replies.extend(spoken(&format!("I heard you, that was utterance {turns}.")));
                }
                replies
            }
            "response.create" => spoken("Okay, I asked malac. I will tell you when it answers."),
            "conversation.item.create" if event["item"]["role"] == "system" => {
                let note = event["item"]["content"][0]["text"]
                    .as_str()
                    .unwrap_or_default();
                let report: String = note
                    .lines()
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(120)
                    .collect();
                // The daemon asks for a response next; answer that with the report.
                spoken_next(&mut socket, &format!("malac says: {report}")).await;
                continue;
            }
            _ => continue,
        };
        for reply in replies {
            if socket
                .send(Message::Text(reply.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
        }
    }
}

/// Waits for the `response.create` that follows a note, then speaks.
async fn spoken_next(socket: &mut WebSocket, text: &str) {
    while let Some(Ok(Message::Text(frame))) = socket.next().await {
        let event: Value = serde_json::from_str(frame.as_str()).unwrap_or_default();
        if event["type"] == "response.create" {
            for reply in spoken(text) {
                let _ = socket.send(Message::Text(reply.to_string().into())).await;
            }
            return;
        }
    }
}

/// A response that says `text`, in 100 ms audio chunks.
fn spoken(text: &str) -> Vec<Value> {
    let wav = speech_wav(text, 24_000);
    let pcm = wav.get(44..).unwrap_or_default();
    let mut events = vec![json!({ "type": "response.created" })];
    for chunk in pcm.chunks(SECOND / 10) {
        events
            .push(json!({ "type": "response.output_audio.delta", "delta": BASE64.encode(chunk) }));
    }
    events.push(json!({ "type": "response.output_audio_transcript.done", "transcript": text }));
    events.push(json!({ "type": "response.done", "response": { "status": "completed" } }));
    events
}

/// `text` spoken by the system voice as 16-bit mono WAV at `rate`, or a beep.
fn speech_wav(text: &str, rate: u32) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!("fake-voice-{}.wav", std::process::id()));
    let ok = if cfg!(target_os = "macos") {
        Command::new("say")
            .args([
                &format!("--data-format=LEI16@{rate}"),
                "--file-format=WAVE",
                "-o",
            ])
            .arg(&path)
            .arg(text)
            .status()
            .is_ok_and(|s| s.success())
    } else {
        Command::new("espeak-ng")
            .arg("-w")
            .arg(&path)
            .arg(text)
            .status()
            .is_ok_and(|s| s.success())
    };
    let spoken = ok.then(|| std::fs::read(&path).ok()).flatten();
    let _ = std::fs::remove_file(&path);
    // `say` writes extra chunks; keep the plain 44-byte header layout the chunking assumes.
    spoken
        .map(|wav| plain_wav(&wav, rate))
        .unwrap_or_else(|| beep(rate))
}

fn plain_wav(wav: &[u8], rate: u32) -> Vec<u8> {
    let data = wav
        .windows(4)
        .position(|w| w == b"data")
        .map(|at| &wav[at + 8..])
        .unwrap_or_default();
    header(rate, data)
}

fn beep(rate: u32) -> Vec<u8> {
    let samples: Vec<u8> = (0..rate / 2)
        .flat_map(|i| {
            {
                let t = i as f32 / rate as f32;
                ((t * 440.0 * std::f32::consts::TAU).sin() * 8000.0) as i16
            }
            .to_le_bytes()
        })
        .collect();
    header(rate, &samples)
}

fn header(rate: u32, data: &[u8]) -> Vec<u8> {
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend(b"RIFF");
    wav.extend((36 + data.len() as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * 2).to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((data.len() as u32).to_le_bytes());
    wav.extend(data);
    wav
}
