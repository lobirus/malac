//! Talking to the user outside the reply: files they should see, and
//! sentences read aloud.

use anyhow::{Result, bail};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolKind, ToolOutput};
use crate::event::Event;
use crate::shared;

/// Largest file sent at once.
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_FILES: usize = 10;
/// Longest sentence `say` reads aloud.
const MAX_SAY_CHARS: usize = 300;

pub struct SendToUser;

#[async_trait]
impl Tool for SendToUser {
    fn name(&self) -> &str {
        "send_to_user"
    }

    fn description(&self) -> String {
        "Put files in front of the user, in every malac app they use (terminal, web, editor, phone): \
         images show inline and can be enlarged, other files can be opened or downloaded. Use it when \
         the user asks you to send, show or share something (\"send me a screenshot\"), or when a result \
         is best seen: a chart, a rendered page, a generated file. `recent_images` sends the newest \
         images your tools returned, such as a browser or desktop screenshot you just took. Do not use \
         it for files the user only needs to know the path of."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "files": { "type": "array", "items": { "type": "string" }, "description": "Paths of files to send" },
                "recent_images": { "type": "integer", "minimum": 1, "maximum": 8, "description": "Send this many of the newest images from your tool results" },
                "message": { "type": "string", "description": "A short caption shown with the files" }
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    fn summary(&self, input: &Value) -> String {
        let mut parts: Vec<String> = input["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| {
                f.as_str()
                    .map(|f| f.rsplit('/').next().unwrap_or(f).to_owned())
            })
            .collect();
        if let Some(n) = input["recent_images"].as_u64() {
            parts.push(if n == 1 {
                "the last image".into()
            } else {
                format!("the last {n} images")
            });
        }
        parts.join(", ")
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        send(&input, ctx).into()
    }
}

fn send(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let paths: Vec<&str> = input["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let images = input["recent_images"].as_u64().unwrap_or(0).min(8) as usize;
    if paths.is_empty() && images == 0 {
        bail!("pass `files`, `recent_images` or both");
    }
    if paths.len() > MAX_FILES {
        bail!("send at most {MAX_FILES} files at once");
    }
    let mut sent = Vec::new();
    for path in paths {
        let path = ctx.resolve(path);
        let meta =
            std::fs::metadata(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        if !meta.is_file() {
            bail!(
                "{} is not a file; zip a folder before sending it",
                path.display()
            );
        }
        if meta.len() > MAX_FILE_BYTES {
            bail!(
                "{} is over {} MB",
                path.display(),
                MAX_FILE_BYTES / 1024 / 1024
            );
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        sent.push(shared::save(&name, &std::fs::read(&path)?)?);
    }
    if images > 0 {
        let recent = ctx.images.newest(images);
        if recent.is_empty() {
            bail!("none of your tools returned an image yet; take a screenshot first");
        }
        let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M.%S");
        let count = recent.len();
        for (index, (media_type, data)) in recent.into_iter().enumerate() {
            let bytes = BASE64.decode(data)?;
            let suffix = if count > 1 {
                format!(" ({})", index + 1)
            } else {
                String::new()
            };
            let name = format!(
                "Screenshot {stamp}{suffix}.{}",
                shared::image_extension(&media_type)
            );
            sent.push(shared::save(&name, &bytes)?);
        }
    }
    let names: Vec<&str> = sent.iter().map(|f| f.name.as_str()).collect();
    let mut text = format!("Sent to the user: {}.", names.join(", "));
    if let Some(message) = input["message"].as_str().filter(|m| !m.trim().is_empty()) {
        // Angle brackets would read as tags in clients' parsers.
        let caption: String = message
            .trim()
            .chars()
            .filter(|c| !matches!(c, '<' | '>'))
            .collect();
        text.push_str(&format!("\n<caption>{caption}</caption>"));
    }
    for file in &sent {
        text.push('\n');
        text.push_str(&file.tag());
    }
    Ok(ToolOutput::text(text))
}

pub struct Say;

#[async_trait]
impl Tool for Say {
    fn name(&self) -> &str {
        "say"
    }

    fn description(&self) -> String {
        "Say one short sentence aloud through the user's speaker; they turned speech on. Use it sparingly: \
         when you need them (a question, a decision, something only they can do), when a long task \
         finishes, or when something notable happens, such as tests failing or a deploy going out. \
         Write it to be heard: plain words, no code, paths, numbers with many digits or markdown. \
         Approval requests are announced for you."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "text": { "type": "string", "description": "One or two short spoken sentences" } },
            "required": ["text"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    fn summary(&self, input: &Value) -> String {
        input["text"].as_str().unwrap_or_default().to_owned()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let text: String = input["text"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .chars()
            .take(MAX_SAY_CHARS)
            .collect();
        if text.is_empty() {
            return ToolOutput::error("`text` is empty");
        }
        ctx.emit(Event::Say { text });
        ToolOutput::text("Said.")
    }
}
