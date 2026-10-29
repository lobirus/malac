//! Talk mode: a spoken conversation with an end-to-end voice model (the
//! OpenAI Realtime API) that works through malac. The client streams its
//! microphone to the daemon and plays what comes back; the daemon relays
//! both ways and gives the voice model tools to hand work to the tab's
//! agent, check on it, answer its approvals and stop it. When the agent
//! finishes or asks for approval, the voice model hears about it and tells
//! the user.
//!
//! Client frames: binary PCM16 mono 24 kHz both ways, plus JSON text:
//! `{"type":"text","text"}` (typed input), `{"type":"interrupt"}` (stop
//! the reply) and `{"type":"stop"}`. The daemon sends `ready`, `listening`
//! (the user started speaking: stop playback), `thinking`, `transcript`
//! (`role`, `text`, `final`), `tool` (`name`, `summary`), `done`,
//! `error` (`message`) and `closed` (`reason`).

use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::extract::ws::{Message as ClientMessage, WebSocket};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::{SinkExt, StreamExt};
use malac_core::event::{Event, TurnStatus};
use malac_core::permission::Decision;
use malac_core::voice;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};

use super::hub::{Conn, Hub};
use crate::protocol::{Command, ServerMessage};

/// Most of a report the voice model is told.
const REPORT_CHARS: usize = 4000;

pub async fn run(hub: Arc<Hub>, socket: WebSocket, tab: Option<String>, agent: Option<String>) {
    let (mut client_tx, mut client_rx) = socket.split();
    let reason = match session(&hub, &mut client_tx, &mut client_rx, tab, agent).await {
        Ok(reason) => reason,
        Err(err) => {
            let message = json!({ "type": "error", "message": format!("{err:#}") }).to_string();
            let _ = client_tx.send(ClientMessage::Text(message.into())).await;
            format!("{err:#}")
        }
    };
    let closed = json!({ "type": "closed", "reason": reason }).to_string();
    let _ = client_tx.send(ClientMessage::Text(closed.into())).await;
    let _ = client_tx.close().await;
}

type ClientTx = futures::stream::SplitSink<WebSocket, ClientMessage>;
type ClientRx = futures::stream::SplitStream<WebSocket>;

/// Who the voice model works through.
struct Target {
    tab: String,
    agent: Option<String>,
    name: String,
    cwd: String,
}

