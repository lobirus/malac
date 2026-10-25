//! Follow and stop the background jobs started with `bash` `background: true`.

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolKind, ToolOutput, str_arg, truncate_chars};
use crate::jobs::{DEFAULT_TAIL_LINES, JobInfo, MAX_TAIL_LINES};

const LISTED: usize = 50;

pub struct List;

#[async_trait]
impl Tool for List {
    fn name(&self) -> &str {
        "job_list"
    }

    fn description(&self) -> String {
        "List background jobs (commands started with bash `background: true`), newest first, \
         with their status, process id and working directory."
            .into()
    }

    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    async fn run(&self, _input: Value, ctx: &ToolContext) -> ToolOutput {
        let jobs = ctx.runtime.jobs.list();
        if jobs.is_empty() {
            return ToolOutput::text("No background jobs");
        }
        let rows: Vec<String> = jobs.iter().take(LISTED).map(describe).collect();
        ToolOutput::text(rows.join("\n"))
    }
}

fn describe(job: &JobInfo) -> String {
    let elapsed = job.finished.unwrap_or_else(Utc::now) - job.started;
    let pid = job.pid.map(|p| format!(" pid {p}")).unwrap_or_default();
    format!(
        "- {} {}{pid} after {}: `{}` in {}",
        job.id,
        job.describe_status(),
        duration(elapsed),
        truncate_chars(job.command.lines().next().unwrap_or_default(), 120),
        job.cwd.display()
    )
}

fn duration(delta: chrono::TimeDelta) -> String {
    let secs = delta.num_seconds().max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m{}s", secs / 60, secs % 60),
        _ => format!("{}h{}m", secs / 3600, secs % 3600 / 60),
    }
}

pub struct Output;

#[async_trait]
impl Tool for Output {
    fn name(&self) -> &str {
        "job_output"
    }

    fn description(&self) -> String {
        "Show the latest output of a background job and whether it is still running.".into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "Job id from bash or job_list" },
                "tail": { "type": "integer", "description": "Lines from the end (default 100, max 2000)" }
            },
            "required": ["id"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        output(&input, ctx).into()
    }
}

fn output(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let id = str_arg(input, "id")?;
    let lines = input["tail"]
        .as_u64()
        .map_or(DEFAULT_TAIL_LINES, |n| n as usize)
        .min(MAX_TAIL_LINES);
    let out = ctx.runtime.jobs.output(id, lines)?;
    let body = if out.text.trim().is_empty() {
        "(no output yet)".to_owned()
    } else if out.truncated {
        format!("[... earlier output omitted ...]\n{}", out.text)
    } else {
        out.text
    };
    Ok(ToolOutput::text(format!(
        "{}\n\n{body}",
        describe(&out.job)
    )))
}

pub struct Kill;

#[async_trait]
impl Tool for Kill {
    fn name(&self) -> &str {
        "job_kill"
    }

    fn description(&self) -> String {
        "Stop a background job and every process it started: SIGTERM, then SIGKILL after 3 seconds."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "string" } },
            "required": ["id"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Execute
    }

    fn permission_keys(&self, input: &Value) -> Vec<String> {
        input["id"]
            .as_str()
            .map(str::to_owned)
            .into_iter()
            .collect()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let result = str_arg(&input, "id").and_then(|id| ctx.runtime.jobs.kill(id));
        match result {
            Ok(job) => ToolOutput::text(format!("Stopping job {}: `{}`", job.id, job.command)),
            Err(err) => ToolOutput::error(format!("{err:#}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(duration(chrono::TimeDelta::seconds(42)), "42s");
        assert_eq!(duration(chrono::TimeDelta::seconds(125)), "2m5s");
        assert_eq!(duration(chrono::TimeDelta::seconds(7322)), "2h2m");
    }
}
