//! Web UI: a local HTTP server with a WebSocket event stream.
//!
//! Every API call needs the per-run token (bearer header or `token` query
//! parameter), and requests must name a loopback host, which blocks DNS
//! rebinding when bound to localhost.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
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
use malac_core::{AgentSpec, Block, Runtime, session};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, oneshot};

use crate::SessionOptions;
use crate::repl::{load_config, working_dir};

const INDEX_HTML: &str = include_str!("../../../web/index.html");
const APP_JS: &str = include_str!("../../../web/app.js");
const STYLE_CSS: &str = include_str!("../../../web/style.css");

#[derive(Args)]
pub struct WebArgs {
    #[arg(long)]
    host: Option<String>,
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
    let host = args.host.clone().unwrap_or_else(|| config.web.host.clone());
    let port = args.port.unwrap_or(config.web.port);
    let runtime = Runtime::new(config, cwd)?;
    for server in runtime.connect_mcp().await {
        if let Some(err) = server.error {
            eprintln!(
                "! MCP server `{}` unavailable: {}",
                server.name,
                err.lines().next().unwrap_or_default()
            );
        }
    }
    let approver = Arc::new(WebApprover::default());
    runtime.set_approver(approver.clone());
    let token = args.token.clone().unwrap_or_else(random_token);
    let server = Server::bind(runtime.clone(), approver, &host, port, token).await?;

    if args.json_ready {
        println!("{}", json!({ "url": server.url, "token": server.token }));
    } else {
        eprintln!("malac web UI: {}", server.url_with_token());
    }
    if args.open {
        open_browser(&server.url_with_token());
    }
    // Open WebSockets would hold a graceful shutdown forever; just stop serving.
    let result = tokio::select! {
        result = server.serve() => result,
        _ = tokio::signal::ctrl_c() => Ok(()),
    };
    runtime.shutdown().await;
    result
}

/// Serves the UI for an existing runtime on a free local port. Returns the
/// URL (with token) and an approver to combine with the terminal's.
pub async fn spawn_for(
    runtime: Arc<Runtime>,
    port: Option<u16>,
) -> Result<(String, Arc<dyn Approver>)> {
    let approver = Arc::new(WebApprover::default());
    let server = Server::bind(
        runtime,
        approver.clone(),
        "127.0.0.1",
        port.unwrap_or(0),
        random_token(),
    )
    .await?;
    let url = server.url_with_token();
    tokio::spawn(async move {
        if let Err(err) = server.serve().await {
            tracing::error!("web server stopped: {err:#}");
        }
    });
    Ok((url, approver))
}

struct Server {
    listener: tokio::net::TcpListener,
    router: Router,
    url: String,
    token: String,
}

impl Server {
    async fn bind(
        runtime: Arc<Runtime>,
        approver: Arc<WebApprover>,
        host: &str,
        port: u16,
        token: String,
    ) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind((host, port))
            .await
            .with_context(|| format!("cannot listen on {host}:{port}"))?;
        let addr: SocketAddr = listener.local_addr()?;
        let state = Arc::new(AppState {
            runtime,
            approver,
            token: token.clone(),
            loopback_only: addr.ip().is_loopback(),
        });
        let api = Router::new()
            .route("/api/state", get(api_state))
            .route("/api/sessions/{id}", get(api_session))
            .route("/api/ws", get(api_ws))
            .route_layer(middleware::from_fn_with_state(state.clone(), authorize));
        let router = Router::new()
            .route(
                "/",
                get(|| async { asset("text/html; charset=utf-8", INDEX_HTML) }),
            )
            .route(
                "/app.js",
                get(|| async { asset("text/javascript; charset=utf-8", APP_JS) }),
            )
            .route(
                "/style.css",
                get(|| async { asset("text/css; charset=utf-8", STYLE_CSS) }),
            )
            .merge(api)
            .layer(middleware::from_fn_with_state(state.clone(), check_host))
            .with_state(state);
        let display_host = if addr.ip().is_unspecified() {
            "127.0.0.1".to_owned()
        } else {
            addr.ip().to_string()
        };
        Ok(Self {
            listener,
            router,
            url: format!("http://{display_host}:{}", addr.port()),
            token,
        })
    }

    fn url_with_token(&self) -> String {
        format!("{}/?token={}", self.url, self.token)
    }

    async fn serve(self) -> Result<()> {
        axum::serve(self.listener, self.router).await?;
        Ok(())
    }
}

struct AppState {
    runtime: Arc<Runtime>,
    approver: Arc<WebApprover>,
    token: String,
    loopback_only: bool,
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
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
    let runtime = &state.runtime;
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
    let mut events = state.runtime.bus.subscribe();
    let (reply_tx, mut reply_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();

    let writer = tokio::spawn(async move {
        loop {
            let payload = tokio::select! {
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
    let runtime = &state.runtime;
    Ok(match message {
        ClientMessage::Prompt {
            agent,
            text,
            images,
            model,
            client_ref,
        } => {
            let handle = match agent {
                Some(id) => runtime
                    .agent(&id)
                    .with_context(|| format!("no live agent {id}"))?,
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
            json!({ "type": "mode", "mode": mode.as_str() })
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
