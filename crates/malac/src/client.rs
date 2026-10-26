//! A typed client for the malac daemon. It finds the daemon through
//! `~/.malac/daemon.json`, starts it when needed, sends [`Command`]s and
//! receives the [`Snapshot`] followed by every [`ServerMessage`].
//!
//! ```no_run
//! # async fn demo() -> anyhow::Result<()> {
//! use malac::client::Client;
//! use malac::protocol::{Command, ServerMessage};
//!
//! let (client, mut events, snapshot) = Client::connect_or_start(&Default::default()).await?;
//! let workspace = snapshot.workspace.expect("a workspace");
//! let tab = workspace.active_tab.expect("a tab");
//! client
//!     .request(Command::Prompt {
//!         tab: Some(tab),
//!         agent: None,
//!         text: "list the crates".into(),
//!         images: Vec::new(),
//!         attachments: Vec::new(),
//!         model: None,
//!     })
//!     .await?;
//! while let Some(message) = events.recv().await {
//!     if let ServerMessage::Event { envelope, .. } = message {
//!         println!("{:?}", envelope.event);
//!     }
//! }
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use malac_core::config;
use malac_core::event::AgentId;
use malac_core::update;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::daemon::{self, ServeArgs};
use crate::protocol::{
    Command, DaemonInfo, Launch, Request, ServerMessage, Snapshot, TabId, WorkspaceView,
};

/// How long to wait for a daemon started in the background.
const START_TIMEOUT: Duration = Duration::from_secs(20);

/// `~/.malac/daemon.json`: where the running daemon listens. Readable only
/// by its owner, since the token grants full control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonFile {
    /// Base URL on the loopback interface, e.g. `http://127.0.0.1:7700`.
    pub url: String,
    pub token: String,
    pub pid: u32,
    pub version: String,
    /// The UI URL with the token. It works on the daemon's machine only.
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub started: Option<DateTime<Utc>>,
    /// The saved connection this describes, for a daemon on another machine.
    #[serde(skip)]
    pub connection: Option<String>,
}

impl DaemonFile {
    pub fn path() -> PathBuf {
        config::home_dir().join("daemon.json")
    }

    pub fn read() -> Option<Self> {
        let text = std::fs::read_to_string(Self::path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Writes the file with owner-only permissions.
    pub fn write(&self) -> Result<()> {
        let path = Self::path();
        let tmp = path.with_extension("json.tmp");
        config::write_private(&tmp, &serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
    }

    /// Removes the file unless another daemon has replaced it.
    pub fn remove_if_ours(pid: u32) {
        if Self::read().is_some_and(|f| f.pid == pid) {
            let _ = std::fs::remove_file(Self::path());
        }
    }

    /// A daemon elsewhere, reached with a device token.
    pub fn remote(name: &str, url: &str, token: &str) -> Self {
        Self {
            url: url.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            pid: 0,
            version: String::new(),
            urls: vec![url.to_owned()],
            started: None,
            connection: Some(name.to_owned()),
        }
    }

    /// The web UI address: with the master token for the local daemon. A
    /// browser pairs on its own with a daemon elsewhere.
    pub fn web_url(&self) -> String {
        match self.connection {
            Some(_) => format!("{}/", self.url),
            None => format!("{}/?token={}", self.url, self.token),
        }
    }

    /// The WebSocket request. The token goes in a header, so proxy and
    /// tunnel logs never see it in a URL.
    fn ws_request(&self) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let base = self
            .url
            .replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1);
        let mut request =
            format!("{base}/api/ws?version={}", update::CURRENT).into_client_request()?;
        request.headers_mut().insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", self.token).parse()?,
        );
        Ok(request)
    }
}

/// The running daemon, if `daemon.json` names a live one that answers.
pub async fn running() -> Option<DaemonFile> {
    let file = DaemonFile::read()?;
    if !daemon::pid_alive(file.pid) {
        return None;
    }
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    let ok = http
        .get(format!("{}/api/daemon", file.url))
        .bearer_auth(&file.token)
        .send()
        .await
        .is_ok_and(|r| r.status().is_success());
    ok.then_some(file)
}