async fn session(
    hub: &Arc<Hub>,
    client_tx: &mut ClientTx,
    client_rx: &mut ClientRx,
    tab: Option<String>,
    agent: Option<String>,
) -> Result<String> {
    let status = hub.voice().status();
    if !status.talk.enabled {
        bail!("talk mode is off");
    }
    let spec = status
        .talk
        .model
        .context("no model can talk; add a Realtime model such as gpt-realtime")?;
    let mut target = target(hub, tab, agent)?;
    let _talking = hub.voice().talking();
    let runtime = hub.runtime(&target.tab).await?;
    let mut events = runtime.bus.subscribe();

    let config = hub.base_config();
    let endpoint = voice::endpoint(&config, &spec)?;
    let mut request = endpoint.realtime_url().into_client_request()?;
    for (name, value) in endpoint.realtime_headers() {
        request.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(&value)?,
        );
    }
    let (upstream, _) = tokio_tungstenite::connect_async(request)
        .await
        .with_context(|| format!("cannot reach {spec}'s Realtime API"))?;
    let (mut up_tx, mut up_rx) = upstream.split();

    let speaker = status
        .talk
        .voice
        .clone()
        .unwrap_or_else(|| voice::DEFAULT_VOICE.to_owned());
    // A transcription model on the same provider shows what the user said.
    let provider = spec.split('/').next().unwrap_or_default();
    let transcriber = status
        .dictation
        .models
        .iter()
        .find(|m| {
            m.starts_with(&format!("{provider}/"))
                && voice::audio_kind(m, None) == Some(malac_core::config::AudioKind::Transcription)
        })
        .map(|m| {
            m.split_once('/')
                .map(|(_, model)| model.to_owned())
                .unwrap_or_default()
        });
    let instructions = instructions(&target);
    send_up(
        &mut up_tx,
        session_update(&instructions, &speaker, transcriber.as_deref(), false),
    )
    .await?;

    let mut state = Conversation::default();
    loop {
        tokio::select! {
            frame = client_rx.next() => {
                let Some(Ok(frame)) = frame else { break };
                match frame {
                    ClientMessage::Binary(pcm) => {
                        let audio = BASE64.encode(&pcm);
                        send_up(&mut up_tx, json!({ "type": "input_audio_buffer.append", "audio": audio })).await?;
                    }
                    ClientMessage::Text(text) => {
                        let message: Value = serde_json::from_str(text.as_str()).unwrap_or_default();
                        match message["type"].as_str() {
                            Some("stop") => break,
                            Some("interrupt") if state.responding => {
                                send_up(&mut up_tx, json!({ "type": "response.cancel" })).await?;
                            }
                            Some("text") => {
                                let said = message["text"].as_str().unwrap_or_default();
                                state.queue(item("user", said));
                                state.flush(&mut up_tx).await?;
                            }
                            _ => {}
                        }
                    }
                    ClientMessage::Close(_) => break,
                    _ => {}
                }
            }
            message = up_rx.next() => {
                let Some(message) = message else { bail!("the voice model hung up") };
                let text = match message.context("the voice model's connection failed")? {
                    UpstreamMessage::Text(text) => text,
                    UpstreamMessage::Close(frame) => {
                        // A provider that reported an error and hung up has said why already.
                        if let Some(error) = state.relayed.take() {
                            return Ok(error);
                        }
                        let reason = frame.map(|f| f.reason.to_string()).unwrap_or_default();
                        bail!("the voice model closed the conversation{}", if reason.is_empty() { String::new() } else { format!(": {reason}") });
                    }
                    _ => continue,
                };
                let event: Value = serde_json::from_str(text.as_str()).unwrap_or_default();
                let kind = event["type"].as_str().unwrap_or_default();
                match kind {
                    "session.updated" => {
                        if !state.ready {
                            state.ready = true;
                            to_client(client_tx, json!({ "type": "ready", "model": spec, "voice": speaker })).await?;
                        }
                    }
                    "input_audio_buffer.speech_started" => {
                        to_client(client_tx, json!({ "type": "listening" })).await?;
                    }
                    "input_audio_buffer.speech_stopped" => {
                        to_client(client_tx, json!({ "type": "thinking" })).await?;
                    }
                    "response.created" => {
                        state.responding = true;
                        state.relayed = None;
                    }
                    "response.output_audio.delta" | "response.audio.delta" => {
                        if let Ok(pcm) = BASE64.decode(event["delta"].as_str().unwrap_or_default()) {
                            client_tx.send(ClientMessage::Binary(pcm.into())).await?;
                        }
                    }
                    "response.output_audio_transcript.delta" | "response.audio_transcript.delta" => {
                        to_client(client_tx, json!({ "type": "transcript", "role": "assistant", "text": event["delta"], "final": false })).await?;
                    }
                    "response.output_audio_transcript.done" | "response.audio_transcript.done" => {
                        to_client(client_tx, json!({ "type": "transcript", "role": "assistant", "text": event["transcript"], "final": true })).await?;
                    }
                    "conversation.item.input_audio_transcription.completed" => {
                        to_client(client_tx, json!({ "type": "transcript", "role": "user", "text": event["transcript"], "final": true })).await?;
                    }
                    "response.function_call_arguments.done" => {
                        let name = event["name"].as_str().unwrap_or_default().to_owned();
                        let arguments: Value = serde_json::from_str(event["arguments"].as_str().unwrap_or("{}")).unwrap_or_default();
                        let (output, summary) = call_tool(hub, &mut target, &name, &arguments).await;
                        to_client(client_tx, json!({ "type": "tool", "name": name, "summary": summary })).await?;
                        state.queue(json!({
                            "type": "conversation.item.create",
                            "item": { "type": "function_call_output", "call_id": event["call_id"], "output": output }
                        }));
                    }
                    "response.done" => {
                        state.responding = false;
                        let response = &event["response"];
                        if response["status"] == "failed" {
                            let reason = response["status_details"]["error"]["message"].as_str().unwrap_or("the voice model failed");
                            to_client(client_tx, json!({ "type": "error", "message": reason })).await?;
                        }
                        to_client(client_tx, json!({ "type": "done" })).await?;
                        state.flush(&mut up_tx).await?;
                    }
                    "error" => {
                        let message = event["error"]["message"].as_str().unwrap_or("the voice model reported an error");
                        // Servers on the older beta protocol reject the GA session shape.
                        if !state.ready && !state.beta && (message.contains("session.type") || message.contains("Unknown parameter")) {
                            state.beta = true;
                            send_up(&mut up_tx, session_update(&instructions, &speaker, transcriber.as_deref(), true)).await?;
                            continue;
                        }
                        if event["error"]["code"] == "conversation_already_has_active_response" {
                            continue;
                        }
                        // A refused request leaves no reply under way; later notes must still go out.
                        state.responding = false;
                        state.relayed = Some(message.to_owned());
                        to_client(client_tx, json!({ "type": "error", "message": message })).await?;
                    }
                    _ => {}
                }
            }
            envelope = events.recv() => {
                let envelope = match envelope {
                    Ok(envelope) => envelope,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => bail!("the tab was closed"),
                };
                if target.agent.as_deref() != Some(envelope.agent.as_str()) {
                    continue;
                }
                let note = match &envelope.event {
                    Event::TurnFinished { status, text } => Some(match status {
                        TurnStatus::Completed => format!("malac finished and reported:\n{}", cut(text)),
                        TurnStatus::Cancelled => "malac's work was stopped.".to_owned(),
                        TurnStatus::Failed => format!("malac stopped with an error:\n{}", cut(text)),
                    }),
                    Event::PermissionRequested { tool, summary, .. } => Some(format!(
                        "malac asks for approval to use {tool}: {summary}. Ask the user whether to allow it, then call answer_approval."
                    )),
                    _ => None,
                };
                if let Some(note) = note {
                    state.queue(item("system", &note));
                    state.flush(&mut up_tx).await?;
                }
            }
        }
    }
    let _ = up_tx.close().await;
    Ok("talk mode ended".to_owned())
}

