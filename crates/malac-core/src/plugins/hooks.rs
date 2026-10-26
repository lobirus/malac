//! Plugin hooks in Claude Code's format (`hooks/hooks.json`): shell
//! commands that run at points of a session, read a JSON description of
//! the event on stdin, and answer with an exit code and optional JSON.
//!
//! Supported events: `SessionStart` (and `SubagentStart` for subagents),
//! `UserPromptSubmit`, `PreToolUse` (allow, deny, ask, or change the input)
//! and `PostToolUse` (feedback for the model). Other events are reported as
//! unsupported and never run. Only `command` hooks run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use futures::future::join_all;
use regex::Regex;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::catalog::substitute_plugin;

pub const SUPPORTED_EVENTS: &[&str] = &[
    "SessionStart",
    "SubagentStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
];
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_OUTPUT: usize = 64 * 1024;

/// One hook as declared.
#[derive(Debug, Clone, PartialEq)]
pub struct HookSpec {
    pub event: String,
    pub matcher: Option<String>,
    pub command: String,
    /// Exec form: run `command` with these arguments, without a shell.
    pub args: Option<Vec<String>>,
    pub timeout: Option<u64>,
}

/// The hooks of an event map (`{"PreToolUse": [{matcher, hooks: [...]}]}`),
/// and what it holds that malac does not run.
pub fn parse(events: &Value) -> (Vec<HookSpec>, Vec<String>) {
    let mut specs = Vec::new();
    let mut unsupported = Vec::new();
    let Some(events) = events.as_object() else {
        return (specs, unsupported);
    };
    for (event, groups) in events {
        if !SUPPORTED_EVENTS.contains(&event.as_str()) {
            unsupported.push(format!("hook event {event}"));
            continue;
        }
        for group in groups.as_array().into_iter().flatten() {
            let matcher = group
                .get("matcher")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty() && *m != "*")
                .map(str::to_owned);
            for hook in group
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let kind = hook
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("command");
                let Some(command) = hook
                    .get("command")
                    .and_then(Value::as_str)
                    .filter(|_| kind == "command")
                else {
                    unsupported.push(format!("{kind} hooks"));
                    continue;
                };
                specs.push(HookSpec {
                    event: event.clone(),
                    matcher: matcher.clone(),
                    command: command.to_owned(),
                    args: hook.get("args").and_then(Value::as_array).map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    }),
                    timeout: hook.get("timeout").and_then(Value::as_u64),
                });
            }
        }
    }
    (specs, unsupported)
}

/// A hook ready to run for a plugin.
#[derive(Debug, Clone)]
struct Hook {
    plugin: String,
    event: String,
    matcher: Option<Regex>,
    command: String,
    args: Option<Vec<String>>,
    timeout: Duration,
    root: PathBuf,
    data: PathBuf,
}

/// Every enabled plugin's hooks.
#[derive(Debug, Clone, Default)]
pub struct Hooks {
    hooks: Vec<Hook>,
}

/// What a session's hooks are told about it.
#[derive(Debug, Clone)]
pub struct Session<'a> {
    pub id: &'a str,
    pub cwd: &'a Path,
    pub project_dir: &'a Path,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PromptOutcome {
    /// Set when a hook refused the prompt, with its reason.
    pub blocked: Option<String>,
    pub context: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    Deny(String),
    Ask(String),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolOutcome {
    pub decision: Option<Decision>,
    /// Replacement tool input.
    pub input: Option<Value>,
}

