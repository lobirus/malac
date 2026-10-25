//! Web UI: an HTTP server with a WebSocket event stream.
//!
//! Every API call needs the per-run token (bearer header or `token` query
//! parameter), and requests must name a loopback host, which blocks DNS
//! rebinding when bound to localhost.

mod settings;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use clap::Args;
use futures::{SinkExt, StreamExt};
use malac_core::permission::{Approver, Decision, Mode, PermissionRequest};
use malac_core::{AgentSpec, AgentStatus, Block, Runtime, session};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, oneshot, watch};

use crate::SessionOptions;
use crate::repl::{load_config, working_dir};

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
    /// Bind address (default 127.0.0.1, or 0.0.0.0 with --remote).
    #[arg(long)]
    host: Option<String>,
    /// Accept connections from other machines. The token is still required.
    #[arg(long)]
    remote: bool,
    /// Port to listen on; 0 picks a free one.
    #[arg(long)]
    port: Option<u16>,
    /// Use this token instead of a random one.
    #[arg(long, env = "MALAC_WEB_TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// Open the UI in the default browser.
    #[arg(long)]
    open: bool,
    /// Print one JSON line with the URL and token once listening (for editor integrations).
    #[arg(long)]
    json_ready: bool,
    #[command(flatten)]
    session: SessionOptions,
}

pub async fn run(args: WebArgs) -> Result<()> {
    let cwd = working_dir(args.session.cwd.as_ref())?;
    let config = load_config(&args.session, &cwd)?;
    config.main_model()?;
    let remote = args.remote || config.web.remote;
    let host = match &args.host {
        Some(host) => host.clone(),
        None if remote => "0.0.0.0".into(),
        None => config.web.host.clone(),
    };
    let port = args.port.unwrap_or(config.web.port);
    let runtime = start_runtime(config, cwd.clone()).await?;
    let approver = Arc::new(WebApprover::default());
    runtime.set_approver(approver.clone());
    let reload = Reload {
        options: args.session.clone(),
        cwd,
        lock: tokio::sync::Mutex::new(()),
    };
    let token = args.token.clone().unwrap_or_else(random_token);
    let server = Server::bind(runtime, approver, &host, port, false, token, Some(reload)).await?;

    let urls = server.urls();
    if args.json_ready {
        println!(
            "{}",
            json!({ "url": server.base_url(), "token": server.token, "urls": urls })
        );
    } else {
        for url in &urls {
            eprintln!("malac web UI: {url}");
        }
        if !server.loopback_only() {
            eprintln!(
                "! listening on all interfaces over plain HTTP: anyone with the URL can control malac"
            );
        }
    }
    if args.open {
        open_browser(&urls[0]);
    }
    let state = server.state.clone();
    // Open WebSockets would hold a graceful shutdown forever; just stop serving.
    let result = tokio::select! {
        result = server.serve() => result,
        _ = tokio::signal::ctrl_c() => Ok(()),
    };
    state.runtime().shutdown().await;
    result
}

async fn start_runtime(config: malac_core::Config, cwd: PathBuf) -> Result<Arc<Runtime>> {
    let runtime = Runtime::new(config, cwd)?;
    runtime.start_update_check();
    for server in runtime.connect_mcp().await {
        if let Some(err) = server.error {
            eprintln!(
                "! MCP server `{}` unavailable: {}",
                server.name,
                err.lines().next().unwrap_or_default()
            );
        }
    }
    Ok(runtime)
}

/// Serves the UI for a terminal session's runtime. Falls back to a free port
/// when `port` is taken. Returns the URLs (with token) and an approver to
/// combine with the terminal's.
pub async fn spawn_for(
    runtime: Arc<Runtime>,
    host: &str,
    port: u16,
) -> Result<(Vec<String>, Arc<dyn Approver>)> {
    let approver = Arc::new(WebApprover::default());
    let server = Server::bind(
        runtime,
        approver.clone(),
        host,
        port,
        true,
        random_token(),
        None,
    )
    .await?;
    let urls = server.urls();
    tokio::spawn(async move {
        if let Err(err) = server.serve().await {
            tracing::error!("web server stopped: {err:#}");
        }
    });
    Ok((urls, approver))
}

struct Server {
    listener: tokio::net::TcpListener,
    router: Router,
    addr: SocketAddr,
    token: String,
    state: Arc<AppState>,
}

