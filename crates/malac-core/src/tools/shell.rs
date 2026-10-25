use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolKind, ToolOutput, str_arg, truncate_chars};
use crate::config;

const DEFAULT_TIMEOUT: u64 = 120;
const MAX_TIMEOUT: u64 = 1800;
const MAX_CAPTURE: usize = 4 * 1024 * 1024;
/// Grace period for output after the shell exits; background children may hold the pipe.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

pub struct Bash;

#[async_trait]
impl Tool for Bash {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> String {
        "Run a shell command in the working directory and return combined stdout and stderr. \
         Each call starts a fresh shell, so `cd` does not persist; chain with `&&` instead. \
         Set `background` for servers and watchers: output then goes to a log file you can read later."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "description": { "type": "string", "description": "A few words on what the command does" },
                "timeout": { "type": "integer", "description": "Seconds before the command is killed (default 120, max 1800)" },
                "background": { "type": "boolean", "description": "Start without waiting for it to finish" }
            },
            "required": ["command"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Execute
    }

    fn summary(&self, input: &Value) -> String {
        let command = input["command"].as_str().unwrap_or_default();
        truncate_chars(command.lines().next().unwrap_or_default(), 160)
    }

    fn permission_keys(&self, input: &Value) -> Vec<String> {
        input["command"]
            .as_str()
            .map(command_segments)
            .unwrap_or_default()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let result = match input["background"].as_bool() {
            Some(true) => run_background(&input, ctx).await,
            _ => run_foreground(&input, ctx).await,
        };
        result.into()
    }
}

/// Splits a command line on `&&`, `||`, `;`, `|` and newlines so permission
/// rules apply to every program it runs.
pub fn command_segments(command: &str) -> Vec<String> {
    command
        .split(['\n', ';', '|', '&'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| s.ends_with("bash") || s.ends_with("zsh"))
        .unwrap_or_else(|| "/bin/sh".into())
}

fn command(script: &str, ctx: &ToolContext) -> Command {
    let mut cmd = Command::new(shell());
    cmd.arg("-c")
        .arg(script)
        .current_dir(&ctx.cwd)
        .stdin(Stdio::null())
        .env("MALAC", "1")
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat");
    #[cfg(unix)]
    cmd.process_group(0);
    cmd
}

async fn run_foreground(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let script = str_arg(input, "command")?;
    let timeout = input["timeout"]
        .as_u64()
        .unwrap_or(DEFAULT_TIMEOUT)
        .clamp(1, MAX_TIMEOUT);

    let mut child = command(&format!("exec 2>&1\n{script}"), ctx)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("failed to start shell")?;
    let pid = child.id();

    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut stdout = child.stdout.take().context("stdout unavailable")?;
    let sink = captured.clone();
    let reader = tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        while let Ok(n) = stdout.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let mut out = sink.lock().unwrap();
            if out.len() < MAX_CAPTURE {
                out.extend_from_slice(&buf[..n]);
            }
        }
    });

    let outcome = tokio::select! {
        status = child.wait() => Outcome::Exited(status?.code()),
        _ = tokio::time::sleep(Duration::from_secs(timeout)) => Outcome::TimedOut,
        _ = ctx.cancel.cancelled() => Outcome::Cancelled,
    };
    if !matches!(outcome, Outcome::Exited(_)) {
        kill_group(pid);
        let _ = child.kill().await;
    }
    if tokio::time::timeout(DRAIN_GRACE, reader).await.is_err() {
        kill_group(pid);
    }

    let bytes = std::mem::take(&mut *captured.lock().unwrap());
    let mut text = String::from_utf8_lossy(&bytes).trim_end().to_owned();
    let is_error = match outcome {
        Outcome::Exited(Some(0)) => false,
        Outcome::Exited(code) => {
            let code = code.map_or("signal".to_owned(), |c| c.to_string());
            text.push_str(&format!("\n[exit code {code}]"));
            true
        }
        Outcome::TimedOut => {
            text.push_str(&format!("\n[killed after {timeout}s timeout]"));
            true
        }
        Outcome::Cancelled => {
            text.push_str("\n[cancelled]");
            true
        }
    };
    if text.trim().is_empty() {
        text = "(no output)".into();
    }
    let mut output = ToolOutput::text(text);
    output.is_error = is_error;
    Ok(output)
}

enum Outcome {
    Exited(Option<i32>),
    TimedOut,
    Cancelled,
}

async fn run_background(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let script = str_arg(input, "command")?;
    let dir = config::home_dir().join("logs");
    std::fs::create_dir_all(&dir)?;
    let log_path = dir.join(format!("bash-{}.log", uuid::Uuid::now_v7()));
    let log = std::fs::File::create(&log_path)?;
    let child = command(script, ctx)
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .context("failed to start shell")?;
    Ok(ToolOutput::text(format!(
        "Started in background (pid {}). Output is written to {}",
        child.id().unwrap_or_default(),
        log_path.display()
    )))
}

#[cfg(unix)]
fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        // The shell leads its own process group, so this reaches its children too.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: Option<u32>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_compound_commands() {
        assert_eq!(
            command_segments("cargo build && cargo test | tee out; echo done"),
            vec!["cargo build", "cargo test", "tee out", "echo done"]
        );
    }
}