/// What the voice model still has to hear, held while it is answering.
#[derive(Default)]
struct Conversation {
    ready: bool,
    beta: bool,
    /// The provider's last error, as passed on to the client.
    relayed: Option<String>,
    responding: bool,
    pending: VecDeque<Value>,
}

impl Conversation {
    fn queue(&mut self, item: Value) {
        self.pending.push_back(item);
    }

    /// Sends what is queued and asks for a reply, unless one is under way.
    async fn flush<S>(&mut self, up: &mut S) -> Result<()>
    where
        S: SinkExt<UpstreamMessage> + Unpin,
        S::Error: std::error::Error + Send + Sync + 'static,
    {
        if self.responding || self.pending.is_empty() {
            return Ok(());
        }
        while let Some(item) = self.pending.pop_front() {
            send_up(up, item).await?;
        }
        send_up(up, json!({ "type": "response.create" })).await?;
        self.responding = true;
        Ok(())
    }
}

fn item(role: &str, text: &str) -> Value {
    json!({
        "type": "conversation.item.create",
        "item": { "type": "message", "role": role, "content": [{ "type": "input_text", "text": text }] }
    })
}

async fn send_up<S>(up: &mut S, message: Value) -> Result<()>
where
    S: SinkExt<UpstreamMessage> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    up.send(UpstreamMessage::Text(message.to_string().into()))
        .await?;
    Ok(())
}

async fn to_client(client: &mut ClientTx, message: Value) -> Result<()> {
    client
        .send(ClientMessage::Text(message.to_string().into()))
        .await?;
    Ok(())
}

fn cut(text: &str) -> String {
    if text.chars().count() <= REPORT_CHARS {
        return text.to_owned();
    }
    let head: String = text.chars().take(REPORT_CHARS).collect();
    format!("{head}…")
}

