//! Web UI and daemon API: an HTTP server with a WebSocket message stream.
//!
//! Every API call needs the token (bearer header or `token` query
//! parameter). Requests from this machine must name a loopback host, which
//! blocks DNS rebinding; other machines can connect only when the server
//! listens on all interfaces. The protocol is described in `docs/protocol.md`.

mod fs;
mod settings;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Args;
use futures::{SinkExt, StreamExt};
use malac_core::permission::Approver;
use malac_core::{Runtime, session, update};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

use crate::SessionOptions;
use crate::client::{self, Client};
use crate::daemon::hub::Conn;
use crate::daemon::{Hub, HubOptions, ServeArgs};
use crate::protocol::{Command, Request as ClientRequest, ServerMessage, TabSpec};
use crate::repl::working_dir;
use settings::ApiError;

const INDEX_HTML: &str = include_str!("../../../../web/index.html");
const APP_JS: &str = include_str!("../../../../web/app.js");
const DOM_JS: &str = include_str!("../../../../web/dom.js");
const ICONS_JS: &str = include_str!("../../../../web/icons.js");
const MARKDOWN_JS: &str = include_str!("../../../../web/markdown.js");
const STORE_JS: &str = include_str!("../../../../web/store.js");
const WIDGETS_JS: &str = include_str!("../../../../web/widgets.js");
const TRANSCRIPT_JS: &str = include_str!("../../../../web/transcript.js");
const VIEWS_JS: &str = include_str!("../../../../web/views.js");
const SETTINGS_JS: &str = include_str!("../../../../web/settings.js");
const STYLE_CSS: &str = include_str!("../../../../web/style.css");
const FAVICON_SVG: &str = include_str!("../../../../web/favicon.svg");
const GEIST_WOFF2: &[u8] = include_bytes!("../../../../web/fonts/Geist-Variable.woff2");
const GEIST_MONO_WOFF2: &[u8] = include_bytes!("../../../../web/fonts/GeistMono-Regular.woff2");

#[derive(Args)]
pub struct WebArgs {
    #[command(flatten)]
    serve: ServeArgs,
    /// Open the UI in the default browser.
    #[arg(long)]
    open: bool,
    /// Print one JSON line with the URL and token, then stay attached until
    /// stopped (for editor integrations).
    #[arg(long)]
    json_ready: bool,
    #[command(flatten)]
    session: SessionOptions,
}

