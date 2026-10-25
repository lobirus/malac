//! Delegation: spawn subagents, collect their reports, steer them.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures::future::join_all;
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolKind, ToolOutput, str_arg, truncate_chars};
use crate::agent::{AgentHandle, AgentStatus};
use crate::message::Block;
use crate::runtime::AgentSpec;

const DEFAULT_WAIT_SECS: u64 = 1800;

pub struct Spawn;

#[async_trait]
impl Tool for Spawn {
    fn name(&self) -> &str {
        "agent_spawn"
    }

    fn description(&self) -> String {
        "Delegate a self-contained task to a new subagent with its own context window and the same tools. \
         The brief must be complete: the subagent cannot see this conversation. By default this waits and \
         returns the subagent's report; several calls in one response run in parallel. Set `background` to \
         return immediately and collect the report later with `agent_wait`."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "description": "Complete brief: goal, context, files, what done looks like" },
                "label": { "type": "string", "description": "Short name shown in the UI, e.g. `test-runner`" },
                "agent": { "type": "string", "description": "Optional agent type, only if listed under Agent types" },
                "model": { "type": "string", "description": "Override the model, as provider/model" },
                "read_only": { "type": "boolean", "description": "Only allow tools that cannot change anything" },
                "background": { "type": "boolean" }
            },
            "required": ["task"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    fn summary(&self, input: &Value) -> String {
        let label = input["label"]
            .as_str()
            .or(input["agent"].as_str())
            .unwrap_or("agent");
        let task = input["task"].as_str().unwrap_or_default();
        format!(
            "{label}: {}",
            truncate_chars(task.lines().next().unwrap_or_default(), 100)
        )
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        spawn(&input, ctx).await.into()
    }
}

async fn spawn(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let task = str_arg(input, "task")?;
    let mut label = input["label"].as_str().unwrap_or_default().to_owned();
    // Models often put a role name here; treat an unknown type as a label.
    let agent_type = match input["agent"].as_str() {
        Some(name) if ctx.runtime.agent_type(name).is_some() => Some(name.to_owned()),
        Some(name) => {
            if label.is_empty() {
                label = name.to_owned();
            }
            None
        }
        None => None,
    };
    let handle = ctx.runtime.spawn_agent(AgentSpec {
        label,
        parent: Some(ctx.agent.clone()),
        depth: ctx.depth + 1,
        model: input["model"].as_str().map(str::to_owned),
        cwd: Some(ctx.cwd.clone()),
        agent_type,
        read_only: input["read_only"].as_bool().unwrap_or(false),
    })?;
    let content = vec![Block::text(task)];
    if input["background"].as_bool().unwrap_or(false) {
        handle.send(content);
        return Ok(ToolOutput::text(format!(
            "Started `{}` (id {}) in the background. Collect its report with agent_wait.",
            handle.info().label,
            handle.id
        )));
    }
    let result = tokio::select! {
        result = handle.prompt(content) => result,
        _ = ctx.cancel.cancelled() => {
            ctx.runtime.cancel_tree(&handle.id);
            Err(anyhow::anyhow!("cancelled"))
        }
    };
    Ok(report(&handle, result))
}

fn report(handle: &AgentHandle, result: Result<String>) -> ToolOutput {
    let header = format!("Report from `{}` (id {})", handle.info().label, handle.id);
    match result {
        Ok(text) if text.trim().is_empty() => ToolOutput::text(format!("{header}: (empty report)")),
        Ok(text) => ToolOutput::text(format!("{header}:\n\n{text}")),
        Err(err) => ToolOutput::error(format!("{header}: failed: {err:#}")),
    }
}

pub struct Wait;

#[async_trait]
impl Tool for Wait {
    fn name(&self) -> &str {
        "agent_wait"
    }