/// The tab and agent to work through: the ones asked for, else the active
/// tab of the last workspace and its active agent.
fn target(hub: &Hub, tab: Option<String>, agent: Option<String>) -> Result<Target> {
    let snapshot = hub.snapshot(&Conn::default());
    let workspace = snapshot
        .workspace
        .context("open a project in malac first")?;
    let tab_id = tab
        .or(workspace.active_tab.clone())
        .context("open a project in malac first")?;
    let view = workspace.tabs.iter().find(|t| t.id == tab_id);
    Ok(Target {
        agent: agent.or_else(|| view.and_then(|t| t.active_agent.clone())),
        name: view.map(|t| t.name.clone()).unwrap_or_default(),
        cwd: view
            .map(|t| t.cwd.display().to_string())
            .unwrap_or_default(),
        tab: tab_id,
    })
}

fn instructions(target: &Target) -> String {
    format!(
        "You are the voice of malac, a coding agent working in {cwd} (project \"{name}\"). The user talks to you \
         instead of typing. You cannot see or change files yourself: hand every request about the project to \
         malac with ask_malac, in the user's words plus anything they told you, and say in a few words what you \
         asked. While malac works, chat briefly or wait; use malac_status when the user asks how it is going. When \
         malac reports back, tell the user the outcome in one to three spoken sentences: what changed, what failed, \
         what needs them. Skip code, paths and long numbers unless asked. When malac asks for approval, explain \
         what it wants to do and call answer_approval with the user's answer. Use stop_malac when the user wants \
         it to stop. Be brief, warm and natural, and speak the user's language.",
        cwd = target.cwd,
        name = target.name,
    )
}

fn tools() -> Value {
    json!([
        {
            "type": "function",
            "name": "ask_malac",
            "description": "Give malac a task or question about the project. It works in the background and you hear its report when it finishes.",
            "parameters": {
                "type": "object",
                "properties": { "request": { "type": "string", "description": "What the user wants, with every detail they gave" } },
                "required": ["request"]
            }
        },
        {
            "type": "function",
            "name": "malac_status",
            "description": "What malac is doing now and whether it waits for approval.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "type": "function",
            "name": "answer_approval",
            "description": "Allow or deny what malac asked approval for, as the user decided.",
            "parameters": {
                "type": "object",
                "properties": { "decision": { "type": "string", "enum": ["allow", "deny"] } },
                "required": ["decision"]
            }
        },
        {
            "type": "function",
            "name": "stop_malac",
            "description": "Stop what malac is doing now.",
            "parameters": { "type": "object", "properties": {} }
        }
    ])
}

/// The session settings, in the GA shape or the older beta one.
fn session_update(instructions: &str, voice: &str, transcriber: Option<&str>, beta: bool) -> Value {
    let transcription = transcriber.map(|model| json!({ "model": model }));
    let session = if beta {
        let mut session = json!({
            "modalities": ["audio", "text"],
            "instructions": instructions,
            "voice": voice,
            "input_audio_format": "pcm16",
            "output_audio_format": "pcm16",
            "turn_detection": { "type": "server_vad" },
            "tools": tools(),
            "tool_choice": "auto",
        });
        if let Some(transcription) = transcription {
            session["input_audio_transcription"] = transcription;
        }
        session
    } else {
        let mut input = json!({
            "format": { "type": "audio/pcm", "rate": 24000 },
            "turn_detection": { "type": "server_vad" },
        });
        if let Some(transcription) = transcription {
            input["transcription"] = transcription;
        }
        json!({
            "type": "realtime",
            "instructions": instructions,
            "audio": {
                "input": input,
                "output": { "format": { "type": "audio/pcm", "rate": 24000 }, "voice": voice },
            },
            "tools": tools(),
            "tool_choice": "auto",
        })
    };
    json!({ "type": "session.update", "session": session })
}

/// Runs one of the voice model's tools: what it hears back, and a line for the client.
async fn call_tool(
    hub: &Arc<Hub>,
    target: &mut Target,
    name: &str,
    arguments: &Value,
) -> (String, String) {
    let result = match name {
        "ask_malac" => {
            ask(
                hub,
                target,
                arguments["request"].as_str().unwrap_or_default(),
            )
            .await
        }
        "malac_status" => status(hub, target).await,
        "answer_approval" => {
            answer(hub, target, arguments["decision"].as_str() == Some("allow")).await
        }
        "stop_malac" => stop(hub, target).await,
        other => Err(anyhow::anyhow!("no tool named {other}")),
    };
    match result {
        Ok((output, summary)) => (output, summary),
        Err(err) => (format!("Error: {err:#}"), format!("{name} failed: {err:#}")),
    }
}