/// `malac web`: makes sure the daemon runs and prints or opens its URL.
/// With `--cwd` (as the editor extension passes), the daemon's current
/// workspace gets a tab for that directory and makes it active.
pub async fn run(args: WebArgs) -> Result<()> {
    let file = client::ensure_daemon(&args.serve).await?;
    if args.serve.token.as_ref().is_some_and(|t| *t != file.token) {
        eprintln!("! the daemon was already running with its own token; use the URL below");
    }
    let (client, mut events, snapshot) = Client::connect(&file).await?;
    if args.serve.remote && !snapshot.daemon.remote {
        eprintln!(
            "! the daemon already runs for this machine only; `malac daemon restart --remote` opens it up"
        );
    }
    if let Some(cwd) = &args.session.cwd {
        let spec = TabSpec {
            cwd: working_dir(Some(cwd))?,
            options: args.session.tab_options(),
            reuse: true,
            ..Default::default()
        };
        client.request(Command::TabCreate(spec)).await?;
    }
    let urls = if file.urls.is_empty() {
        vec![file.web_url()]
    } else {
        file.urls.clone()
    };
    if args.json_ready {
        println!(
            "{}",
            json!({ "url": file.url, "token": file.token, "urls": urls })
        );
    } else {
        for url in &urls {
            eprintln!("malac web UI: {url}");
        }
        if snapshot.daemon.remote {
            eprintln!(
                "! listening on all interfaces over plain HTTP: anyone with the URL can control malac"
            );
        }
    }
    if args.open {
        open_browser(&urls[0]);
    }
    if args.json_ready {
        // Editors keep this process while their panel is open; as a
        // connected client it also keeps the daemon from idling out.
        tokio::select! {
            _ = async { while events.recv().await.is_some() {} } => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    Ok(())
}

/// Serves the UI for a terminal session's runtime. Falls back to a free port
/// when `port` is taken. Returns the URLs (with token) and an approver to
/// combine with the terminal's.
pub async fn spawn_for(
    runtime: Arc<Runtime>,
    host: &str,
    port: u16,
) -> Result<(Vec<String>, Arc<dyn Approver>)> {
    let remote = host.parse::<IpAddr>().is_ok_and(|ip| !ip.is_loopback());
    let hub = Hub::attached(
        runtime,
        HubOptions {
            remote,
            idle_minutes: 0,
            browse_root: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
        },
    );
    let approver = hub.approver();
    let server = Server::bind(hub, host, port, true, random_token()).await?;
    let urls = server.urls();
    tokio::spawn(async move {
        if let Err(err) = server.serve().await {
            tracing::error!("web server stopped: {err:#}");
        }
    });
    Ok((urls, approver))
}

/// A bound listener with its routes, ready to serve a [`Hub`].
pub struct Server {
    listener: tokio::net::TcpListener,
    router: Router,
    addr: SocketAddr,
    token: String,
}

impl Server {
    pub async fn bind(
        hub: Arc<Hub>,
        host: &str,
        port: u16,
        any_port_if_taken: bool,
        token: String,
    ) -> Result<Self> {
        let listener = match tokio::net::TcpListener::bind((host, port)).await {
            Ok(listener) => listener,
            Err(err) if any_port_if_taken && err.kind() == std::io::ErrorKind::AddrInUse => {
                tokio::net::TcpListener::bind((host, 0)).await?
            }
            Err(err) => return Err(err).with_context(|| format!("cannot listen on {host}:{port}")),
        };
        let addr: SocketAddr = listener.local_addr()?;
        let state = Arc::new(AppState {
            hub,
            token: token.clone(),
            url: base_url(addr),
        });
        let api = Router::new()
            .route("/api/state", get(api_state))
            .route(
                "/api/sessions/{id}",
                get(api_session).delete(settings::delete_session),
            )
            .route("/api/ws", get(api_ws))
            .route("/api/daemon", get(api_daemon))
            .route("/api/daemon/stop", post(api_daemon_stop))
            .route("/api/snapshot", get(api_snapshot))
            .route("/api/command", post(api_command))
            .route("/api/workspaces", get(api_workspaces))
            .route("/api/jobs", get(api_jobs))
            .route("/api/jobs/{id}/output", get(api_job_output))
            .route("/api/fs", get(fs::list))
            .merge(settings::routes())
            .route_layer(middleware::from_fn_with_state(state.clone(), authorize));
        let router = Router::new()
            .route(
                "/",
                get(|| async { asset("index.html", "text/html; charset=utf-8", INDEX_HTML) }),
            )
            .route("/app.js", get(|| async { script("app.js", APP_JS) }))
            .route("/dom.js", get(|| async { script("dom.js", DOM_JS) }))
            .route("/icons.js", get(|| async { script("icons.js", ICONS_JS) }))
            .route(
                "/markdown.js",
                get(|| async { script("markdown.js", MARKDOWN_JS) }),
            )
            .route("/store.js", get(|| async { script("store.js", STORE_JS) }))
            .route(
                "/widgets.js",
                get(|| async { script("widgets.js", WIDGETS_JS) }),
            )
            .route(
                "/transcript.js",
                get(|| async { script("transcript.js", TRANSCRIPT_JS) }),
            )
            .route("/views.js", get(|| async { script("views.js", VIEWS_JS) }))
            .route(
                "/settings.js",
                get(|| async { script("settings.js", SETTINGS_JS) }),
            )
            .route(
                "/style.css",
                get(|| async { asset("style.css", "text/css; charset=utf-8", STYLE_CSS) }),
            )
            .route(
                "/favicon.svg",
                get(|| async { asset("favicon.svg", "image/svg+xml", FAVICON_SVG) }),
            )
            .route(
                "/fonts/Geist-Variable.woff2",
                get(|| async { font("fonts/Geist-Variable.woff2", GEIST_WOFF2) }),
            )
            .route(
                "/fonts/GeistMono-Regular.woff2",
                get(|| async { font("fonts/GeistMono-Regular.woff2", GEIST_MONO_WOFF2) }),
            )
            .merge(api)
            .layer(middleware::from_fn(check_host))
            .with_state(state);
        Ok(Self {
            listener,
            router,
            addr,
            token,
        })
    }

    /// The address local clients use, e.g. `http://127.0.0.1:7700`.
    pub fn base_url(&self) -> String {
        base_url(self.addr)
    }

    /// The local URL, plus the LAN URL when listening on all interfaces.
    pub fn urls(&self) -> Vec<String> {
        let mut urls = vec![format!("{}/?token={}", self.base_url(), self.token)];
        if self.addr.ip().is_unspecified() {
            if let Some(ip) = lan_address() {
                urls.push(format!(
                    "http://{ip}:{}/?token={}",
                    self.addr.port(),
                    self.token
                ));
            }
        }
        urls
    }

    /// Serves until the future is dropped. Open WebSockets would hold a
    /// graceful shutdown forever, so callers just stop serving.
    pub async fn serve(self) -> Result<()> {
        axum::serve(
            self.listener,
            self.router
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;
        Ok(())
    }
}

fn base_url(addr: SocketAddr) -> String {
    let host = if addr.ip().is_unspecified() {
        "127.0.0.1".to_owned()
    } else {
        addr.ip().to_string()
    };
    format!("http://{host}:{}", addr.port())
}

/// The address of the interface used for outbound traffic. Connecting a UDP
/// socket sends nothing; it only picks a route.
fn lan_address() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

struct AppState {
    hub: Arc<Hub>,
    token: String,
    url: String,
}

/// Serves an embedded file, or the copy in `$MALAC_WEB_DIR` when set, so the
/// UI can be developed without rebuilding the binary.
fn asset(name: &str, content_type: &'static str, embedded: &'static str) -> Response {
    let body = std::env::var_os("MALAC_WEB_DIR")
        .and_then(|dir| std::fs::read_to_string(PathBuf::from(dir).join(name)).ok())
        .unwrap_or_else(|| embedded.to_owned());
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

fn script(name: &str, embedded: &'static str) -> Response {
    asset(name, "text/javascript; charset=utf-8", embedded)
}

/// Binary counterpart of [`asset`] for the self-hosted fonts.
fn font(name: &str, embedded: &'static [u8]) -> Response {
    let body: std::borrow::Cow<'static, [u8]> = std::env::var_os("MALAC_WEB_DIR")
        .and_then(|dir| std::fs::read(PathBuf::from(dir).join(name)).ok())
        .map_or(embedded.into(), Into::into);
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "max-age=86400"),
        ],
        body,
    )
        .into_response()
}

/// Connections from this machine must name a loopback host, so a web page
/// on another origin cannot reach the API through DNS rebinding.
async fn check_host(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if peer.ip().is_loopback() {
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default();
        let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
        if !matches!(name, "localhost" | "127.0.0.1" | "[::1]") {
            return (StatusCode::FORBIDDEN, "unexpected Host header").into_response();
        }
    }
    next.run(request).await
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

async fn authorize(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TokenQuery>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let presented = bearer.or(query.token.as_deref());
    if presented != Some(state.token.as_str()) {
        return (StatusCode::UNAUTHORIZED, "missing or invalid token").into_response();
    }
    next.run(request).await
}

type Shared = State<Arc<AppState>>;

/// The web UI's original state call: the active tab's runtime and every
/// live agent.
async fn api_state(State(state): Shared) -> Result<Json<Value>, ApiError> {
    let hub = &state.hub;
    let runtime = hub.settings_runtime().await?;
    let models: Vec<String> = runtime
        .config
        .providers
        .iter()
        .flat_map(|(name, p)| p.models.iter().map(move |m| format!("{name}/{m}")))
        .collect();
    Ok(Json(json!({
        "version": update::CURRENT,
        "cwd": runtime.cwd,
        "mode": runtime.policy.mode().as_str(),
        "default_model": runtime.config.model,
        "models": models,
        "agents": hub.live_agents(),
        "sessions": session::list(false, 50).unwrap_or_default(),
        "servers": runtime.servers(),
        "pending": hub.pending_requests(),
        "web": { "remote": hub.options().remote, "reloadable": !hub.is_attached() },
        "update": update::status(),
    })))
}

async fn api_session(Path(id): Path<String>) -> Response {
    match session::load(&id) {
        Ok(t) => {
            Json(json!({ "meta": t.meta, "history": t.history, "usage": t.usage })).into_response()
        }
        Err(err) => (StatusCode::NOT_FOUND, format!("{err:#}")).into_response(),
    }
}

async fn api_daemon(State(state): Shared) -> Json<Value> {
    Json(serde_json::to_value(state.hub.info(&state.url)).unwrap_or_default())
}

#[derive(Deserialize, Default)]
struct StopBody {
    #[serde(default)]
    force: bool,
}

async fn api_daemon_stop(State(state): Shared, body: Option<Json<StopBody>>) -> Response {
    let force = body.is_some_and(|b| b.force);
    let hub = state.hub.clone();
    if hub.is_attached() {
        let error = json!({ "error": "this server belongs to a terminal session" });
        return (StatusCode::BAD_REQUEST, Json(error)).into_response();
    }
    let info = hub.info(&state.url);
    if info.busy() && !force {
        let error = json!({
            "error": format!(
                "{} agents and {} jobs are running; pass force to stop anyway",
                info.agents_running, info.jobs_running
            )
        });
        return (StatusCode::CONFLICT, Json(error)).into_response();
    }
    // Stop after the response is on its way.
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        hub.request_stop("stopped by a client");
    });
    Json(json!({ "stopping": true })).into_response()
}

