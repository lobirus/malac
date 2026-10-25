//! Serve a [`Handler`] over stdio or WebSocket.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

use crate::protocol::*;

const SUPPORTED_VERSIONS: &[&str] = &[PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];

#[async_trait]
pub trait Handler: Send + Sync + 'static {
    fn info(&self) -> Implementation;

    fn instructions(&self) -> Option<String> {
        None
    }

    async fn list_tools(&self) -> Vec<Tool>;

    async fn call_tool(&self, name: &str, arguments: Value) -> CallToolResult;
}

/// Handles one incoming message, returning the reply for requests.
pub async fn dispatch(handler: &dyn Handler, msg: Message) -> Option<Message> {
    if msg.kind() != Kind::Request {
        return None;
    }
    let id = msg.id.unwrap_or(Value::Null);
    let params = msg.params.unwrap_or(Value::Null);
    let reply = match msg.method.as_deref().unwrap_or_default() {
        "initialize" => {
            let requested = params["protocolVersion"].as_str().unwrap_or_default();
            let version = SUPPORTED_VERSIONS
                .iter()
                .find(|v| **v == requested)
                .unwrap_or(&PROTOCOL_VERSION);
            let result = InitializeResult {
                protocol_version: (*version).into(),
                capabilities: json!({ "tools": {} }),
                server_info: handler.info(),
                instructions: handler.instructions(),
            };
            Message::response(id, serde_json::to_value(result).unwrap_or_default())
        }
        "ping" => Message::response(id, json!({})),
        "tools/list" => Message::response(id, json!({ "tools": handler.list_tools().await })),
        "tools/call" => match params["name"].as_str() {
            Some(name) => {
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                let result = handler.call_tool(name, arguments).await;
                Message::response(id, serde_json::to_value(result).unwrap_or_default())
            }
            None => Message::error_response(id, INVALID_PARAMS, "missing tool name"),
        },
        other => Message::error_response(id, METHOD_NOT_FOUND, format!("unknown method: {other}")),
    };
    Some(reply)
}

/// Serves newline-delimited JSON-RPC on stdin/stdout until stdin closes.
pub async fn serve_stdio(handler: Arc<dyn Handler>) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(msg) = rx.recv().await {
            let mut line = serde_json::to_vec(&msg)?;
            line.push(b'\n');
            stdout.write_all(&line).await?;
            stdout.flush().await?;
        }
        anyhow::Ok(())
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        spawn_dispatch(&handler, &tx, &line);
    }
    drop(tx);
    writer.await??;
    Ok(())
}

/// Serves MCP over WebSocket. When `token` is set, clients must send it as a
/// `token` query parameter or a bearer `Authorization` header.
pub async fn serve_websocket(
    handler: Arc<dyn Handler>,
    addr: SocketAddr,
    token: Option<String>,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(
        "MCP WebSocket server listening on ws://{}",
        listener.local_addr()?
    );
    let token: Arc<Option<String>> = Arc::new(token);
    loop {
        let (stream, peer) = listener.accept().await?;
        let handler = handler.clone();
        let token = token.clone();
        tokio::spawn(async move {
            if let Err(err) = serve_connection(handler, stream, &token).await {
                tracing::debug!("connection from {peer} closed: {err}");
            }
        });
    }
}

async fn serve_connection(
    handler: Arc<dyn Handler>,
    stream: TcpStream,
    token: &Option<String>,
) -> Result<()> {
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        if !origin_allowed(req) {
            return Err(reject(StatusCode::FORBIDDEN, "origin not allowed"));
        }
        if let Some(expected) = token {
            if request_token(req).as_deref() != Some(expected.as_str()) {
                return Err(reject(StatusCode::UNAUTHORIZED, "invalid token"));
            }
        }
        Ok(resp)
    };
    let ws = tokio_tungstenite::accept_hdr_async(stream, check).await?;
    let (mut sink, mut source) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            sink.send(WsMessage::text(serde_json::to_string(&msg)?))
                .await?;
        }
        anyhow::Ok(())
    });

    while let Some(frame) = source.next().await {
        match frame? {
            WsMessage::Text(text) => spawn_dispatch(&handler, &tx, text.as_str()),
            WsMessage::Close(_) => break,
            _ => {}
        }
    }
    drop(tx);
    writer.abort();
    Ok(())
}

fn spawn_dispatch(handler: &Arc<dyn Handler>, tx: &mpsc::UnboundedSender<Message>, raw: &str) {
    let msg = match serde_json::from_str::<Message>(raw) {
        Ok(msg) => msg,
        Err(err) => {
            let _ = tx.send(Message::error_response(
                Value::Null,
                PARSE_ERROR,
                err.to_string(),
            ));
            return;
        }
    };
    let handler = handler.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        if let Some(reply) = dispatch(handler.as_ref(), msg).await {
            let _ = tx.send(reply);
        }
    });
}

/// Browsers always send `Origin`; native clients usually do not. Only local
/// pages and editor webviews may connect, which blocks drive-by websites.
fn origin_allowed(req: &Request) -> bool {
    let Some(origin) = req.headers().get("origin").and_then(|v| v.to_str().ok()) else {
        return true;
    };
    is_local_origin(origin)
}

pub fn is_local_origin(origin: &str) -> bool {
    let host = origin
        .split_once("://")
        .map(|(scheme, rest)| (scheme, rest.split(['/', ':']).next().unwrap_or_default()));
    match host {
        Some(("vscode-webview" | "vscode-file", _)) => true,
        Some(("http" | "https", host)) => matches!(host, "localhost" | "127.0.0.1" | "[::1]"),
        _ => false,
    }
}

fn request_token(req: &Request) -> Option<String> {
    let from_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    from_header.or_else(|| {
        req.uri().query()?.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == "token").then(|| value.to_owned())
        })
    })
}

fn reject(status: StatusCode, reason: &str) -> ErrorResponse {
    let mut resp = ErrorResponse::new(Some(reason.to_owned()));
    *resp.status_mut() = status;
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_origins() {
        assert!(is_local_origin("http://localhost:7700"));
        assert!(is_local_origin("http://127.0.0.1"));
        assert!(is_local_origin("vscode-webview://abc123"));
        assert!(!is_local_origin("https://example.com"));
        assert!(!is_local_origin("http://localhost.evil.com"));
        assert!(!is_local_origin("null"));
    }
}
