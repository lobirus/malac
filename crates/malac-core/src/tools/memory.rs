use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolKind, ToolOutput, str_arg};
use crate::memory::Scope;

pub struct Save;

#[async_trait]
impl Tool for Save {
    fn name(&self) -> &str {
        "memory_save"
    }

    fn description(&self) -> String {
        "Save a durable fact for future sessions: user preferences, project commands, conventions, gotchas, \
         decisions and their reasons, pointers to external resources. One fact per memory; saving an existing \
         name replaces it. Do not save what the code or git history already records. Scope `global` follows \
         the user everywhere. Scope `project` is committed with the repository in `.malac/memory/` and listed \
         in `.malac/MALAC.md` under its `kind`, so teammates and other machines get it too."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Short kebab-case identifier" },
                "description": { "type": "string", "description": "One line used to decide relevance later" },
                "content": { "type": "string" },
                "scope": { "type": "string", "enum": ["global", "project"] },
                "kind": {
                    "type": "string",
                    "enum": ["command", "convention", "gotcha", "decision", "note"],
                    "description": "For project memories: the section of MALAC.md it belongs to"
                }
            },
            "required": ["name", "description", "content", "scope"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    fn summary(&self, input: &Value) -> String {
        input["name"].as_str().unwrap_or_default().to_owned()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        save(&input, ctx).await.into()
    }
}

async fn save(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let scope = Scope::parse(str_arg(input, "scope")?)?;
    let entry = ctx
        .runtime
        .memory
        .save(
            scope,
            str_arg(input, "name")?,
            str_arg(input, "description")?,
            input["kind"].as_str(),
            str_arg(input, "content")?,
        )
        .await?;
    Ok(ToolOutput::text(format!(
        "Saved memory `{}` to {}",
        entry.name,
        entry.path.display()
    )))
}

pub struct Search;

#[async_trait]
impl Tool for Search {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> String {
        "Search saved memories by meaning (or keywords when no vector store is configured).".into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer", "default": 5 }
            },
            "required": ["query"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    fn summary(&self, input: &Value) -> String {
        input["query"].as_str().unwrap_or_default().to_owned()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        search(&input, ctx).await.into()
    }
}

async fn search(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let limit = input["limit"].as_u64().unwrap_or(5) as usize;
    let hits = ctx
        .runtime
        .memory
        .search(str_arg(input, "query")?, limit)
        .await?;
    if hits.is_empty() {
        return Ok(ToolOutput::text("No matching memories"));
    }
    let text = hits
        .iter()
        .map(|h| {
            format!(
                "## {} ({:?}, score {:.2})\n{}\n\n{}",
                h.entry.name, h.entry.scope, h.score, h.entry.description, h.entry.body
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(ToolOutput::text(text))
}

pub struct Forget;

#[async_trait]
impl Tool for Forget {
    fn name(&self) -> &str {
        "memory_forget"
    }

    fn description(&self) -> String {
        "Delete a memory that turned out to be wrong or obsolete.".into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "scope": { "type": "string", "enum": ["global", "project"] }
            },
            "required": ["name", "scope"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    fn summary(&self, input: &Value) -> String {
        input["name"].as_str().unwrap_or_default().to_owned()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        forget(&input, ctx).await.into()
    }
}

async fn forget(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let name = str_arg(input, "name")?;
    ctx.runtime
        .memory
        .forget(Scope::parse(str_arg(input, "scope")?)?, name)
        .await?;
    Ok(ToolOutput::text(format!("Forgot `{name}`")))
}
