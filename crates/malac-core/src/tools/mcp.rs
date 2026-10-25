use std::sync::Arc;

use async_trait::async_trait;
use malac_mcp::{Client, Content};
use serde_json::Value;

use super::{MAX_OUTPUT_CHARS, Tool, ToolContext, ToolKind, ToolOutput, truncate_middle};
use crate::message::Block;

/// A tool served by an MCP server.
pub struct McpTool {
    client: Arc<Client>,
    name: String,
    remote_name: String,
    description: String,
    schema: Value,
    kind: ToolKind,
}

impl McpTool {
    pub fn new(server: &str, client: Arc<Client>, tool: malac_mcp::Tool, prefix: bool) -> Self {
        let name = if prefix {
            format!("{}__{}", sanitize(server), sanitize(&tool.name))
        } else {
            sanitize(&tool.name)
        };
        let kind = if tool.is_read_only() {
            ToolKind::Safe
        } else {
            ToolKind::Execute
        };
        Self {
            client,
            name: crate::tools::truncate_chars(&name, 64)
                .trim_end_matches('…')
                .to_owned(),
            description: tool.description,
            schema: tool.input_schema,
            remote_name: tool.name,
            kind,
        }
    }
}

/// Provider APIs only accept `[A-Za-z0-9_-]` in tool names.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    fn kind(&self) -> ToolKind {
        self.kind
    }

    async fn run(&self, input: Value, _ctx: &ToolContext) -> ToolOutput {
        let result = match self.client.call_tool(&self.remote_name, input).await {
            Ok(result) => result,
            Err(err) => return ToolOutput::error(format!("{err:#}")),
        };
        let content = result
            .content
            .into_iter()
            .filter_map(|c| match c {
                Content::Text { text } => {
                    Some(Block::text(truncate_middle(&text, MAX_OUTPUT_CHARS)))
                }
                Content::Image { data, mime_type } => Some(Block::Image {
                    media_type: mime_type,
                    data,
                }),
                Content::Unsupported => None,
            })
            .collect();
        ToolOutput {
            content,
            is_error: result.is_error,
        }
    }
}
