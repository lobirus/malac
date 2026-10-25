//! Assembles the system prompt from the base text and what is on disk.

use std::path::Path;
use std::process::Command;

use crate::catalog::AgentType;
use crate::config;
use crate::permission::Mode;
use crate::runtime::Runtime;

const BASE: &str = include_str!("../prompts/system.md");
const SUBAGENT: &str = include_str!("../prompts/subagent.md");
pub const COMPACT: &str = include_str!("../prompts/compact.md");

pub struct PromptInputs<'a> {
    pub runtime: &'a Runtime,
    pub cwd: &'a Path,
    pub model: &'a str,
    pub depth: u32,
    pub agent_type: Option<&'a AgentType>,
}

/// The stable part of the prompt, built once per agent.
pub fn build(inputs: &PromptInputs<'_>) -> String {
    let runtime = inputs.runtime;
    let mut sections = vec![BASE.trim().to_owned()];
    if inputs.depth > 0 {
        sections.push(SUBAGENT.trim().to_owned());
    }
    if let Some(agent) = inputs.agent_type {
        sections.push(format!("# Role: {}\n\n{}", agent.name, agent.instructions));
    }
    sections.push(environment(inputs.cwd, &runtime.project_root, inputs.model));

    for file in &runtime.instructions {
        sections.push(format!(
            "# Instructions from {}\n\nThese come from the user and take precedence over the defaults above.\n\n{}",
            file.path.display(),
            file.text
        ));
    }

    if runtime.memory.enabled() {
        let index = runtime.memory.prompt_index();
        let mut memory = String::from(
            "# Memory\n\nYou have persistent memory across sessions. Save durable facts with `memory_save` \
             (user preferences, corrections, project decisions and why they were made); update or forget \
             memories that turn out to be wrong. Read a memory's file when its description looks relevant.",
        );
        if !index.is_empty() {
            memory.push_str("\n\nSaved memories:\n");
            memory.push_str(&index);
        }
        sections.push(memory);
    }

    if !runtime.skills.is_empty() {
        let list: Vec<String> = runtime
            .skills
            .iter()
            .map(|s| format!("- {}: {} ({})", s.name, s.description, s.path.display()))
            .collect();
        sections.push(format!(
            "# Skills\n\nSkills are instruction packs for specific tasks. When one matches the task, read its file \
             and follow it.\n\n{}",
            list.join("\n")
        ));
    }

    if inputs.depth < runtime.config.agents.max_depth && !runtime.agent_types.is_empty() {
        let list: Vec<String> = runtime
            .agent_types
            .iter()
            .map(|a| format!("- {}: {}", a.name, a.description))
            .collect();
        sections.push(format!(
            "# Agent types\n\nPass one as `agent` to `agent_spawn` for specialized work:\n\n{}",
            list.join("\n")
        ));
    }

    for (server, text) in runtime.mcp_instructions() {
        sections.push(format!(
            "# Instructions from the `{server}` tool server\n\n{text}"
        ));
    }
    sections.join("\n\n")
}

/// The part that can change between turns.
pub fn autonomy(mode: Mode, depth: u32) -> String {
    let body = match mode {
        Mode::Autopilot if depth == 0 => {
            "You are on autopilot: the user is away and every tool call is pre-approved. Do not stop to ask \
             questions. Make sensible decisions, note the assumptions you made, and carry the task to completion. \
             Still never do anything destructive or irreversible that the user did not ask for."
        }
        Mode::Autopilot => "Tool calls are pre-approved.",
        Mode::Edits => "File edits are pre-approved; commands may need the user's approval.",
        Mode::Ask => {
            "The user approves edits and commands as you make them. If they deny a call, adjust your approach."
        }
    };
    format!("# Permissions\n\n{body}")
}

fn environment(cwd: &Path, project_root: &Path, model: &str) -> String {
    let mut lines = vec![
        format!("- Working directory: {}", cwd.display()),
        format!(
            "- Platform: {} ({})",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
        format!("- Date: {}", chrono::Local::now().format("%Y-%m-%d")),
        format!("- Model: {model}"),
        format!("- malac home: {}", config::home_dir().display()),
    ];
    if let Some(branch) = git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        lines.push(format!(
            "- Git repository: {} (branch {branch})",
            project_root.display()
        ));
        if let Some(status) = git(cwd, &["status", "--short"]) {
            let changed: Vec<&str> = status.lines().take(20).collect();
            if !changed.is_empty() {
                lines.push(format!(
                    "- Uncommitted changes:\n  {}",
                    changed.join("\n  ")
                ));
            }
        }
    }
    format!("# Environment\n\n{}", lines.join("\n"))
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
