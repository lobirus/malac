//! MCP client over stdio, WebSocket or streamable HTTP.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

use crate::protocol::*;

const INIT_TIMEOUT: Duration = Duration::from_secs(30);
const STDERR_TAIL: usize = 20;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[async_trait]
trait Transport: Send + Sync {
    async fn request(&self, msg: Message) -> Result<Message>;
    async fn notify(&self, msg: Message) -> Result<()>;

    async fn close(&self) {}
}

pub struct Client {
    transport: Box<dyn Transport>,
    next_id: AtomicU64,
    init: InitializeResult,
}

impl Client {
    /// Spawns `command` and speaks MCP over its stdin/stdout.
    pub async fn stdio(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&Path>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Its own process group, so Ctrl-C in the terminal reaches malac only.
        #[cfg(unix)]
        cmd.process_group(0);
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to start MCP server `{command}`"))?;

        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            let name = command.to_owned();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "mcp", "{name}: {line}");
                    let mut tail = tail.lock().unwrap();
                    if tail.len() == STDERR_TAIL {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            });
        }

        let mut stdin = child.stdin.take().context("child stdin unavailable")?;
        let stdout = child.stdout.take().context("child stdout unavailable")?;
        let (outbound, mut rx) = mpsc::unbounded_channel::<Message>();
        let pending = Pending::new();

        let (close, mut closed) = oneshot::channel::<()>();
        tokio::spawn(async move {
            loop {
                let msg = tokio::select! {
                    msg = rx.recv() => msg,
                    _ = &mut closed => None,
                };
                let Some(msg) = msg else {
                    // Dropping stdin tells the server to exit.
                    return anyhow::Ok(());
                };
                let mut line = serde_json::to_vec(&msg)?;
                line.push(b'\n');
                stdin.write_all(&line).await?;
                stdin.flush().await?;
            }
        });

        let router = pending.clone();
        let replies = outbound.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<Message>(&line) {
                    Ok(msg) => router.route(msg, &replies),
                    Err(_) => tracing::debug!(target: "mcp", "ignoring non-JSON stdout: {line}"),
                }
            }
            router.close();
        });

        let transport = Channel {
            outbound,
            pending,
            process: Some(Process {
                child: tokio::sync::Mutex::new(Some(child)),
                close: Mutex::new(Some(close)),
            }),
        };
        Self::initialize(Box::new(transport)).await.map_err(|err| {
            let tail: Vec<String> = stderr_tail.lock().unwrap().iter().cloned().collect();
            if tail.is_empty() {
                err
            } else {
                anyhow!("{err}\nserver stderr:\n{}", tail.join("\n"))
            }
        })
    }

    /// Connects to an MCP server that accepts JSON-RPC frames over WebSocket.
    pub async fn websocket(url: &str, bearer: Option<&str>) -> Result<Self> {
        let mut request = url.into_client_request()?;
        if let Some(token) = bearer {
            request
                .headers_mut()
                .insert("authorization", format!("Bearer {token}").parse()?);
        }
        let (ws, _) = tokio_tungstenite::connect_async(request)
            .await
            .with_context(|| format!("failed to connect to {url}"))?;
        let (mut sink, mut source) = ws.split();
        let (outbound, mut rx) = mpsc::unbounded_channel::<Message>();
        let pending = Pending::new();

        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                sink.send(WsMessage::text(serde_json::to_string(&msg)?))
                    .await?;
            }
            anyhow::Ok(())
        });

        let router = pending.clone();
        let replies = outbound.clone();
        tokio::spawn(async move {
            while let Some(Ok(frame)) = source.next().await {
                if let WsMessage::Text(text) = frame {
                    if let Ok(msg) = serde_json::from_str::<Message>(text.as_str()) {
                        router.route(msg, &replies);
                    }
                }
            }
            router.close();
        });

        let transport = Channel {
            outbound,
            pending,
            process: None,
        };
        Self::initialize(Box::new(transport)).await
    }

    /// Connects to a streamable-HTTP MCP endpoint.
    pub async fn http(url: &str, headers: &HashMap<String, String>) -> Result<Self> {
        let mut header_map = reqwest::header::HeaderMap::new();
        for (key, value) in headers {
            header_map.insert(
                reqwest::header::HeaderName::from_bytes(key.as_bytes())?,
                value.parse()?,
            );
        }
        let transport = Http {
            client: reqwest::Client::new(),
            url: url.to_owned(),
            headers: header_map,
            session: Mutex::new(None),
        };
        Self::initialize(Box::new(transport)).await
    }

    async fn initialize(transport: Box<dyn Transport>) -> Result<Self> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "malac", "version": env!("CARGO_PKG_VERSION") },
        });
        let reply = tokio::time::timeout(
            INIT_TIMEOUT,
            transport.request(Message::request(0, "initialize", Some(params))),
        )
        .await
        .context("MCP server did not answer initialize in time")??;
        let init: InitializeResult =
            serde_json::from_value(reply.into_result()?).context("malformed initialize result")?;
        transport
            .notify(Message::notification("notifications/initialized", None))
            .await?;
        Ok(Self {
            transport,
            next_id: AtomicU64::new(1),
            init,
        })
    }

    /// Ends the session. A spawned server gets end-of-file on stdin and a few
    /// seconds to clean up before it is killed.
    pub async fn close(&self) {
        self.transport.close().await;
    }

    pub fn server_info(&self) -> &InitializeResult {
        &self.init
    }

    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor.as_ref().map(|c| json!({ "cursor": c }));
            let page = self.call("tools/list", params).await?;
            let batch: Vec<Tool> = serde_json::from_value(page["tools"].clone())
                .context("malformed tools/list result")?;
            tools.extend(batch);
            match page["nextCursor"].as_str() {
                Some(next) if !next.is_empty() => cursor = Some(next.to_owned()),
                _ => return Ok(tools),
            }
        }
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<CallToolResult> {
        let result = self
            .call(
                "tools/call",
                Some(json!({ "name": name, "arguments": arguments })),
            )
            .await?;
        serde_json::from_value(result).context("malformed tools/call result")
    }

    async fn call(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.transport
            .request(Message::request(id, method, params))
            .await?
            .into_result()
    }
}

