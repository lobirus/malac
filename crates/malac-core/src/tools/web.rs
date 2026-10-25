use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolKind, ToolOutput, str_arg, truncate_middle};

const DEFAULT_MAX_CHARS: usize = 40_000;
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

pub struct WebFetch;

#[async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> String {
        "Fetch a URL over HTTP(S). HTML is converted to readable text; JSON and plain text are returned as is. \
         For pages that need JavaScript or a login, use the browser tools instead."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string" },
                "max_chars": { "type": "integer", "description": "Truncate the result (default 40000)" }
            },
            "required": ["url"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Execute
    }

    fn summary(&self, input: &Value) -> String {
        input["url"].as_str().unwrap_or_default().to_owned()
    }

    /// Rules match on the host, e.g. `web_fetch(*.rust-lang.org)`.
    fn permission_keys(&self, input: &Value) -> Vec<String> {
        input["url"]
            .as_str()
            .and_then(|u| reqwest::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(str::to_owned))
            .into_iter()
            .collect()
    }

    async fn run(&self, input: Value, _ctx: &ToolContext) -> ToolOutput {
        fetch(&input).await.into()
    }
}

async fn fetch(input: &Value) -> Result<ToolOutput> {
    let url = reqwest::Url::parse(str_arg(input, "url")?).context("invalid URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("only http and https URLs are supported");
    }
    let max_chars = input["max_chars"]
        .as_u64()
        .map_or(DEFAULT_MAX_CHARS, |n| n as usize);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!(
            "Mozilla/5.0 (compatible; malac/",
            env!("CARGO_PKG_VERSION"),
            ")"
        ))
        .build()?;
    let resp = client.get(url.clone()).send().await?;
    let status = resp.status();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = resp.bytes().await?;
    if bytes.len() > MAX_BODY_BYTES {
        bail!("response is larger than 10 MB");
    }
    let body = if content_type.contains("html") {
        html2text::from_read(bytes.as_ref(), 100).unwrap_or_default()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let text = format!(
        "{status} {url}\n\n{}",
        truncate_middle(body.trim(), max_chars)
    );
    Ok(if status.is_success() {
        ToolOutput::text(text)
    } else {
        ToolOutput::error(text)
    })
}
