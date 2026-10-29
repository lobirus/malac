use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolKind, ToolOutput, str_arg, truncate_chars};

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
         Set `background` for servers and watchers: the command becomes a job that keeps running, \
         and `job_list`, `job_output` and `job_kill` follow and stop it."
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

/// A shell running `script` in `cwd`, leading its own process group so the
/// whole tree can be stopped at once.
pub(crate) fn shell_command(script: &str, cwd: &std::path::Path) -> Command {
    let mut cmd = Command::new(shell());
    cmd.arg("-c")
        .arg(script)
        .current_dir(cwd)
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
    let script = format!("{}{script}", ctx.env.exports());
    let run = run_script(
        script_command(&script, &ctx.cwd),
        timeout,
        ctx.cancel.cancelled(),
    )
    .await?;
    let mut output = ToolOutput::text(run.report(timeout));
    output.is_error = run.outcome != Outcome::Exited(Some(0));
    Ok(output)
}

/// What a script printed, stdout and stderr together and at most
/// `MAX_CAPTURE` bytes of it, and how it ended.
pub struct ScriptRun {
    pub output: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// With the exit code; `None` when a signal ended it.
    Exited(Option<i32>),
    TimedOut,
    Cancelled,
}

impl ScriptRun {
    /// The output as the `bash` tool reports it: how it ended on a last line
    /// unless it succeeded, and "(no output)" for none.
    pub fn report(&self, timeout: u64) -> String {
        let mut text = self.output.clone();
        match self.outcome {
            Outcome::Exited(Some(0)) => {}
            Outcome::Exited(code) => {
                let code = code.map_or("signal".to_owned(), |c| c.to_string());
                text.push_str(&format!("\n[exit code {code}]"));
            }
            Outcome::TimedOut => text.push_str(&format!("\n[killed after {timeout}s timeout]")),
            Outcome::Cancelled => text.push_str("\n[cancelled]"),
        }
        if text.trim().is_empty() {
            text = "(no output)".into();
        }
        text
    }
}

/// A shell running `script` in `cwd` for [`run_script`], with its stderr
/// sent to stdout so the output reads in order.
pub fn script_command(script: &str, cwd: &std::path::Path) -> Command {
    shell_command(&format!("exec 2>&1\n{script}"), cwd)
}

/// Runs a shell from [`script_command`] the way the `bash` tool does,
/// killing its whole process group after `timeout` seconds or once
/// `cancelled` completes.
pub async fn run_script(
    mut command: Command,
    timeout: u64,
    cancelled: impl std::future::Future<Output = ()>,
) -> Result<ScriptRun> {
    let mut child = command
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
        _ = cancelled => Outcome::Cancelled,
    };
    if !matches!(outcome, Outcome::Exited(_)) {
        kill_group(pid);
        let _ = child.kill().await;
    }
    if tokio::time::timeout(DRAIN_GRACE, reader).await.is_err() {
        kill_group(pid);
    }

    let bytes = std::mem::take(&mut *captured.lock().unwrap());
    Ok(ScriptRun {
        output: String::from_utf8_lossy(&bytes).trim_end().to_owned(),
        outcome,
    })
}

async fn run_background(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let script = str_arg(input, "command")?;
    let script = format!("{}{script}", ctx.env.exports());
    let job = ctx
        .runtime
        .jobs
        .start(&script, &ctx.cwd, Some(&ctx.agent))?;
    // Front ends read the pid and the log path, which comes last, from this text.
    Ok(ToolOutput::text(format!(
        "Started job {} in the background (pid {}). Check on it with job_output and stop it \
         with job_kill. Output is written to {}",
        job.id,
        job.pid.unwrap_or_default(),
        job.log.display()
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