/// Outstanding requests keyed by their serialized id. `None` once the
/// connection has closed.
#[derive(Clone)]
struct Pending(Arc<Mutex<Option<Waiters>>>);

type Waiters = HashMap<String, oneshot::Sender<Message>>;

impl Pending {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Some(HashMap::new()))))
    }

    fn register(&self, id: &Value) -> Result<oneshot::Receiver<Message>> {
        let (tx, rx) = oneshot::channel();
        self.0
            .lock()
            .unwrap()
            .as_mut()
            .ok_or_else(|| anyhow!("MCP connection closed"))?
            .insert(id.to_string(), tx);
        Ok(rx)
    }

    fn route(&self, msg: Message, replies: &mpsc::UnboundedSender<Message>) {
        match msg.kind() {
            Kind::Response => {
                let key = msg.id.as_ref().map(Value::to_string).unwrap_or_default();
                let waiter = self.0.lock().unwrap().as_mut().and_then(|m| m.remove(&key));
                if let Some(waiter) = waiter {
                    let _ = waiter.send(msg);
                }
            }
            Kind::Request => {
                let id = msg.id.unwrap_or(Value::Null);
                let reply = if msg.method.as_deref() == Some("ping") {
                    Message::response(id, json!({}))
                } else {
                    Message::error_response(id, METHOD_NOT_FOUND, "client does not support this")
                };
                let _ = replies.send(reply);
            }
            Kind::Notification => {}
        }
    }

    /// Drops every waiter so callers see the connection close.
    fn close(&self) {
        self.0.lock().unwrap().take();
    }
}

struct Channel {
    outbound: mpsc::UnboundedSender<Message>,
    pending: Pending,
    process: Option<Process>,
}

struct Process {
    child: tokio::sync::Mutex<Option<Child>>,
    close: Mutex<Option<oneshot::Sender<()>>>,
}

#[async_trait]
impl Transport for Channel {
    async fn request(&self, msg: Message) -> Result<Message> {
        let id = msg.id.clone().context("request without id")?;
        let rx = self.pending.register(&id)?;
        self.outbound
            .send(msg)
            .map_err(|_| anyhow!("MCP connection closed"))?;
        rx.await.map_err(|_| anyhow!("MCP connection closed"))
    }

    async fn notify(&self, msg: Message) -> Result<()> {
        self.outbound
            .send(msg)
            .map_err(|_| anyhow!("MCP connection closed"))
    }

    async fn close(&self) {
        let Some(process) = &self.process else {
            return;
        };
        if let Some(close) = process.close.lock().unwrap().take() {
            let _ = close.send(());
        }
        if let Some(mut child) = process.child.lock().await.take() {
            if tokio::time::timeout(SHUTDOWN_GRACE, child.wait())
                .await
                .is_err()
            {
                let _ = child.kill().await;
            }
        }
    }
}

struct Http {
    client: reqwest::Client,
    url: String,
    headers: reqwest::header::HeaderMap,
    session: Mutex<Option<String>>,
}

impl Http {
    async fn post(&self, msg: &Message) -> Result<reqwest::Response> {
        let mut req = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", PROTOCOL_VERSION)
            .json(msg);
        if let Some(session) = self.session.lock().unwrap().clone() {
            req = req.header("mcp-session-id", session);
        }
        let resp = req.send().await?;
        if let Some(session) = resp.headers().get("mcp-session-id") {
            *self.session.lock().unwrap() = Some(session.to_str()?.to_owned());
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("MCP HTTP error {status}: {body}");
        }
        Ok(resp)
    }
}

#[async_trait]
impl Transport for Http {
    async fn request(&self, msg: Message) -> Result<Message> {
        let id = msg.id.clone();
        let resp = self.post(&msg).await?;
        let is_sse = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        if !is_sse {
            return Ok(resp.json().await?);
        }
        let mut events = resp.bytes_stream().eventsource();
        while let Some(event) = events.next().await {
            let event = event?;
            if let Ok(reply) = serde_json::from_str::<Message>(&event.data) {
                if reply.kind() == Kind::Response && reply.id == id {
                    return Ok(reply);
                }
            }
        }
        bail!("MCP stream ended without a response")
    }

    async fn notify(&self, msg: Message) -> Result<()> {
        self.post(&msg).await.map(|_| ())
    }
}