async fn ask(hub: &Arc<Hub>, target: &mut Target, request: &str) -> Result<(String, String)> {
    if request.trim().is_empty() {
        bail!("the request is empty");
    }
    let mut conn = Conn::default();
    let reply = hub
        .execute(
            &mut conn,
            Command::Prompt {
                tab: Some(target.tab.clone()),
                agent: target.agent.clone(),
                text: request.to_owned(),
                images: Vec::new(),
                attachments: Vec::new(),
                model: None,
            },
        )
        .await?;
    if let Some(ServerMessage::Ack {
        agent: Some(agent), ..
    }) = reply
    {
        target.agent = Some(agent);
    }
    Ok((
        "malac started on it. You will hear its report when it finishes.".to_owned(),
        format!("Asked malac: {request}"),
    ))
}

async fn status(hub: &Arc<Hub>, target: &Target) -> Result<(String, String)> {
    let runtime = hub.runtime(&target.tab).await?;
    let agents = runtime.agents();
    let mut lines: Vec<String> = agents
        .iter()
        .filter(|a| {
            target
                .agent
                .as_deref()
                .is_none_or(|id| a.info.id == id || a.info.parent.as_deref() == Some(id))
        })
        .map(|a| {
            let doing = if a.activity.is_empty() {
                String::new()
            } else {
                format!(": {}", a.activity)
            };
            format!("{} is {:?}{doing}", a.info.label, a.status)
        })
        .collect();
    let waiting = pending(hub, target);
    if !waiting.is_empty() {
        lines.push(format!(
            "Waiting for approval: {}",
            waiting
                .iter()
                .map(|(_, s)| s.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if lines.is_empty() {
        lines.push("malac is idle.".to_owned());
    }
    Ok((lines.join("\n"), "Checked on malac".to_owned()))
}

/// Open approval requests in the target tab, oldest first, as (id, summary).
fn pending(hub: &Hub, target: &Target) -> Vec<(String, String)> {
    hub.snapshot(&Conn::default())
        .pending
        .into_iter()
        .filter(|p| p.tab.as_deref() == Some(target.tab.as_str()))
        .map(|p| {
            (
                p.request.id.clone(),
                format!("{} {}", p.request.tool, p.request.summary),
            )
        })
        .collect()
}

async fn answer(hub: &Arc<Hub>, target: &Target, allow: bool) -> Result<(String, String)> {
    let (id, summary) = pending(hub, target)
        .into_iter()
        .next()
        .context("nothing is waiting for approval")?;
    let decision = if allow {
        Decision::Allow
    } else {
        Decision::Deny
    };
    let mut conn = Conn {
        by: Some("talk mode".to_owned()),
        ..Default::default()
    };
    hub.execute(
        &mut conn,
        Command::Approve {
            request_id: id,
            decision,
        },
    )
    .await?;
    let verb = if allow { "Allowed" } else { "Denied" };
    Ok((format!("{verb}: {summary}"), format!("{verb} {summary}")))
}

async fn stop(hub: &Arc<Hub>, target: &Target) -> Result<(String, String)> {
    let agent = target
        .agent
        .clone()
        .context("malac is not working on anything")?;
    hub.execute(&mut Conn::default(), Command::Cancel { agent })
        .await?;
    Ok(("Stopped.".to_owned(), "Stopped malac".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_come_in_both_shapes() {
        let ga = session_update("be brief", "alloy", Some("gpt-4o-mini-transcribe"), false);
        assert_eq!(ga["session"]["type"], "realtime");
        assert_eq!(ga["session"]["audio"]["output"]["voice"], "alloy");
        assert_eq!(
            ga["session"]["audio"]["input"]["transcription"]["model"],
            "gpt-4o-mini-transcribe"
        );
        let beta = session_update("be brief", "alloy", None, true);
        assert_eq!(beta["session"]["input_audio_format"], "pcm16");
        assert!(beta["session"].get("input_audio_transcription").is_none());
        let names: Vec<&str> = beta["session"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(
            names,
            ["ask_malac", "malac_status", "answer_approval", "stop_malac"]
        );
    }
}
