//! Anthropic Messages API, also served by many gateways.

use anyhow::{Result, bail};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::{Value, json};

use super::{Completion, Delta, DeltaSink, Provider, Request, parse_arguments, send};
use crate::message::{Block, Message, Role, StopReason, Usage};

const API_VERSION: &str = "2023-06-01";

pub struct Anthropic {
    http: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
    headers: Vec<(String, String)>,
}

impl Anthropic {
    pub fn new(
        http: reqwest::Client,
        base_url: &str,
        api_key: Option<String>,
        headers: Vec<(String, String)>,
    ) -> Self {
        let base = base_url.trim_end_matches('/');
        let endpoint = if base.ends_with("/v1") {
            format!("{base}/messages")
        } else {
            format!("{base}/v1/messages")
        };
        Self {
            http,
            endpoint,
            api_key,
            headers,
        }
    }
}

#[async_trait]
impl Provider for Anthropic {
    async fn complete(&self, req: &Request<'_>, on_delta: DeltaSink<'_>) -> Result<Completion> {
        let mut builder = self
            .http
            .post(&self.endpoint)
            .header("anthropic-version", API_VERSION)
            .json(&request_body(req));
        if let Some(key) = &self.api_key {
            builder = builder.header("x-api-key", key);
        }
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        let resp = send(builder).await?;
        let mut events = resp.bytes_stream().eventsource();
        let mut acc = Accumulator::default();
        while let Some(event) = events.next().await {
            let event = event?;
            if event.data.is_empty() {
                continue;
            }
            let data: Value = serde_json::from_str(&event.data)?;
            if acc.push(&data, on_delta)? {
                break;
            }
        }
        Ok(acc.finish())
    }
}

fn request_body(req: &Request<'_>) -> Value {
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "system": [{ "type": "text", "text": req.system, "cache_control": { "type": "ephemeral" } }],
        "messages": to_wire_messages(req.messages),
        "stream": true,
    });
    if !req.tools.is_empty() {
        let mut tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.input_schema }))
            .collect();
        if let Some(last) = tools.last_mut() {
            last["cache_control"] = json!({ "type": "ephemeral" });
        }
        body["tools"] = tools.into();
    }
    if let Some(budget) = req.thinking_budget {
        body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
    } else if let Some(temperature) = req.temperature {
        body["temperature"] = temperature.into();
    }
    body
}

fn to_wire_messages(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = messages
        .iter()
        .filter_map(|msg| {
            let content: Vec<Value> = msg.content.iter().filter_map(wire_block).collect();
            if content.is_empty() {
                return None;
            }
            let role = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            Some(json!({ "role": role, "content": content }))
        })
        .collect();
    // Rolling cache breakpoint on the newest turn.
    if let Some(block) = out
        .last_mut()
        .and_then(|m| m["content"].as_array_mut())
        .and_then(|c| c.last_mut())
    {
        block["cache_control"] = json!({ "type": "ephemeral" });
    }
    out
}

fn wire_block(block: &Block) -> Option<Value> {
    Some(match block {
        Block::Text { text } if text.is_empty() => return None,
        Block::Text { text } => json!({ "type": "text", "text": text }),
        Block::Image { media_type, data } => json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data },
        }),
        // Thinking can only be replayed with the signature that proves it unmodified.
        Block::Thinking {
            text,
            signature: Some(signature),
        } => json!({ "type": "thinking", "thinking": text, "signature": signature }),
        Block::Thinking { .. } => return None,
        Block::ToolUse { id, name, input } => {
            let input = if input.is_object() {
                input.clone()
            } else {
                json!({})
            };
            json!({ "type": "tool_use", "id": id, "name": name, "input": input })
        }
        Block::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let mut parts: Vec<Value> = content.iter().filter_map(wire_block).collect();
            if parts.is_empty() {
                parts.push(json!({ "type": "text", "text": "(no output)" }));
            }
            json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": parts,
                "is_error": is_error,
            })
        }
    })
}

