//! OpenAI-compatible chat completions (OpenAI, LiteLLM, vLLM, OpenRouter, Together, ...).

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::{Value, json};

use super::{Completion, Delta, DeltaSink, Provider, Request, parse_arguments, send};
use crate::message::{Block, Message, Role, StopReason, Usage};

pub struct OpenAi {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    headers: Vec<(String, String)>,
}

impl OpenAi {
    pub fn new(
        http: reqwest::Client,
        base_url: &str,
        api_key: Option<String>,
        headers: Vec<(String, String)>,
    ) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
            headers,
        }
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        let mut builder = self.http.post(format!("{}{path}", self.base_url));
        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        builder
    }
}

#[async_trait]
impl Provider for OpenAi {
    async fn complete(&self, req: &Request<'_>, on_delta: DeltaSink<'_>) -> Result<Completion> {
        let resp = send(self.post("/chat/completions").json(&request_body(req))).await?;
        let mut events = resp.bytes_stream().eventsource();
        let mut acc = Accumulator::default();
        while let Some(event) = events.next().await {
            let event = event?;
            if event.data == "[DONE]" {
                break;
            }
            let chunk: Value = serde_json::from_str(&event.data)?;
            acc.push(&chunk, on_delta)?;
        }
        Ok(acc.finish())
    }

    async fn embed(&self, model: &str, input: &[String]) -> Result<Vec<Vec<f32>>> {
        let body = json!({ "model": model, "input": input });
        let resp: Value = send(self.post("/embeddings").json(&body))
            .await?
            .json()
            .await?;
        let mut rows: Vec<(u64, Vec<f32>)> = resp["data"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let index = item["index"].as_u64().unwrap_or(0);
                        let vector =
                            serde_json::from_value(item["embedding"].clone()).unwrap_or_default();
                        (index, vector)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if rows.len() != input.len() {
            bail!(
                "embedding response has {} vectors for {} inputs",
                rows.len(),
                input.len()
            );
        }
        rows.sort_by_key(|(index, _)| *index);
        Ok(rows.into_iter().map(|(_, v)| v).collect())
    }
}

fn request_body(req: &Request<'_>) -> Value {
    let mut body = json!({
        "model": req.model,
        "messages": to_wire_messages(req.system, req.messages),
        "max_tokens": req.max_tokens,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();
        body["tools"] = tools.into();
    }
    if let Some(temperature) = req.temperature {
        body["temperature"] = temperature.into();
    }
    body
}

/// Chat completions cannot carry images inside tool messages, so images
/// returned by tools are re-sent in a user message right after the tool replies.
fn to_wire_messages(system: &str, messages: &[Message]) -> Vec<Value> {
    let mut out = vec![json!({ "role": "system", "content": system })];
    for msg in messages {
        match msg.role {
            Role::Assistant => out.push(assistant_message(msg)),
            Role::User => {
                let mut parts = Vec::new();
                let mut tool_images = Vec::new();
                for block in &msg.content {
                    match block {
                        Block::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            let mut text = String::new();
                            for part in content {
                                match part {
                                    Block::Text { text: t } => text.push_str(t),
                                    Block::Image { media_type, data } => {
                                        text.push_str("\n[image attached in the next message]");
                                        tool_images.push(image_part(media_type, data));
                                    }
                                    _ => {}
                                }
                            }
                            out.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": text,
                            }));
                        }
                        Block::Text { text } if !text.is_empty() => {
                            parts.push(json!({ "type": "text", "text": text }));
                        }
                        Block::Image { media_type, data } => {
                            parts.push(image_part(media_type, data))
                        }
                        _ => {}
                    }
                }
                if !tool_images.is_empty() {
                    let mut images = vec![
                        json!({ "type": "text", "text": "Images returned by the tool calls above:" }),
                    ];
                    images.append(&mut tool_images);
                    images.append(&mut parts);
                    parts = images;
                }
                if !parts.is_empty() {
                    out.push(json!({ "role": "user", "content": parts }));
                }
            }
        }
    }
    out
}

fn assistant_message(msg: &Message) -> Value {
    let text = msg.text();
    let mut wire = json!({
        "role": "assistant",
        "content": if text.is_empty() { Value::Null } else { text.into() },
    });
    let calls: Vec<Value> = msg
        .tool_uses()
        .map(|(id, name, input)| {
            let arguments = match input {
                Value::String(raw) => raw.clone(),
                other => other.to_string(),
            };
            json!({
                "id": id,
                "type": "function",
                "function": { "name": name, "arguments": arguments },
            })
        })
        .collect();
    if !calls.is_empty() {
        wire["tool_calls"] = calls.into();
    }
    wire
}