struct Output {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Output {
    fn json(&self) -> Option<Value> {
        let text = self.stdout.trim();
        text.starts_with('{')
            .then(|| serde_json::from_str(text).ok())
            .flatten()
    }
}

impl Hooks {
    /// Adds a plugin's hooks; ones with an invalid matcher are reported.
    pub fn add(
        &mut self,
        plugin: &str,
        root: &Path,
        data: &Path,
        specs: &[HookSpec],
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        for spec in specs {
            let matcher = match &spec.matcher {
                Some(pattern) => match Regex::new(&format!("^(?:{pattern})$")) {
                    Ok(regex) => Some(regex),
                    Err(err) => {
                        warnings.push(format!(
                            "plugin {plugin}: skipped hook with matcher `{pattern}`: {err}"
                        ));
                        continue;
                    }
                },
                None => None,
            };
            self.hooks.push(Hook {
                plugin: plugin.to_owned(),
                event: spec.event.clone(),
                matcher,
                command: spec.command.clone(),
                args: spec.args.clone(),
                timeout: spec.timeout.map_or(DEFAULT_TIMEOUT, Duration::from_secs),
                root: root.to_path_buf(),
                data: data.to_path_buf(),
            });
        }
        warnings
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn has(&self, event: &str) -> bool {
        self.hooks.iter().any(|h| h.event == event)
    }

    /// Hook events per plugin, for display.
    pub fn summary(&self) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for hook in &self.hooks {
            let events = out.entry(hook.plugin.clone()).or_default();
            if !events.contains(&hook.event) {
                events.push(hook.event.clone());
            }
        }
        out
    }