enum Partial {
    Text(String),
    Thinking {
        text: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
    Ignored,
}

#[derive(Default)]
struct Accumulator {
    blocks: Vec<Partial>,
    usage: Usage,
    stop: Option<String>,
}

impl Accumulator {
    /// Returns true once the message is complete.
    fn push(&mut self, event: &Value, on_delta: DeltaSink<'_>) -> Result<bool> {
        match event["type"].as_str().unwrap_or_default() {
            "message_start" => self.record_usage(&event["message"]["usage"]),
            "content_block_start" => {
                let block = &event["content_block"];
                let partial = match block["type"].as_str().unwrap_or_default() {
                    "text" => Partial::Text(block["text"].as_str().unwrap_or_default().to_owned()),
                    "thinking" => Partial::Thinking {
                        text: String::new(),
                        signature: String::new(),
                    },
                    "tool_use" => {
                        let name = block["name"].as_str().unwrap_or_default();
                        on_delta(Delta::ToolCall { name });
                        Partial::ToolUse {
                            id: block["id"].as_str().unwrap_or_default().to_owned(),
                            name: name.to_owned(),
                            json: String::new(),
                        }
                    }
                    _ => Partial::Ignored,
                };
                let index = event["index"].as_u64().unwrap_or(self.blocks.len() as u64) as usize;
                if index >= self.blocks.len() {
                    self.blocks.resize_with(index + 1, || Partial::Ignored);
                }
                self.blocks[index] = partial;
            }
            "content_block_delta" => {
                let index = event["index"].as_u64().unwrap_or(0) as usize;
                let delta = &event["delta"];
                match (self.blocks.get_mut(index), delta["type"].as_str()) {
                    (Some(Partial::Text(text)), Some("text_delta")) => {
                        let chunk = delta["text"].as_str().unwrap_or_default();
                        text.push_str(chunk);
                        on_delta(Delta::Text(chunk));
                    }
                    (Some(Partial::Thinking { text, .. }), Some("thinking_delta")) => {
                        let chunk = delta["thinking"].as_str().unwrap_or_default();
                        text.push_str(chunk);
                        on_delta(Delta::Thinking(chunk));
                    }
                    (Some(Partial::Thinking { signature, .. }), Some("signature_delta")) => {
                        signature.push_str(delta["signature"].as_str().unwrap_or_default());
                    }
                    (Some(Partial::ToolUse { json, .. }), Some("input_json_delta")) => {
                        json.push_str(delta["partial_json"].as_str().unwrap_or_default());
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    self.stop = Some(reason.to_owned());
                }
                self.record_usage(&event["usage"]);
            }
            "message_stop" => return Ok(true),
            "error" => bail!(
                "provider stream error: {}",
                event["error"]["message"].as_str().unwrap_or("unknown")
            ),
            _ => {}
        }
        Ok(false)
    }

    fn record_usage(&mut self, usage: &Value) {
        let field = |name: &str| usage[name].as_u64();
        if let Some(n) = field("input_tokens") {
            self.usage.input_tokens = n;
        }
        if let Some(n) = field("output_tokens") {
            self.usage.output_tokens = n;
        }
        if let Some(n) = field("cache_read_input_tokens") {
            self.usage.cache_read_tokens = n;
        }
        if let Some(n) = field("cache_creation_input_tokens") {
            self.usage.cache_write_tokens = n;
        }
    }

    fn finish(self) -> Completion {
        let content: Vec<Block> = self
            .blocks
            .into_iter()
            .filter_map(|partial| match partial {
                Partial::Text(text) if !text.is_empty() => Some(Block::Text { text }),
                Partial::Thinking { text, signature } if !text.is_empty() => {
                    Some(Block::Thinking {
                        text,
                        signature: (!signature.is_empty()).then_some(signature),
                    })
                }
                Partial::ToolUse { id, name, json } => Some(Block::ToolUse {
                    id,
                    name,
                    input: parse_arguments(&json),
                }),
                _ => None,
            })
            .collect();
        let message = Message::assistant(content);
        let stop = match self.stop.as_deref() {
            _ if message.tool_uses().next().is_some() => StopReason::ToolUse,
            Some("end_turn") | Some("stop_sequence") | None => StopReason::EndTurn,
            Some("max_tokens") => StopReason::MaxTokens,
            Some(_) => StopReason::Other,
        };
        Completion {
            message,
            usage: self.usage,
            stop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_stream() {
        let events = [
            json!({"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":5,"output_tokens":1}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Reading"}}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"t1","name":"read","input":{}}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"x\"}"}}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":20}}),
            json!({"type":"message_stop"}),
        ];
        let mut acc = Accumulator::default();
        let mut sink = |_: Delta<'_>| {};
        let mut done = false;
        for event in &events {
            done = acc.push(event, &mut sink).unwrap();
        }
        assert!(done);
        let completion = acc.finish();
        assert_eq!(completion.stop, StopReason::ToolUse);
        assert_eq!(completion.usage.output_tokens, 20);
        assert_eq!(completion.usage.cache_read_tokens, 5);
        assert_eq!(
            completion.message.content[0],
            Block::Thinking {
                text: "hmm".into(),
                signature: Some("sig".into())
            }
        );
        assert_eq!(completion.message.text(), "Reading");
        assert_eq!(
            completion.message.tool_uses().next().unwrap().2,
            &json!({"path":"x"})
        );
    }

    #[test]
    fn unsigned_thinking_is_not_replayed() {
        let wire = to_wire_messages(&[Message::assistant(vec![
            Block::Thinking {
                text: "t".into(),
                signature: None,
            },
            Block::text("hi"),
        ])]);
        assert_eq!(wire[0]["content"].as_array().unwrap().len(), 1);
        assert_eq!(wire[0]["content"][0]["type"], "text");
    }
}
