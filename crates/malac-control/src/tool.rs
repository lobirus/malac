//! Helpers for declaring tools and decoding their arguments.

use anyhow::{Context, Result};
use malac_mcp::Tool;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub fn define(name: &str, description: &str, properties: Value, required: &[&str]) -> Tool {
    Tool::new(
        name,
        description,
        json!({ "type": "object", "properties": properties, "required": required }),
    )
}

pub fn parse<T: DeserializeOwned>(tool: &str, args: Value) -> Result<T> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).with_context(|| format!("invalid arguments for {tool}"))
}

/// Cuts `text` to at most `limit` bytes on a line boundary when possible,
/// saying how much was left out.
pub fn truncate(text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end]
        .rfind('\n')
        .filter(|&i| i > limit / 2)
        .unwrap_or(end);
    format!(
        "{}\n[truncated: showing {end} of {} bytes]",
        &text[..end],
        text.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_on_line_boundary() {
        let text = "line one\nline two\nline three".to_owned();
        assert_eq!(truncate(text.clone(), 100), text);
        let cut = truncate(text, 20);
        assert!(cut.starts_with("line one\nline two\n[truncated"), "{cut}");
    }

    #[test]
    fn truncates_inside_multibyte_text() {
        let cut = truncate("ééééé".to_owned(), 3);
        assert!(cut.starts_with("é\n[truncated"), "{cut}");
    }

    #[test]
    fn null_arguments_are_empty() {
        #[derive(Debug, serde::Deserialize)]
        struct Args {
            #[serde(default)]
            flag: bool,
        }
        let args: Args = parse("t", Value::Null).unwrap();
        assert!(!args.flag);
        let err = parse::<Args>("t", json!({"flag": 3})).unwrap_err();
        assert!(format!("{err:#}").starts_with("invalid arguments for t: "));
    }
}
