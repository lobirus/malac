//! Model providers. Each one streams a completion for a provider-neutral
//! [`Request`] and assembles the final [`Message`].

mod anthropic;
mod openai;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

use crate::config::{ProviderConfig, ProviderKind};
use crate::message::{Message, StopReason, Usage};

pub use anthropic::Anthropic;
pub use openai::OpenAi;

#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

pub struct Request<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub thinking_budget: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub enum Delta<'a> {
    Text(&'a str),
    Thinking(&'a str),
    ToolCall { name: &'a str },
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub message: Message,
    pub usage: Usage,
    pub stop: StopReason,
}

pub type DeltaSink<'a> = &'a mut (dyn FnMut(Delta<'_>) + Send);

#[async_trait]
pub trait Provider: Send + Sync {
    async fn complete(&self, req: &Request<'_>, on_delta: DeltaSink<'_>) -> Result<Completion>;

    async fn embed(&self, _model: &str, _input: &[String]) -> Result<Vec<Vec<f32>>> {
        anyhow::bail!("this provider does not support embeddings")
    }
}

pub fn build(config: &ProviderConfig, api_key: Option<String>) -> Arc<dyn Provider> {
    let http = http_client();
    let headers = config.headers.clone().into_iter().collect();
    match config.kind {
        ProviderKind::OpenAi => Arc::new(OpenAi::new(http, &config.base_url, api_key, headers)),
        ProviderKind::Anthropic => {
            Arc::new(Anthropic::new(http, &config.base_url, api_key, headers))
        }
    }
}

/// Model ids the provider reports from its `/models` endpoint.
pub async fn list_models(config: &ProviderConfig, api_key: Option<&str>) -> Result<Vec<String>> {
    let base = config.base_url.trim_end_matches('/');
    let url = match config.kind {
        ProviderKind::OpenAi => format!("{base}/models"),
        ProviderKind::Anthropic if base.ends_with("/v1") => format!("{base}/models"),
        ProviderKind::Anthropic => format!("{base}/v1/models"),
    };
    let mut req = http_client().get(url);
    if let Some(key) = api_key {
        req = match config.kind {
            ProviderKind::OpenAi => req.bearer_auth(key),
            ProviderKind::Anthropic => req
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01"),
        };
    }
    for (name, value) in &config.headers {
        req = req.header(name, value);
    }
    let body: Value = send(req).await?.json().await?;
    Ok(body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_owned))
        .collect())
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(600))
        .user_agent(concat!("malac/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("TLS backend available")
}

/// An HTTP-level failure, carrying enough detail to decide on retries.
#[derive(Debug)]
pub struct ProviderError {
    pub status: Option<u16>,
    pub message: String,
}

impl ProviderError {
    pub fn retryable(&self) -> bool {
        match self.status {
            None => true,
            Some(status) => status == 408 || status == 409 || status == 429 || status >= 500,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(status) => write!(f, "provider returned {status}: {}", self.message),
            None => write!(f, "provider request failed: {}", self.message),
        }
    }
}

impl std::error::Error for ProviderError {}

pub(crate) async fn send(builder: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let resp = builder.send().await.map_err(|err| ProviderError {
        status: None,
        message: err.to_string(),
    })?;
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    let message = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or(body);
    Err(ProviderError {
        status: Some(status),
        message,
    }
    .into())
}

/// Tool arguments arrive as JSON text; keep unparseable input as a string so
/// the tool can report the problem back to the model.
pub(crate) fn parse_arguments(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}