/// The running daemon, started in the background first if there is none.
pub async fn ensure_daemon(args: &ServeArgs) -> Result<DaemonFile> {
    if let Some(file) = running().await {
        return Ok(file);
    }
    daemon::spawn_detached(args)?;
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(file) = running().await {
            return Ok(file);
        }
    }
    bail!(
        "the malac daemon did not start within {}s; see {}",
        START_TIMEOUT.as_secs(),
        daemon::log_path().display()
    )
}

/// What a `launch` command opened.
#[derive(Debug, Clone)]
pub struct Launched {
    pub workspace: WorkspaceView,
    pub tab: Option<TabId>,
    pub agent: Option<AgentId>,
}

/// Messages from the daemon, after the snapshot; `None` once it disconnects.
pub type Events = mpsc::UnboundedReceiver<ServerMessage>;

type Waiting = Arc<Mutex<HashMap<String, oneshot::Sender<ServerMessage>>>>;

/// A WebSocket connection to the daemon plus its REST API. Replies to
/// [`Client::request`] are routed back to the caller; everything else goes
/// to the [`Events`] receiver.
pub struct Client {
    file: DaemonFile,
    http: reqwest::Client,
    outgoing: mpsc::UnboundedSender<String>,
    waiting: Waiting,
    next_ref: AtomicU64,
    tasks: Vec<JoinHandle<()>>,
}

impl Client {
    /// Connects to the daemon described by `file`.
    pub async fn connect(file: &DaemonFile) -> Result<(Self, Events, Snapshot)> {
        let (socket, _) = tokio_tungstenite::connect_async(file.ws_request()?)
            .await
            .with_context(|| format!("cannot connect to the malac daemon at {}", file.url))?;
        let (mut sink, mut stream) = socket.split();

        let snapshot = loop {
            let frame = stream
                .next()
                .await
                .context("the daemon closed the connection")??;
            if let WsMessage::Text(text) = frame {
                match serde_json::from_str::<ServerMessage>(text.as_str()) {
                    Ok(ServerMessage::Snapshot(snapshot)) => break *snapshot,
                    Ok(other) => bail!("expected a snapshot, got {other:?}"),
                    Err(err) => bail!("unexpected message from the daemon: {err}"),
                }
            }
        };

        let (outgoing, mut queue) = mpsc::unbounded_channel::<String>();
        let writer = tokio::spawn(async move {
            while let Some(text) = queue.recv().await {
                if sink.send(WsMessage::Text(text.into())).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });

        let waiting: Waiting = Arc::default();
        let (events_tx, events) = mpsc::unbounded_channel();
        let routes = waiting.clone();
        let reader = tokio::spawn(async move {
            while let Some(Ok(frame)) = stream.next().await {
                let WsMessage::Text(text) = frame else {
                    continue;
                };
                let message = match serde_json::from_str::<ServerMessage>(text.as_str()) {
                    Ok(message) => message,
                    Err(err) => {
                        // A reply this client cannot read still ends its request.
                        let reference = serde_json::from_str::<serde_json::Value>(text.as_str())
                            .ok()
                            .and_then(|v| v["client_ref"].as_str().map(str::to_owned));
                        match reference {
                            Some(reference) => ServerMessage::Error {
                                client_ref: Some(reference),
                                message: format!("cannot read the daemon's reply: {err}"),
                            },
                            None => {
                                tracing::debug!("skipping an unknown daemon message: {err}");
                                continue;
                            }
                        }
                    }
                };
                let reply_to = message
                    .client_ref()
                    .and_then(|r| routes.lock().unwrap().remove(r));
                match reply_to {
                    Some(tx) => {
                        let _ = tx.send(message);
                    }
                    None => {
                        if events_tx.send(message).is_err() {
                            break;
                        }
                    }
                }
            }
            routes.lock().unwrap().clear();
        });

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;
        let client = Self {
            file: file.clone(),
            http,
            outgoing,
            waiting,
            next_ref: AtomicU64::new(1),
            tasks: vec![writer, reader],
        };
        Ok((client, events, snapshot))
    }

    /// Connects to the running daemon, starting it first if needed.
    pub async fn connect_or_start(args: &ServeArgs) -> Result<(Self, Events, Snapshot)> {
        let file = ensure_daemon(args).await?;
        Self::connect(&file).await
    }

    pub fn file(&self) -> &DaemonFile {
        &self.file
    }

    /// Sends a command without waiting for its reply. Errors arrive as
    /// [`ServerMessage::Error`] events.
    pub fn send(&self, command: Command) -> Result<()> {
        self.write(&Request {
            command,
            client_ref: None,
        })
    }

    /// Sends a command and waits for its reply. A [`ServerMessage::Error`]
    /// reply becomes an `Err`; commands without a reply resolve once the
    /// daemon has handled them.
    pub async fn request(&self, command: Command) -> Result<ServerMessage> {
        let reference = format!("c{}", self.next_ref.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().unwrap().insert(reference.clone(), tx);
        self.write(&Request {
            command,
            client_ref: Some(reference),
        })?;
        match rx
            .await
            .map_err(|_| anyhow!("the daemon closed the connection"))?
        {
            ServerMessage::Error { message, .. } => Err(anyhow!(message)),
            reply => Ok(reply),
        }
    }

    /// Opens a workspace and tab as the command line does.
    pub async fn launch(&self, launch: Launch) -> Result<Launched> {
        match self.request(Command::Launch(launch)).await? {
            ServerMessage::Launched {
                workspace,
                tab,
                agent,
                ..
            } => Ok(Launched {
                workspace: *workspace,
                tab,
                agent,
            }),
            other => bail!("unexpected reply to launch: {other:?}"),
        }
    }

    /// `GET /api/...` with the token, decoded from JSON.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .http
            .get(format!("{}{path}", self.file.url))
            .bearer_auth(&self.file.token)
            .send()
            .await?;
        decode(response).await
    }

    /// `POST /api/...` with a JSON body.
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        let response = self
            .http
            .post(format!("{}{path}", self.file.url))
            .bearer_auth(&self.file.token)
            .json(body)
            .send()
            .await?;
        decode(response).await
    }