#[derive(Deserialize)]
struct SnapshotQuery {
    workspace: Option<String>,
}

async fn api_snapshot(
    State(state): Shared,
    Query(query): Query<SnapshotQuery>,
) -> Result<Json<Value>, ApiError> {
    let mut conn = Conn::default();
    let command = Command::Snapshot {
        workspace: query.workspace,
    };
    let reply = state.hub.execute(&mut conn, command).await?;
    Ok(Json(serde_json::to_value(reply).context("encode")?))
}

/// Runs any protocol command; the body is the same JSON as on the WebSocket.
async fn api_command(
    State(state): Shared,
    Json(request): Json<ClientRequest>,
) -> Result<Json<Value>, ApiError> {
    let mut conn = Conn::default();
    let reply = state.hub.execute(&mut conn, request.command).await?;
    let reply = match reply {
        Some(reply) => reply.answering(request.client_ref),
        None => ServerMessage::Ack {
            client_ref: request.client_ref,
            agent: None,
            tab: None,
            attachments: Vec::new(),
        },
    };
    Ok(Json(serde_json::to_value(reply).context("encode")?))
}

async fn api_workspaces(State(state): Shared) -> Json<Value> {
    Json(json!({ "workspaces": state.hub.summaries() }))
}

async fn api_jobs(State(state): Shared) -> Json<Value> {
    Json(json!({ "jobs": state.hub.jobs() }))
}