impl Server {
    async fn bind(
        runtime: Arc<Runtime>,
        approver: Arc<WebApprover>,
        host: &str,
        port: u16,
        any_port_if_taken: bool,
        token: String,
        reload: Option<Reload>,
    ) -> Result<Self> {
        let listener = match tokio::net::TcpListener::bind((host, port)).await {
            Ok(listener) => listener,
            Err(err) if any_port_if_taken && err.kind() == std::io::ErrorKind::AddrInUse => {
                tokio::net::TcpListener::bind((host, 0)).await?
            }
            Err(err) => return Err(err).with_context(|| format!("cannot listen on {host}:{port}")),
        };
        let addr: SocketAddr = listener.local_addr()?;
        let (runtime, _) = watch::channel(runtime);
        let (notices, _) = broadcast::channel(64);
        let state = Arc::new(AppState {
            runtime,
            notices,
            approver,
            token: token.clone(),
            loopback_only: addr.ip().is_loopback(),
            reload,
        });
        let api = Router::new()
            .route("/api/state", get(api_state))
            .route(
                "/api/sessions/{id}",
                get(api_session).delete(settings::delete_session),
            )
            .route("/api/ws", get(api_ws))
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
            .layer(middleware::from_fn_with_state(state.clone(), check_host))
            .with_state(state.clone());
        Ok(Self {
            listener,
            router,
            addr,
            token,
            state,
        })
    }

    fn loopback_only(&self) -> bool {
        self.state.loopback_only
    }

    fn base_url(&self) -> String {
        let host = if self.addr.ip().is_unspecified() {
            "127.0.0.1".to_owned()
        } else {
            self.addr.ip().to_string()
        };
        format!("http://{host}:{}", self.addr.port())
    }

    /// The local URL, plus the LAN URL when listening on all interfaces.
    fn urls(&self) -> Vec<String> {
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

    async fn serve(self) -> Result<()> {
        axum::serve(self.listener, self.router).await?;
        Ok(())
    }
}

/// The address of the interface used for outbound traffic. Connecting a UDP
/// socket sends nothing; it only picks a route.
fn lan_address() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// How to rebuild the runtime after a settings change.
struct Reload {
    options: SessionOptions,
    cwd: PathBuf,
    lock: tokio::sync::Mutex<()>,
}

struct AppState {
    runtime: watch::Sender<Arc<Runtime>>,
    /// Server-wide messages for every connected page (mode changes, reloads).
    notices: broadcast::Sender<Value>,
    approver: Arc<WebApprover>,
    token: String,
    loopback_only: bool,
    /// `None` when the runtime belongs to a terminal session, which owns it.
    reload: Option<Reload>,
}

impl AppState {
    fn runtime(&self) -> Arc<Runtime> {
        self.runtime.borrow().clone()
    }

    fn notify(&self, message: Value) {
        let _ = self.notices.send(message);
    }

