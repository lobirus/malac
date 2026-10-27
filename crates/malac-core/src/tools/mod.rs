//! Built-in tools and the [`Tool`] trait shared with MCP-backed tools.

mod agents;
mod fs;
mod index;
mod jobs;
mod mcp;
mod memory;
mod search;
mod settings;
mod share;
mod shell;
mod skill;
mod todo;
mod web;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::event::{AgentId, Event};
use crate::message::Block;
use crate::runtime::Runtime;

pub use mcp::McpTool;
pub use settings::edit as edit_config_text;
pub(crate) use shell::shell_command;

/// Output beyond this is cut from the middle before it reaches the model.
pub const MAX_OUTPUT_CHARS: usize = 40_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// Never needs approval and may run in parallel with other safe tools.
    Safe,
    /// Changes files.
    Edit,
    /// Runs commands or acts outside the workspace.
    Execute,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    fn description(&self) -> String;

    fn schema(&self) -> Value;

    fn kind(&self) -> ToolKind;

    /// One line shown in the UI and in permission prompts.
    fn summary(&self, input: &Value) -> String {
        truncate_chars(&summarize_arguments(input), 120)
    }

    /// Strings matched against `tool(pattern)` permission rules.
    fn permission_keys(&self, _input: &Value) -> Vec<String> {
        Vec::new()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput;
}

#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub content: Vec<Block>,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Block::text(truncate_middle(&text.into(), MAX_OUTPUT_CHARS))],
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            is_error: true,
            ..Self::text(text)
        }
    }

    /// Text of the output, for display.
    pub fn preview(&self) -> String {
        let mut out = String::new();
        for block in &self.content {
            match block {
                Block::Text { text } => out.push_str(text),
                Block::Image { media_type, .. } => out.push_str(&format!("[{media_type} image]")),
                _ => {}
            }
        }
        out
    }
}

impl From<Result<ToolOutput>> for ToolOutput {
    fn from(result: Result<ToolOutput>) -> Self {
        result.unwrap_or_else(|err| ToolOutput::error(format!("{err:#}")))
    }
}

#[derive(Clone)]
pub struct ToolContext {
    pub runtime: Arc<Runtime>,
    pub agent: AgentId,
    pub depth: u32,
    pub cwd: PathBuf,
    pub cancel: CancellationToken,
    pub files: FileTracker,
    /// Variables the agent's shell commands get, like `SKILL_DIR`.
    pub env: SessionEnv,
    /// The newest images tool results returned, for `send_to_user`.
    pub images: RecentImages,
}

/// The last few images an agent's tools returned, as (media type, base64).
#[derive(Clone, Default)]
pub struct RecentImages(Arc<Mutex<std::collections::VecDeque<(String, String)>>>);

impl RecentImages {
    const KEEP: usize = 8;

    pub fn push(&self, output: &ToolOutput) {
        let mut images = self.0.lock().unwrap();
        for block in &output.content {
            if let Block::Image { media_type, data } = block {
                if images.len() == Self::KEEP {
                    images.pop_front();
                }
                images.push_back((media_type.clone(), data.clone()));
            }
        }
    }

    /// Up to `count` of the newest, oldest first.
    pub fn newest(&self, count: usize) -> Vec<(String, String)> {
        let images = self.0.lock().unwrap();
        images
            .iter()
            .skip(images.len().saturating_sub(count))
            .cloned()
            .collect()
    }
}

/// Environment variables for an agent's shell commands, set as it works
/// (loading a skill sets `SKILL_DIR`).
#[derive(Clone, Default)]
pub struct SessionEnv(Arc<Mutex<BTreeMap<String, String>>>);

impl SessionEnv {
    pub fn set(&self, key: &str, value: impl Into<String>) {
        self.0.lock().unwrap().insert(key.to_owned(), value.into());
    }

    pub fn vars(&self) -> BTreeMap<String, String> {
        self.0.lock().unwrap().clone()
    }

    /// `export` lines for these variables, to put before a script.
    pub fn exports(&self) -> String {
        self.vars()
            .iter()
            .map(|(k, v)| format!("export {k}='{}'\n", v.replace('\'', "'\\''")))
            .collect()
    }
}

impl ToolContext {
    pub fn resolve(&self, path: &str) -> PathBuf {
        let expanded = match path.strip_prefix("~/") {
            Some(rest) => dirs::home_dir().map(|h| h.join(rest)),
            None => None,
        };
        let path = expanded.unwrap_or_else(|| PathBuf::from(path));
        if path.is_absolute() {
            path
        } else {
            self.cwd.join(path)
        }
    }

    pub fn emit(&self, event: Event) {
        self.runtime.bus.emit(&self.agent, event);
    }

    /// Path relative to the working directory when it is inside it.
    pub fn display_path(&self, path: &Path) -> String {
        path.strip_prefix(&self.cwd)
            .unwrap_or(path)
            .display()
            .to_string()
    }
}

/// Remembers when each file was last read so edits never clobber changes
/// the model has not seen.
#[derive(Clone, Default)]
pub struct FileTracker(Arc<Mutex<HashMap<PathBuf, Option<SystemTime>>>>);