    /// `DELETE /api/...`.
    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .http
            .delete(format!("{}{path}", self.file.url))
            .bearer_auth(&self.file.token)
            .send()
            .await?;
        decode(response).await
    }

    /// The daemon's status, as `malac daemon status` shows it.
    pub async fn info(&self) -> Result<DaemonInfo> {
        self.get("/api/daemon").await
    }

    fn write(&self, request: &Request) -> Result<()> {
        let text = serde_json::to_string(request)?;
        self.outgoing
            .send(text)
            .map_err(|_| anyhow!("the daemon closed the connection"))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_owned))
            .unwrap_or(text);
        bail!("{message} ({status})");
    }
    serde_json::from_str(&text).with_context(|| format!("unexpected response: {text}"))
}

/// One-off REST call for commands that need no WebSocket.
pub async fn get<T: DeserializeOwned>(file: &DaemonFile, path: &str) -> Result<T> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = http
        .get(format!("{}{path}", file.url))
        .bearer_auth(&file.token)
        .send()
        .await?;
    decode(response).await
}

/// One-off REST `DELETE`.
pub async fn delete<T: DeserializeOwned>(file: &DaemonFile, path: &str) -> Result<T> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = http
        .delete(format!("{}{path}", file.url))
        .bearer_auth(&file.token)
        .send()
        .await?;
    decode(response).await
}

/// One-off REST `POST`.
pub async fn post<T: DeserializeOwned>(
    file: &DaemonFile,
    path: &str,
    body: &impl Serialize,
) -> Result<T> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;
    let response = http
        .post(format!("{}{path}", file.url))
        .bearer_auth(&file.token)
        .json(body)
        .send()
        .await?;
    decode(response).await
}