    fn description(&self) -> String {
        "Wait for subagents to finish and return their reports. Without `ids`, waits for all of your \
         subagents that are still working."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "ids": { "type": "array", "items": { "type": "string" } },
                "timeout": { "type": "integer", "description": "Seconds to wait (default 1800)" }
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        wait(&input, ctx).await.into()
    }
}

async fn wait(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let handles = select_children(input, ctx)?;
    if handles.is_empty() {
        return Ok(ToolOutput::text("No subagents to wait for"));
    }
    let timeout = Duration::from_secs(input["timeout"].as_u64().unwrap_or(DEFAULT_WAIT_SECS));
    let waits = join_all(handles.iter().map(AgentHandle::wait_idle));
    tokio::select! {
        _ = tokio::time::timeout(timeout, waits) => {}
        _ = ctx.cancel.cancelled() => bail!("cancelled"),
    }
    let sections: Vec<String> = handles
        .iter()
        .map(|h| {
            let snap = h.snapshot();
            let body = match snap.status {
                AgentStatus::Running | AgentStatus::Idle => {
                    format!("still working: {}", snap.activity)
                }
                _ => snap.report.unwrap_or_default(),
            };
            format!(
                "## `{}` (id {}) — {:?}\n\n{body}",
                snap.info.label, h.id, snap.status
            )
        })
        .collect();
    Ok(ToolOutput::text(sections.join("\n\n")))
}

fn select_children(input: &Value, ctx: &ToolContext) -> Result<Vec<AgentHandle>> {
    let children = ctx.runtime.children(&ctx.agent);
    match input["ids"].as_array() {
        Some(ids) => ids
            .iter()
            .map(|id| {
                let id = id.as_str().context("ids must be strings")?;
                children
                    .iter()
                    .find(|h| h.id == id)
                    .cloned()
                    .with_context(|| format!("no subagent with id {id}"))
            })
            .collect(),
        None => Ok(children.into_iter().filter(AgentHandle::is_busy).collect()),
    }
}

pub struct Send;

#[async_trait]
impl Tool for Send {
    fn name(&self) -> &str {
        "agent_send"
    }

    fn description(&self) -> String {
        "Send a follow-up message to one of your subagents. It keeps its full context, so use this to ask \
         for corrections or more work instead of spawning a new agent."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "message": { "type": "string" },
                "background": { "type": "boolean" }
            },
            "required": ["id", "message"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        send(&input, ctx).await.into()
    }
}

async fn send(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let id = str_arg(input, "id")?;
    let handle = ctx
        .runtime
        .children(&ctx.agent)
        .into_iter()
        .find(|h| h.id == id)
        .with_context(|| format!("no subagent with id {id}"))?;
    let content = vec![Block::text(str_arg(input, "message")?)];
    if input["background"].as_bool().unwrap_or(false) {
        handle.send(content);
        return Ok(ToolOutput::text("Message queued"));
    }
    let result = tokio::select! {
        result = handle.prompt(content) => result,
        _ = ctx.cancel.cancelled() => {
            ctx.runtime.cancel_tree(&handle.id);
            Err(anyhow::anyhow!("cancelled"))
        }
    };
    Ok(report(&handle, result))
}

pub struct List;

#[async_trait]
impl Tool for List {
    fn name(&self) -> &str {
        "agent_list"
    }

    fn description(&self) -> String {
        "List your subagents with their status and current activity.".into()
    }

    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    async fn run(&self, _input: Value, ctx: &ToolContext) -> ToolOutput {
        let rows: Vec<String> = ctx
            .runtime
            .children(&ctx.agent)
            .iter()
            .map(|h| {
                let snap = h.snapshot();
                format!(
                    "- `{}` id {} — {:?}{}",
                    snap.info.label,
                    h.id,
                    snap.status,
                    if snap.activity.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", snap.activity)
                    }
                )
            })
            .collect();
        if rows.is_empty() {
            ToolOutput::text("No subagents")
        } else {
            ToolOutput::text(rows.join("\n"))
        }
    }
}