#[derive(Deserialize)]
struct TailQuery {
    tail: Option<usize>,
}

async fn api_job_output(
    State(state): Shared,
    Path(id): Path<String>,
    Query(query): Query<TailQuery>,
) -> Result<Json<Value>, ApiError> {
    let output = state.hub.job_output(&id, query.tail)?;
    Ok(Json(serde_json::to_value(output).context("encode")?))
}

#[derive(Deserialize)]
struct WsQuery {
    /// The client's malac version, for the version-skew notice.
    version: Option<String>,
}

async fn api_ws(
    State(state): Shared,
    Query(query): Query<WsQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let allowed = match origin {
        None => true,
        Some(origin) => {
            origin == format!("http://{host}")
                || origin.starts_with("vscode-webview://")
                || origin.starts_with("vscode-file://")
        }
    };
    if !allowed {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    ws.on_upgrade(move |socket| client_socket(state, socket, query.version))
}

/// One client: the snapshot first, then every broadcast message and the
/// replies to its own commands, which run one at a time in order.
async fn client_socket(state: Arc<AppState>, socket: WebSocket, version: Option<String>) {
    let hub = state.hub.clone();
    let _client = hub.connected();
    let (mut sink, mut source) = socket.split();
    let mut conn = Conn {
        workspace: None,
        version,
    };
    // Subscribed before the snapshot is taken, so no change falls between.
    let mut outbox = hub.subscribe();
    let snapshot = ServerMessage::Snapshot(Box::new(hub.snapshot(&conn)));
    let Ok(text) = serde_json::to_string(&snapshot) else {
        return;
    };
    if sink.send(WsMessage::Text(text.into())).await.is_err() {
        return;
    }

    let (reply_tx, mut replies) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        loop {
            let text: String = tokio::select! {
                message = outbox.recv() => match message {
                    Ok(text) => text.to_string(),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        json!({ "type": "lagged", "skipped": skipped }).to_string()
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                reply = replies.recv() => match reply {
                    Some(reply) => reply,
                    None => break,
                },
            };
            if sink.send(WsMessage::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let stopped = hub.stopped();
    tokio::pin!(stopped);
    loop {
        let frame = tokio::select! {
            frame = source.next() => frame,
            _ = &mut stopped => break,
        };
        let Some(Ok(frame)) = frame else { break };
        let WsMessage::Text(text) = frame else {
            continue;
        };
        let reply = match serde_json::from_str::<ClientRequest>(text.as_str()) {
            Ok(request) => {
                let reference = request.client_ref.clone();
                match hub.execute(&mut conn, request.command).await {
                    Ok(Some(reply)) => Some(reply.answering(reference)),
                    Ok(None) => reference.map(|r| ServerMessage::Ack {
                        client_ref: Some(r),
                        agent: None,
                        tab: None,
                        attachments: Vec::new(),
                    }),
                    Err(err) => Some(ServerMessage::Error {
                        client_ref: reference,
                        message: format!("{err:#}"),
                    }),
                }
            }
            Err(err) => Some(ServerMessage::Error {
                client_ref: None,
                message: format!("bad message: {err}"),
            }),
        };
        let Some(reply) = reply else { continue };
        let Ok(text) = serde_json::to_string(&reply) else {
            continue;
        };
        if reply_tx.send(text).is_err() {
            break;
        }
    }
    // Let queued replies go out before the socket closes.
    drop(reply_tx);
    let abort = writer.abort_handle();
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), writer).await;
    abort.abort();
}

fn open_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(url).spawn();
}

/// 244 bits from the OS random number generator, hex-encoded.
pub fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
