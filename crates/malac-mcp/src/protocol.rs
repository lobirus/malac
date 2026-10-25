//! JSON-RPC 2.0 framing and the subset of MCP types malac needs.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: &str = "2025-06-18";

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// A JSON-RPC message. Requests, notifications and responses share one shape;
/// [`Message::kind`] tells them apart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Request,
    Notification,
    Response,
}

impl Message {
    fn empty() -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: None,
            method: None,
            params: None,
            result: None,
            error: None,
        }
    }

    pub fn request(id: impl Into<Value>, method: &str, params: Option<Value>) -> Self {
        Self {
            id: Some(id.into()),
            method: Some(method.into()),
            params,
            ..Self::empty()
        }
    }

    pub fn notification(method: &str, params: Option<Value>) -> Self {
        Self {
            method: Some(method.into()),
            params,
            ..Self::empty()
        }
    }

    pub fn response(id: Value, result: Value) -> Self {
        Self {
            id: Some(id),
            result: Some(result),
            ..Self::empty()
        }
    }

    pub fn error_response(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            id: Some(id),
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
            ..Self::empty()
        }
    }

    pub fn kind(&self) -> Kind {
        match (&self.method, &self.id) {
            (Some(_), Some(_)) => Kind::Request,
            (Some(_), None) => Kind::Notification,
            (None, _) => Kind::Response,
        }
    }

    /// Converts a response into its result, surfacing JSON-RPC errors.
    pub fn into_result(self) -> anyhow::Result<Value> {
        if let Some(err) = self.error {
            anyhow::bail!("MCP error {}: {}", err.code, err.message);
        }
        Ok(self.result.unwrap_or(Value::Null))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    #[serde(default)]
    pub capabilities: Value,
    pub server_info: Implementation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
}

impl Tool {
    pub fn new(name: &str, description: &str, input_schema: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            annotations: None,
        }
    }

    pub fn read_only(mut self) -> Self {
        self.annotations
            .get_or_insert_with(Default::default)
            .read_only_hint = Some(true);
        self
    }

    pub fn is_read_only(&self) -> bool {
        self.annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Content {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(other)]
    Unsupported,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    pub content: Vec<Content>,
    #[serde(default)]
    pub is_error: bool,
}

impl CallToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text { text: text.into() }],
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text { text: text.into() }],
            is_error: true,
        }
    }

    pub fn with_image(mut self, data: String, mime_type: &str) -> Self {
        self.content.push(Content::Image {
            data,
            mime_type: mime_type.into(),
        });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_messages() {
        let req: Message =
            serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap();
        assert_eq!(req.kind(), Kind::Request);
        let note: Message =
            serde_json::from_value(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .unwrap();
        assert_eq!(note.kind(), Kind::Notification);
        let resp: Message =
            serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"result":{}})).unwrap();
        assert_eq!(resp.kind(), Kind::Response);
    }

    #[test]
    fn unknown_content_types_do_not_fail() {
        let result: CallToolResult = serde_json::from_value(json!({
            "content": [{"type":"text","text":"ok"},{"type":"audio","data":"x","mimeType":"audio/wav"}]
        }))
        .unwrap();
        assert_eq!(result.content.len(), 2);
        assert_eq!(result.content[1], Content::Unsupported);
    }
}