    /// Rebuilds the runtime from the configuration files. Saved sessions
    /// carry over; running agents must finish first.
    async fn reload(&self) -> Result<()> {
        let Some(reload) = &self.reload else {
            bail!("saved; restart this malac session to apply the changes");
        };
        let _guard = reload.lock.lock().await;
        let old = self.runtime();
        let running = old
            .agents()
            .iter()
            .filter(|a| a.status == AgentStatus::Running)
            .count();
        if running > 0 {
            bail!("saved, but {running} agents are running; stop them or wait, then apply again");
        }
        let config = load_config(&reload.options, &reload.cwd)?;
        let new = start_runtime(config, reload.cwd.clone()).await?;
        new.set_approver(self.approver.clone());
        self.runtime.send_replace(new);
        self.notify(json!({ "type": "reloaded" }));
        old.shutdown().await;
        Ok(())
    }
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

async fn check_host(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    if state.loopback_only {
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

async fn api_state(State(state): State<Arc<AppState>>) -> Json<Value> {
    let runtime = state.runtime();
    let models: Vec<String> = runtime
        .config
        .providers
        .iter()
        .flat_map(|(name, p)| p.models.iter().map(move |m| format!("{name}/{m}")))
        .collect();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "cwd": runtime.cwd,
        "mode": runtime.policy.mode().as_str(),
        "default_model": runtime.config.model,
        "models": models,
        "agents": runtime.agents(),
        "sessions": session::list(false, 50).unwrap_or_default(),
        "servers": runtime.servers(),
        "pending": state.approver.pending(),
        "web": { "remote": !state.loopback_only, "reloadable": state.reload.is_some() },
        "update": malac_core::update::status(),
    }))
}

async fn api_session(Path(id): Path<String>) -> Response {
    match session::load(&id) {
        Ok(t) => {
            Json(json!({ "meta": t.meta, "history": t.history, "usage": t.usage })).into_response()
        }
        Err(err) => (StatusCode::NOT_FOUND, format!("{err:#}")).into_response(),
    }
}

async fn api_ws(
    State(state): State<Arc<AppState>>,
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
    ws.on_upgrade(move |socket| session_socket(state, socket))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Prompt {
        agent: Option<String>,
        text: String,
        #[serde(default)]
        images: Vec<ImageInput>,
        model: Option<String>,
        client_ref: Option<String>,
    },
    Resume {
        session: String,
        client_ref: Option<String>,
    },
    Cancel {
        agent: String,
    },
    Approve {
        request_id: String,
        decision: Decision,
    },
    Mode {
        mode: Mode,
    },
    Model {
        agent: String,
        model: String,
    },
    Compact {
        agent: String,
    },
}

#[derive(Deserialize)]
struct ImageInput {
    media_type: String,
    data: String,
}

async fn session_socket(state: Arc<AppState>, socket: WebSocket) {
    let (mut sink, mut source) = socket.split();
    let mut runtimes = state.runtime.subscribe();
    let mut events = runtimes.borrow_and_update().bus.subscribe();
    let mut notices = state.notices.subscribe();
    let (reply_tx, mut reply_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();

    let writer = tokio::spawn(async move {
        loop {
            let payload = tokio::select! {
                changed = runtimes.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    events = runtimes.borrow_and_update().bus.subscribe();
                    continue;
                }
                notice = notices.recv() => match notice {
                    Ok(notice) => notice,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                event = events.recv() => match event {
                    Ok(envelope) => json!({ "type": "event", "envelope": envelope.as_ref() }),
                    Err(broadcast::error::RecvError::Lagged(n)) => json!({ "type": "lagged", "skipped": n }),
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                reply = reply_rx.recv() => match reply {
                    Some(reply) => reply,
                    None => break,
                },
            };
            if sink
                .send(WsMessage::Text(payload.to_string().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    while let Some(Ok(frame)) = source.next().await {
        let WsMessage::Text(text) = frame else {
            continue;
        };
        let reply = match serde_json::from_str::<ClientMessage>(text.as_str()) {
            Ok(message) => handle(&state, message)
                .unwrap_or_else(|err| json!({ "type": "error", "message": format!("{err:#}") })),
            Err(err) => json!({ "type": "error", "message": format!("bad message: {err}") }),
        };
        if !reply.is_null() && reply_tx.send(reply).is_err() {
            break;
        }
    }
    writer.abort();
}

fn handle(state: &AppState, message: ClientMessage) -> Result<Value> {
    let runtime = state.runtime();
    Ok(match message {
        ClientMessage::Prompt {
            agent,
            text,
            images,
            model,
            client_ref,
        } => {
            let handle = match agent {
                Some(id) => runtime.resume_agent(&id)?,
                None => runtime.spawn_agent(AgentSpec {
                    model,
                    ..Default::default()
                })?,
            };
            let mut content: Vec<Block> = images
                .into_iter()
                .map(|i| Block::Image {
                    media_type: i.media_type,
                    data: i.data,
                })
                .collect();
            content.push(Block::text(text));
            handle.send(content);
            json!({ "type": "ack", "client_ref": client_ref, "agent": handle.id })
        }
        ClientMessage::Resume {
            session,
            client_ref,
        } => {
            let handle = runtime.resume_agent(&session)?;
            json!({ "type": "ack", "client_ref": client_ref, "agent": handle.id })
        }
        ClientMessage::Cancel { agent } => {
            runtime.cancel_tree(&agent);
            Value::Null
        }
        ClientMessage::Approve {
            request_id,
            decision,
        } => {
            state.approver.resolve(&request_id, decision);
            Value::Null
        }
        ClientMessage::Mode { mode } => {
            runtime.policy.set_mode(mode);
            state.notify(json!({ "type": "mode", "mode": mode.as_str() }));
            Value::Null
        }
        ClientMessage::Model { agent, model } => {
            runtime
                .agent(&agent)
                .context("no such agent")?
                .set_model(&model);
            Value::Null
        }
        ClientMessage::Compact { agent } => {
            runtime.agent(&agent).context("no such agent")?.compact();
            Value::Null
        }
    })
}

/// Holds permission requests until a browser answers them.
#[derive(Default)]
pub struct WebApprover {
    waiting: Mutex<HashMap<String, (PermissionRequest, oneshot::Sender<Decision>)>>,
}

impl WebApprover {
    fn pending(&self) -> Vec<PermissionRequest> {
        self.waiting
            .lock()
            .unwrap()
            .values()
            .map(|(r, _)| r.clone())
            .collect()
    }

    fn resolve(&self, id: &str, decision: Decision) {
        if let Some((_, tx)) = self.waiting.lock().unwrap().remove(id) {
            let _ = tx.send(decision);
        }
    }
}

#[async_trait]
impl Approver for WebApprover {
    async fn approve(&self, request: &PermissionRequest) -> Decision {
        let (tx, rx) = oneshot::channel();
        self.waiting
            .lock()
            .unwrap()
            .insert(request.id.clone(), (request.clone(), tx));
        // Forget the request if another approver answers first.
        struct Forget<'a>(&'a WebApprover, &'a str);
        impl Drop for Forget<'_> {
            fn drop(&mut self) {
                self.0.waiting.lock().unwrap().remove(self.1);
            }
        }
        let _forget = Forget(self, &request.id);
        rx.await.unwrap_or(Decision::Deny)
    }
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
fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