impl FileTracker {
    pub fn record(&self, path: &Path) {
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        self.0.lock().unwrap().insert(path.to_path_buf(), modified);
    }

    pub fn ensure_fresh(&self, path: &Path) -> Result<()> {
        let Ok(meta) = std::fs::metadata(path) else {
            return Ok(());
        };
        match self.0.lock().unwrap().get(path) {
            None => bail!("read {} before modifying it", path.display()),
            Some(seen) if *seen != meta.modified().ok() => bail!(
                "{} changed since it was last read; read it again first",
                path.display()
            ),
            Some(_) => Ok(()),
        }
    }
}

pub fn builtin() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(fs::Read),
        Arc::new(fs::Write),
        Arc::new(fs::Edit),
        Arc::new(shell::Bash),
        Arc::new(jobs::List),
        Arc::new(jobs::Output),
        Arc::new(jobs::Kill),
        Arc::new(search::Glob),
        Arc::new(search::Grep),
        Arc::new(web::WebFetch),
        Arc::new(todo::Todos),
        Arc::new(skill::Load),
        Arc::new(index::ProjectIndex),
        Arc::new(memory::Save),
        Arc::new(memory::Search),
        Arc::new(memory::Forget),
        Arc::new(agents::Spawn),
        Arc::new(agents::Wait),
        Arc::new(agents::Send),
        Arc::new(agents::List),
        Arc::new(settings::Settings),
        Arc::new(settings::Configure),
        Arc::new(share::SendToUser),
        Arc::new(share::Say),
    ]
}

/// Names of tools that manage subagents, hidden once the depth limit is reached.
pub const AGENT_TOOLS: &[&str] = &["agent_spawn", "agent_wait", "agent_send", "agent_list"];

/// Claude Code's names for the built-in tools, as plugins' agents and hooks use them.
const CLAUDE_NAMES: &[(&str, &str)] = &[
    ("read", "Read"),
    ("write", "Write"),
    ("edit", "Edit"),
    ("edit", "MultiEdit"),
    ("bash", "Bash"),
    ("glob", "Glob"),
    ("grep", "Grep"),
    ("web_fetch", "WebFetch"),
    ("todo", "TodoWrite"),
    ("agent_spawn", "Task"),
    ("agent_spawn", "Agent"),
    ("skill", "Skill"),
];

/// The Claude Code name of a built-in tool, e.g. `Bash` for `bash`.
pub fn claude_name(tool: &str) -> Option<&'static str> {
    CLAUDE_NAMES
        .iter()
        .find(|(ours, _)| *ours == tool)
        .map(|(_, theirs)| *theirs)
}

/// The built-in tool a Claude Code tool name means, e.g. `bash` for `Bash`.
pub fn from_claude_name(name: &str) -> Option<&'static str> {
    CLAUDE_NAMES
        .iter()
        .find(|(_, theirs)| *theirs == name)
        .map(|(ours, _)| *ours)
}

/// `key=value` pairs, with strings unquoted: `url=https://example.com ref=3`.
pub fn summarize_arguments(input: &Value) -> String {
    match input.as_object() {
        Some(map) => map
            .iter()
            .map(|(key, value)| match value {
                Value::String(s) => format!("{key}={}", s.lines().next().unwrap_or_default()),
                other => format!("{key}={other}"),
            })
            .collect::<Vec<_>>()
            .join(" "),
        None => input.to_string(),
    }
}

pub(crate) fn str_arg<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    match input.get(key) {
        Some(Value::String(s)) => Ok(s),
        Some(_) => bail!("`{key}` must be a string"),
        None if input.is_string() => bail!("tool input is not valid JSON"),
        None => bail!("missing required argument `{key}`"),
    }
}

pub fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((idx, _)) => format!("{}…", &text[..idx]),
        None => text.to_owned(),
    }
}

/// Keeps the start and the (usually more relevant) end of long output.
pub fn truncate_middle(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let head_len = max / 3;
    let tail_len = max - head_len;
    let head_end = floor_char_boundary(text, head_len);
    let tail_start = floor_char_boundary(text, text.len() - tail_len);
    let omitted = text[head_end..tail_start].lines().count();
    format!(
        "{}\n\n[... {omitted} lines omitted ...]\n\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_from_the_middle() {
        let text: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let out = truncate_middle(&text, 300);
        assert!(out.starts_with("line 0\n"));
        assert!(out.trim_end().ends_with("line 999"));
        assert!(out.contains("lines omitted"));
        assert_eq!(truncate_middle("short", 300), "short");
    }

    #[test]
    fn summarizes_arguments() {
        let input = serde_json::json!({"url": "https://example.com", "ref": 3});
        assert_eq!(summarize_arguments(&input), "ref=3 url=https://example.com");
        assert_eq!(summarize_arguments(&serde_json::json!({})), "");
    }

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate_chars("héllo", 2), "hé…");
        let out = truncate_middle(&"é".repeat(500), 101);
        assert!(out.contains("omitted"));
    }
}