    fn matching<'a>(
        &'a self,
        event: &'a str,
        subjects: &'a [&'a str],
    ) -> impl Iterator<Item = &'a Hook> + 'a {
        self.hooks.iter().filter(move |h| {
            h.event == event
                && h.matcher
                    .as_ref()
                    .is_none_or(|m| subjects.iter().any(|s| m.is_match(s)))
        })
    }

    async fn run_all(
        &self,
        event: &str,
        subjects: &[&str],
        session: &Session<'_>,
        input: Value,
    ) -> Vec<(String, Output)> {
        let hooks: Vec<&Hook> = self.matching(event, subjects).collect();
        if hooks.is_empty() {
            return Vec::new();
        }
        let runs = hooks.iter().map(|hook| {
            let input = input.clone();
            async move { (hook.plugin.clone(), run(hook, session, &input).await) }
        });
        let outputs = join_all(runs).await;
        for (plugin, output) in &outputs {
            if !matches!(output.code, Some(0) | Some(2)) {
                tracing::warn!(
                    "plugin {plugin}: {event} hook failed ({}): {}",
                    output
                        .code
                        .map_or("killed".to_owned(), |c| format!("exit {c}")),
                    output.stderr.trim()
                );
            }
        }
        outputs
    }

    /// Context from `SessionStart` hooks, or `SubagentStart` for subagents.
    /// `source` is `startup` or `resume`.
    pub async fn session_start(
        &self,
        session: &Session<'_>,
        source: &str,
        subagent: Option<&str>,
    ) -> Vec<String> {
        let (event, subject) = match subagent {
            Some(agent_type) => ("SubagentStart", agent_type),
            None => ("SessionStart", source),
        };
        let input = base_input(
            session,
            event,
            json!({ "source": source, "agent_type": subject }),
        );
        self.run_all(event, &[subject], session, input)
            .await
            .into_iter()
            .filter(|(_, o)| o.code == Some(0))
            .filter_map(|(_, output)| context(&output, true))
            .collect()
    }

    pub async fn prompt_submit(&self, session: &Session<'_>, prompt: &str) -> PromptOutcome {
        let input = base_input(session, "UserPromptSubmit", json!({ "prompt": prompt }));
        let mut outcome = PromptOutcome::default();
        for (plugin, output) in self
            .run_all("UserPromptSubmit", &[""], session, input)
            .await
        {
            if let Some(reason) = blocked(&output) {
                outcome.blocked = Some(format!("blocked by plugin {plugin}: {reason}"));
            } else if output.code == Some(0) {
                outcome.context.extend(context(&output, true));
            }
        }
        outcome
    }

    /// `tool` is malac's name; the hook sees Claude Code's where one exists
    /// (`Bash`), and its matcher may use either.
    pub async fn pre_tool(&self, session: &Session<'_>, tool: &str, input: &Value) -> ToolOutcome {
        let shown = crate::tools::claude_name(tool).unwrap_or(tool);
        let event_input = base_input(
            session,
            "PreToolUse",
            json!({ "tool_name": shown, "tool_input": hook_input(input) }),
        );
        let mut outcome = ToolOutcome::default();
        for (plugin, output) in self
            .run_all("PreToolUse", &[shown, tool], session, event_input)
            .await
        {
            let from = |reason: &str| format!("plugin {plugin}: {reason}");
            if output.code == Some(2) {
                outcome.decision = Some(Decision::Deny(from(output.stderr.trim())));
                continue;
            }
            let Some(json) = output.json().filter(|_| output.code == Some(0)) else {
                continue;
            };
            let specific = &json["hookSpecificOutput"];
            let reason = specific["permissionDecisionReason"]
                .as_str()
                .or(json["reason"].as_str())
                .unwrap_or_default();
            let decision = match (
                specific["permissionDecision"].as_str(),
                json["decision"].as_str(),
            ) {
                (Some("deny"), _) | (_, Some("block")) => Some(Decision::Deny(from(reason))),
                (Some("ask"), _) => Some(Decision::Ask(from(reason))),
                (Some("allow"), _) | (_, Some("approve")) => Some(Decision::Allow),
                _ if json["continue"] == Value::Bool(false) => Some(Decision::Deny(from(
                    json["stopReason"].as_str().unwrap_or(reason),
                ))),
                _ => None,
            };
            // Deny beats ask beats allow.
            outcome.decision = match (outcome.decision.take(), decision) {
                (Some(Decision::Deny(r)), _) | (_, Some(Decision::Deny(r))) => {
                    Some(Decision::Deny(r))
                }
                (Some(Decision::Ask(r)), _) | (_, Some(Decision::Ask(r))) => Some(Decision::Ask(r)),
                (current, new) => new.or(current),
            };
            if let Some(updated) = specific.get("updatedInput").filter(|v| v.is_object()) {
                outcome.input = Some(tool_input(updated, input));
            }
        }
        outcome
    }

    /// Feedback for the model after a tool ran.
    pub async fn post_tool(
        &self,
        session: &Session<'_>,
        tool: &str,
        input: &Value,
        output: &str,
        is_error: bool,
    ) -> Vec<String> {
        let shown = crate::tools::claude_name(tool).unwrap_or(tool);
        let event_input = base_input(
            session,
            "PostToolUse",
            json!({
                "tool_name": shown,
                "tool_input": hook_input(input),
                "tool_response": { "output": output, "is_error": is_error },
            }),
        );
        let mut feedback = Vec::new();
        for (plugin, out) in self
            .run_all("PostToolUse", &[shown, tool], session, event_input)
            .await
        {
            if let Some(reason) = blocked(&out) {
                feedback.push(format!("plugin {plugin}: {reason}"));
            } else if out.code == Some(0) {
                feedback.extend(context(&out, false).map(|c| format!("plugin {plugin}: {c}")));
            }
        }
        feedback
    }
}

fn base_input(session: &Session<'_>, event: &str, extra: Value) -> Value {
    let mut input = json!({
        "session_id": session.id,
        "transcript_path": crate::session::sessions_dir().join(format!("{}.jsonl", session.id)),
        "cwd": session.cwd,
        "hook_event_name": event,
    });
    if let (Some(map), Value::Object(extra)) = (input.as_object_mut(), extra) {
        map.extend(extra);
    }
    input
}

/// The input as Claude Code's tools name it: `file_path` beside `path`.
fn hook_input(input: &Value) -> Value {
    let mut input = input.clone();
    if let Some(map) = input.as_object_mut() {
        if let Some(path) = map.get("path").cloned() {
            map.entry("file_path").or_insert(path);
        }
    }
    input
}

