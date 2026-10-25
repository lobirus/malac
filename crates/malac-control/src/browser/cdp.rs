//! A minimal Chrome DevTools Protocol client: one WebSocket to the browser
//! endpoint, with page sessions multiplexed over it
//! (`Target.attachToTarget` with `flatten: true`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};

const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Full-page screenshots arrive as one large message.
const MAX_MESSAGE_SIZE: usize = 256 << 20;
const EVENT_BUFFER: usize = 1024;

#[derive(Debug, Clone)]
pub struct Event {
    pub method: String,
    pub session: Option<String>,
    pub params: Value,
}

type Reply = Result<Value, String>;
/// Outstanding calls by id; `None` once the connection has closed.
type Waiters = Arc<Mutex<Option<HashMap<u64, oneshot::Sender<Reply>>>>>;

pub struct Cdp {
    outbound: mpsc::UnboundedSender<String>,
    waiters: Waiters,
    events: broadcast::Sender<Event>,
    closed: Arc<AtomicBool>,
    next_id: AtomicU64,
}

impl Cdp {
    pub async fn connect(url: &str) -> Result<Self> {
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_SIZE))
            .max_frame_size(Some(MAX_MESSAGE_SIZE));
        let (socket, _) = tokio_tungstenite::connect_async_with_config(url, Some(config), false)
            .await
            .with_context(|| format!("failed to connect to the browser at {url}"))?;
        let (mut sink, mut source) = socket.split();

        let (outbound, mut queue) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(text) = queue.recv().await {
                if sink.send(Message::text(text)).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });

        let waiters: Waiters = Arc::new(Mutex::new(Some(HashMap::new())));
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let closed = Arc::new(AtomicBool::new(false));
        {
            let waiters = waiters.clone();
            let events = events.clone();
            let closed = closed.clone();
            tokio::spawn(async move {
                while let Some(Ok(frame)) = source.next().await {
                    if let Message::Text(text) = frame {
                        route(text.as_str(), &waiters, &events);
                    }
                }
                closed.store(true, Ordering::Release);
                waiters.lock().unwrap().take();
            });
        }

        Ok(Self {
            outbound,
            waiters,
            events,
            closed,
            next_id: AtomicU64::new(1),
        })
    }

    /// Calls a browser-level method.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.request(None, method, params).await
    }

    /// Calls a method in an attached target's session.
    pub async fn call_in(&self, session: &str, method: &str, params: Value) -> Result<Value> {
        self.request(Some(session), method, params).await
    }

    /// Events from every session, starting now.
    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    async fn request(&self, session: Option<&str>, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = session.into();
        }
        let (tx, rx) = oneshot::channel();
        self.waiters
            .lock()
            .unwrap()
            .as_mut()
            .ok_or_else(connection_closed)?
            .insert(id, tx);
        self.outbound
            .send(message.to_string())
            .map_err(|_| connection_closed())?;
        let reply = match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => return Err(connection_closed()),
            Err(_) => {
                if let Some(waiters) = self.waiters.lock().unwrap().as_mut() {
                    waiters.remove(&id);
                }
                bail!("{method} timed out after {}s", CALL_TIMEOUT.as_secs());
            }
        };
        reply.map_err(|message| anyhow!("{method} failed: {message}"))
    }
}

fn connection_closed() -> anyhow::Error {
    anyhow!("the connection to the browser closed")
}

fn route(text: &str, waiters: &Waiters, events: &broadcast::Sender<Event>) {
    let Ok(mut message) = serde_json::from_str::<Value>(text) else {
        tracing::debug!("ignoring malformed CDP message");
        return;
    };
    if let Some(id) = message["id"].as_u64() {
        let waiter = waiters.lock().unwrap().as_mut().and_then(|w| w.remove(&id));
        if let Some(waiter) = waiter {
            let reply = match message.get("error") {
                Some(error) => Err(error_text(error)),
                None => Ok(message["result"].take()),
            };
            let _ = waiter.send(reply);
        }
    } else if let Some(method) = message["method"].as_str().map(str::to_owned) {
        let _ = events.send(Event {
            method,
            session: message["sessionId"].as_str().map(str::to_owned),
            params: message["params"].take(),
        });
    }
}

fn error_text(error: &Value) -> String {
    let message = error["message"].as_str().unwrap_or("unknown error");
    match error["data"].as_str() {
        Some(data) if !data.is_empty() => format!("{message} ({data})"),
        _ => message.to_owned(),
    }
}

/// Waits for an event matching `matches`, giving up after `timeout` or when
/// the connection closes.
pub async fn wait_for(
    events: &mut broadcast::Receiver<Event>,
    timeout: Duration,
    mut matches: impl FnMut(&Event) -> bool,
) -> Option<Event> {
    let next = async {
        loop {
            match events.recv().await {
                Ok(event) if matches(&event) => return Some(event),
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    };
    tokio::time::timeout(timeout, next).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_replies_and_events() {
        let waiters: Waiters = Arc::new(Mutex::new(Some(HashMap::new())));
        let (events, mut rx) = broadcast::channel(4);
        let (ok_tx, mut ok_rx) = oneshot::channel();
        let (err_tx, mut err_rx) = oneshot::channel();
        {
            let mut map = waiters.lock().unwrap();
            let map = map.as_mut().unwrap();
            map.insert(1, ok_tx);
            map.insert(2, err_tx);
        }

        route(r#"{"id":1,"result":{"value":42}}"#, &waiters, &events);
        route(
            r#"{"id":2,"error":{"code":-32000,"message":"No node","data":"id 7"}}"#,
            &waiters,
            &events,
        );
        route(
            r#"{"method":"Page.loadEventFired","sessionId":"S1","params":{"timestamp":1}}"#,
            &waiters,
            &events,
        );

        assert_eq!(ok_rx.try_recv().unwrap(), Ok(json!({"value": 42})));
        assert_eq!(err_rx.try_recv().unwrap(), Err("No node (id 7)".to_owned()));
        let event = rx.try_recv().unwrap();
        assert_eq!(event.method, "Page.loadEventFired");
        assert_eq!(event.session.as_deref(), Some("S1"));
        assert_eq!(event.params["timestamp"], 1);
    }
}
