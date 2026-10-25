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

use crate::config::{self, Config, ProviderConfig, ProviderKind};
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

/// What one provider offers, for model pickers.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderModels {
    pub provider: String,
    pub kind: &'static str,
    pub has_key: bool,
    /// Models listed in the configuration.
    pub configured: Vec<String>,
    /// Models the provider reported from its `/models` endpoint.
    pub available: Vec<String>,
    pub error: Option<String>,
}

impl ProviderModels {
    /// Configured models first, then the rest of what the provider reports.
    pub fn all(&self) -> Vec<String> {
        let mut models = self.configured.clone();
        for model in &self.available {
            if !models.contains(model) {
                models.push(model.clone());
            }
        }
        models
    }
}

/// Asks every usable provider for its models, in parallel. Providers with
/// no key are skipped unless they run locally.
pub async fn discover_models(config: &Config) -> Vec<ProviderModels> {
    let lookups = config.providers.iter().map(|(name, provider)| async move {
        let key = config::api_key(name, provider);
        let local = provider.base_url.contains("://localhost")
            || provider.base_url.contains("://127.0.0.1");
        let mut entry = ProviderModels {
            provider: name.clone(),
            kind: provider.kind.as_str(),
            has_key: key.is_some(),
            configured: provider.models.clone(),
            available: Vec::new(),
            error: None,
        };
        if key.is_none() && !local {
            entry.error = Some("no API key".into());
            return entry;
        }
        match tokio::time::timeout(
            Duration::from_secs(8),
            list_models(provider, key.as_deref()),
        )
        .await
        {
            Ok(Ok(models)) => entry.available = models,
            Ok(Err(err)) => entry.error = Some(format!("{err:#}")),
            Err(_) => entry.error = Some("timed out".into()),
        }
        entry
    });
    futures::future::join_all(lookups).await
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