/// A hook's replacement input in malac's terms.
fn tool_input(updated: &Value, original: &Value) -> Value {
    let mut updated = updated.clone();
    if let Some(map) = updated.as_object_mut() {
        if original.get("path").is_some() {
            if let Some(path) = map.remove("file_path") {
                map.insert("path".into(), path);
            }
        }
    }
    updated
}

/// A refusal: exit code 2 (reason on stderr) or `decision: "block"`.
fn blocked(output: &Output) -> Option<String> {
    if output.code == Some(2) {
        let reason = output.stderr.trim();
        return Some(if reason.is_empty() {
            "no reason given".into()
        } else {
            reason.to_owned()
        });
    }
    let json = output.json().filter(|_| output.code == Some(0))?;
    if json["decision"] == "block" {
        return Some(
            json["reason"]
                .as_str()
                .unwrap_or("no reason given")
                .to_owned(),
        );
    }
    if json["continue"] == Value::Bool(false) {
        return Some(
            json["stopReason"]
                .as_str()
                .unwrap_or("no reason given")
                .to_owned(),
        );
    }
    None
}

/// `additionalContext`, or plain stdout where it counts as context.
fn context(output: &Output, plain_counts: bool) -> Option<String> {
    let text = match output.json() {
        Some(json) => json["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .or(json["additionalContext"].as_str())
            .map(str::to_owned)?,
        None if plain_counts => output.stdout.trim().to_owned(),
        None => return None,
    };
    (!text.trim().is_empty()).then_some(text)
}

async fn run(hook: &Hook, session: &Session<'_>, input: &Value) -> Output {
    let expand = |s: &str| substitute_plugin(s, &hook.root, &hook.data);
    let project = session.project_dir.display().to_string();
    let mut command = match &hook.args {
        Some(args) => {
            let mut command = tokio::process::Command::new(expand(&hook.command));
            command
                .args(args.iter().map(|a| expand(a)))
                .current_dir(session.cwd);
            #[cfg(unix)]
            command.process_group(0);
            command
        }
        None => crate::tools::shell_command(
            &expand(&hook.command).replace("${CLAUDE_PROJECT_DIR}", &project),
            session.cwd,
        ),
    };
    if let Err(err) = std::fs::create_dir_all(&hook.data) {
        tracing::debug!("cannot create {}: {err}", hook.data.display());
    }
    // Claude Code's variables only: hooks are its format, and plugins tell
    // hosts apart by which variables they see (`PLUGIN_DATA` means Codex).
    command
        .env("CLAUDE_PLUGIN_ROOT", &hook.root)
        .env("CLAUDE_PLUGIN_DATA", &hook.data)
        .env("CLAUDE_PROJECT_DIR", session.project_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let failed = |message: String| Output {
        code: None,
        stdout: String::new(),
        stderr: message,
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return failed(format!("cannot start `{}`: {err}", hook.command)),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let bytes = serde_json::to_vec(input).unwrap_or_default();
        // A hook that ignores its input may exit before reading it.
        let _ = stdin.write_all(&bytes).await;
    }
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let collect = async {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let read_out = async {
            if let Some(s) = stdout.as_mut() {
                let _ = s.read_to_end(&mut out).await;
            }
        };
        let read_err = async {
            if let Some(s) = stderr.as_mut() {
                let _ = s.read_to_end(&mut err).await;
            }
        };
        tokio::join!(read_out, read_err);
        let status = child.wait().await;
        (status, out, err)
    };
    let text =
        |bytes: &[u8]| String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_OUTPUT)]).into_owned();
    match tokio::time::timeout(hook.timeout, collect).await {
        Ok((status, out, err)) => Output {
            code: status.ok().and_then(|s| s.code()),
            stdout: text(&out),
            stderr: text(&err),
        },
        Err(_) => failed(format!("timed out after {}s", hook.timeout.as_secs())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks(events: Value, root: &Path) -> Hooks {
        let (specs, unsupported) = parse(&events);
        assert!(unsupported.is_empty(), "{unsupported:?}");
        let mut hooks = Hooks::default();
        assert!(
            hooks
                .add("demo", root, &root.join("data"), &specs)
                .is_empty()
        );
        hooks
    }

    fn session(dir: &Path) -> Session<'_> {
        Session {
            id: "s1",
            cwd: dir,
            project_dir: dir,
        }
    }

    #[tokio::test]
    async fn runs_session_and_prompt_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = hooks(
            json!({
                "SessionStart": [{"matcher": "startup|resume", "hooks": [
                    {"type": "command", "command": "echo \"root=$CLAUDE_PLUGIN_ROOT\""},
                    {"type": "command", "command": "echo '{\"hookSpecificOutput\": {\"additionalContext\": \"json context\"}}'"}
                ]}],
                "UserPromptSubmit": [{"hooks": [
                    {"type": "command", "command": "grep -q secret && { echo 'no secrets' >&2; exit 2; } || echo 'prompt seen'"}
                ]}]
            }),
            dir.path(),
        );
        let session = session(dir.path());
        let mut context = hooks.session_start(&session, "startup", None).await;
        context.sort();
        assert_eq!(
            context,
            vec![
                "json context".to_owned(),
                format!("root={}", dir.path().display())
            ]
        );
        assert!(
            hooks
                .session_start(&session, "startup", Some("reviewer"))
                .await
                .is_empty()
        );

        let ok = hooks.prompt_submit(&session, "hello").await;
        assert_eq!(ok.context, vec!["prompt seen"]);
        let refused = hooks.prompt_submit(&session, "my secret").await;
        assert_eq!(
            refused.blocked.as_deref(),
            Some("blocked by plugin demo: no secrets")
        );
    }

    #[tokio::test]
    async fn pre_tool_hooks_decide_and_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = hooks(
            json!({
                "PreToolUse": [
                    {"matcher": "Bash", "hooks": [{"type": "command", "command": "grep -q 'rm -rf' && { echo 'too dangerous' >&2; exit 2; } || true"}]},
                    {"matcher": "Write|Edit", "hooks": [{"type": "command", "command": "echo '{\"hookSpecificOutput\": {\"permissionDecision\": \"allow\", \"updatedInput\": {\"file_path\": \"b.txt\", \"content\": \"x\"}}}'"}]}
                ],
                "PostToolUse": [{"matcher": "bash", "hooks": [{"type": "command", "command": "echo '{\"decision\": \"block\", \"reason\": \"lint failed\"}'"}]}]
            }),
            dir.path(),
        );
        let session = session(dir.path());
        let denied = hooks
            .pre_tool(&session, "bash", &json!({"command": "rm -rf /tmp/x"}))
            .await;
        assert_eq!(
            denied.decision,
            Some(Decision::Deny("plugin demo: too dangerous".into()))
        );
        let fine = hooks
            .pre_tool(&session, "bash", &json!({"command": "ls"}))
            .await;
        assert_eq!(fine, ToolOutcome::default());
        let rewritten = hooks
            .pre_tool(&session, "write", &json!({"path": "a.txt", "content": "x"}))
            .await;
        assert_eq!(rewritten.decision, Some(Decision::Allow));
        assert_eq!(
            rewritten.input,
            Some(json!({"path": "b.txt", "content": "x"}))
        );
        let feedback = hooks
            .post_tool(&session, "bash", &json!({"command": "ls"}), "out", false)
            .await;
        assert_eq!(feedback, vec!["plugin demo: lint failed"]);
        assert!(
            hooks
                .post_tool(&session, "read", &json!({}), "", false)
                .await
                .is_empty()
        );
    }

    #[test]
    fn reports_unsupported_events_and_types() {
        let (specs, unsupported) = parse(&json!({
            "Stop": [{"hooks": [{"type": "command", "command": "x"}]}],
            "PostToolUse": [{"hooks": [{"type": "prompt", "prompt": "check"}]}]
        }));
        assert!(specs.is_empty());
        assert_eq!(unsupported, vec!["prompt hooks", "hook event Stop"]);
    }
}
