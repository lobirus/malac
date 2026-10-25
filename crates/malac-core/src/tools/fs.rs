use std::path::Path;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolKind, ToolOutput, str_arg};
use crate::message::Block;

const DEFAULT_LINE_LIMIT: usize = 2000;
const MAX_LINE_CHARS: usize = 2000;
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

pub struct Read;

#[async_trait]
impl Tool for Read {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> String {
        "Read a file. Text comes back with line numbers (cat -n style); images (png, jpg, gif, webp) \
         come back as images. Use offset and limit to page through large files."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute or working-directory-relative path" },
                "offset": { "type": "integer", "description": "1-based line to start from" },
                "limit": { "type": "integer", "description": "Maximum lines to return (default 2000)" }
            },
            "required": ["path"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    fn summary(&self, input: &Value) -> String {
        input["path"].as_str().unwrap_or_default().to_owned()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        read(&input, ctx).await.into()
    }
}

async fn read(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let path = ctx.resolve(str_arg(input, "path")?);
    let meta = tokio::fs::metadata(&path)
        .await
        .with_context(|| format!("cannot read {}", path.display()))?;
    if meta.is_dir() {
        bail!("{} is a directory; use glob or bash `ls`", path.display());
    }
    if let Some(media_type) = image_type(&path) {
        if meta.len() > MAX_IMAGE_BYTES {
            bail!("image is larger than 5 MB");
        }
        let bytes = tokio::fs::read(&path).await?;
        ctx.files.record(&path);
        return Ok(ToolOutput {
            content: vec![Block::Image {
                media_type: media_type.into(),
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            }],
            is_error: false,
        });
    }

    let bytes = tokio::fs::read(&path).await?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        bail!("{} looks like a binary file", path.display());
    }
    ctx.files.record(&path);
    let text = String::from_utf8_lossy(&bytes);
    if text.is_empty() {
        return Ok(ToolOutput::text("(empty file)"));
    }

    let offset = input["offset"].as_u64().unwrap_or(1).max(1) as usize;
    let limit = input["limit"]
        .as_u64()
        .map_or(DEFAULT_LINE_LIMIT, |n| n as usize);
    let total = text.lines().count();
    let mut out = String::new();
    for (number, line) in text.lines().enumerate().skip(offset - 1).take(limit) {
        let line = super::truncate_chars(line, MAX_LINE_CHARS);
        out.push_str(&format!("{:>6}\t{line}\n", number + 1));
    }
    let shown_end = (offset - 1 + limit).min(total);
    if shown_end < total {
        out.push_str(&format!(
            "\n[showing lines {offset}-{shown_end} of {total}; use offset to read more]"
        ));
    }
    Ok(ToolOutput::text(out))
}

fn image_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

pub struct Write;

#[async_trait]
impl Tool for Write {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> String {
        "Create a file or replace its entire contents. Read an existing file before overwriting it. \
         Prefer `edit` for changes to existing files."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Edit
    }

    fn summary(&self, input: &Value) -> String {
        input["path"].as_str().unwrap_or_default().to_owned()
    }

    fn permission_keys(&self, input: &Value) -> Vec<String> {
        input["path"]
            .as_str()
            .map(str::to_owned)
            .into_iter()
            .collect()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        write(&input, ctx).await.into()
    }
}

async fn write(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let path = ctx.resolve(str_arg(input, "path")?);
    let content = str_arg(input, "content")?;
    ctx.files.ensure_fresh(&path)?;
    let existed = path.exists();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, content).await?;
    ctx.files.record(&path);
    let verb = if existed { "Wrote" } else { "Created" };
    Ok(ToolOutput::text(format!(
        "{verb} {} ({} lines)",
        ctx.display_path(&path),
        content.lines().count()
    )))
}

pub struct Edit;

#[async_trait]
impl Tool for Edit {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> String {
        "Replace an exact string in a file. `old_string` must match the file exactly, including \
         whitespace, and be unique unless `replace_all` is set. Read the file first."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old_string": { "type": "string" },
                "new_string": { "type": "string" },
                "replace_all": { "type": "boolean", "default": false }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Edit
    }

    fn summary(&self, input: &Value) -> String {
        input["path"].as_str().unwrap_or_default().to_owned()
    }

    fn permission_keys(&self, input: &Value) -> Vec<String> {
        input["path"]
            .as_str()
            .map(str::to_owned)
            .into_iter()
            .collect()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        edit(&input, ctx).await.into()
    }
}

async fn edit(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let path = ctx.resolve(str_arg(input, "path")?);
    let old = str_arg(input, "old_string")?;
    let new = str_arg(input, "new_string")?;
    let replace_all = input["replace_all"].as_bool().unwrap_or(false);
    if old.is_empty() {
        bail!("old_string is empty; use `write` to create files");
    }
    if old == new {
        bail!("old_string and new_string are identical");
    }
    if !path.exists() {
        bail!("{} does not exist", path.display());
    }
    ctx.files.ensure_fresh(&path)?;

    let original = tokio::fs::read_to_string(&path).await?;
    let updated = apply_edit(&original, old, new, replace_all)?;
    tokio::fs::write(&path, &updated).await?;
    ctx.files.record(&path);

    let start = original.find(old).unwrap_or(0);
    let line = original[..start].matches('\n').count() + 1;
    Ok(ToolOutput::text(format!(
        "Edited {}:\n{}",
        ctx.display_path(&path),
        excerpt(&updated, line, new.lines().count().max(1))
    )))
}

fn apply_edit(original: &str, old: &str, new: &str, replace_all: bool) -> Result<String> {
    match original.matches(old).count() {
        0 => bail!("old_string not found; read the file again and copy the text exactly"),
        1 => Ok(original.replacen(old, new, 1)),
        _ if replace_all => Ok(original.replace(old, new)),
        n => bail!("old_string matches {n} places; include more context or set replace_all"),
    }
}

/// Numbered lines around an edit so the model can check the result.
fn excerpt(text: &str, first_line: usize, span: usize) -> String {
    let from = first_line.saturating_sub(3).max(1);
    let to = first_line + span + 2;
    text.lines()
        .enumerate()
        .skip(from - 1)
        .take(to - from + 1)
        .map(|(i, l)| format!("{:>6}\t{l}", i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_requires_unique_match() {
        assert_eq!(apply_edit("a b a", "b", "c", false).unwrap(), "a c a");
        assert!(apply_edit("a b a", "a", "c", false).is_err());
        assert_eq!(apply_edit("a b a", "a", "c", true).unwrap(), "c b c");
        assert!(apply_edit("abc", "x", "y", false).is_err());
    }

    #[test]
    fn excerpt_is_numbered() {
        let text = "1\n2\n3\n4\n5\n6\n7\n8";
        let out = excerpt(text, 5, 1);
        assert!(out.starts_with("     2\t2"));
        assert!(out.ends_with("     8\t8"));
    }
}