fn image_part(media_type: &str, data: &str) -> Value {
    json!({
        "type": "image_url",
        "image_url": { "url": format!("data:{media_type};base64,{data}") },
    })
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct Accumulator {
    text: String,
    thinking: String,
    calls: BTreeMap<u64, PartialCall>,
    usage: Usage,
    finish: Option<String>,
}

impl Accumulator {
    fn push(&mut self, chunk: &Value, on_delta: DeltaSink<'_>) -> Result<()> {
        if let Some(message) = chunk["error"]["message"].as_str() {
            bail!("provider stream error: {message}");
        }
        if chunk["usage"].is_object() {
            self.usage = parse_usage(&chunk["usage"]);
        }
        let Some(choices) = chunk["choices"].as_array() else {
            return Ok(());
        };
        for choice in choices {
            let delta = &choice["delta"];
            if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
                self.text.push_str(text);
                on_delta(Delta::Text(text));
            }
            for key in ["reasoning_content", "reasoning"] {
                if let Some(text) = delta[key].as_str().filter(|t| !t.is_empty()) {
                    self.thinking.push_str(text);
                    on_delta(Delta::Thinking(text));
                }
            }
            for call in delta["tool_calls"].as_array().into_iter().flatten() {
                let index = call["index"].as_u64().unwrap_or(0);
                let entry = self.calls.entry(index).or_default();
                if let Some(id) = call["id"].as_str().filter(|s| !s.is_empty()) {
                    entry.id = id.to_owned();
                }
                if let Some(name) = call["function"]["name"].as_str().filter(|s| !s.is_empty()) {
                    if entry.name.is_empty() {
                        entry.name = name.to_owned();
                        on_delta(Delta::ToolCall { name });
                    }
                }
                if let Some(args) = call["function"]["arguments"].as_str() {
                    entry.arguments.push_str(args);
                }
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                self.finish = Some(reason.to_owned());
            }
        }
        Ok(())
    }

    fn finish(self) -> Completion {
        let mut content = Vec::new();
        if !self.thinking.is_empty() {
            content.push(Block::Thinking {
                text: self.thinking,
                signature: None,
            });
        }
        if !self.text.is_empty() {
            content.push(Block::Text { text: self.text });
        }
        let has_calls = !self.calls.is_empty();
        for (index, call) in self.calls {
            content.push(Block::ToolUse {
                id: if call.id.is_empty() {
                    format!("call_{index}")
                } else {
                    call.id
                },
                name: call.name,
                input: parse_arguments(&call.arguments),
            });
        }
        let stop = match self.finish.as_deref() {
            _ if has_calls => StopReason::ToolUse,
            Some("length") => StopReason::MaxTokens,
            Some("stop") | None => StopReason::EndTurn,
            Some(_) => StopReason::Other,
        };
        Completion {
            message: Message::assistant(content),
            usage: self.usage,
            stop,
        }
    }
}

fn parse_usage(usage: &Value) -> Usage {
    let prompt = usage["prompt_tokens"].as_u64().unwrap_or(0);
    let cached = usage["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .unwrap_or(0);
    Usage {
        input_tokens: prompt.saturating_sub(cached),
        output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: cached,
        cache_write_tokens: 0,
        cost: usage["cost"].as_f64().unwrap_or(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(chunks: &[Value]) -> (Completion, Vec<String>) {
        let mut acc = Accumulator::default();
        let mut seen = Vec::new();
        let mut sink = |d: Delta<'_>| {
            if let Delta::Text(t) = d {
                seen.push(t.to_owned());
            }
        };
        for chunk in chunks {
            acc.push(chunk, &mut sink).unwrap();
        }
        (acc.finish(), seen)
    }

    #[test]
    fn assembles_streamed_tool_calls() {
        let (done, _) = feed(&[
            json!({"choices":[{"delta":{"reasoning_content":"need the file"}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":""}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a.txt\"}"}}]}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":40},"cost":0.001}}),
        ]);
        assert_eq!(done.stop, StopReason::ToolUse);
        assert_eq!(done.usage.input_tokens, 60);
        assert_eq!(done.usage.cache_read_tokens, 40);
        let calls: Vec<_> = done.message.tool_uses().collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "call_1");
        assert_eq!(calls[0].2, &json!({"path":"a.txt"}));
        assert!(matches!(done.message.content[0], Block::Thinking { .. }));
    }

    #[test]
    fn streams_text() {
        let (done, seen) = feed(&[
            json!({"choices":[{"delta":{"content":"Hel"}}]}),
            json!({"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}]}),
        ]);
        assert_eq!(seen, vec!["Hel", "lo"]);
        assert_eq!(done.message.text(), "Hello");
        assert_eq!(done.stop, StopReason::EndTurn);
    }

    #[test]
    fn tool_images_follow_tool_messages() {
        let messages = vec![
            Message::assistant(vec![Block::ToolUse {
                id: "c1".into(),
                name: "screenshot".into(),
                input: json!({}),
            }]),
            Message::user(vec![Block::ToolResult {
                tool_use_id: "c1".into(),
                content: vec![
                    Block::text("1280x800"),
                    Block::Image {
                        media_type: "image/png".into(),
                        data: "AAAA".into(),
                    },
                ],
                is_error: false,
            }]),
        ];
        let wire = to_wire_messages("sys", &messages);
        assert_eq!(wire[1]["tool_calls"][0]["function"]["arguments"], "{}");
        assert_eq!(wire[2]["role"], "tool");
        assert_eq!(wire[3]["role"], "user");
        assert_eq!(wire[3]["content"][1]["type"], "image_url");
    }
}
