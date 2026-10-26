//! Assembles the system prompt from the base text and what is on disk.

use std::path::Path;
use std::process::Command;

use crate::catalog::AgentType;
use crate::config;
use crate::memory::Scope;
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
    sections.push(environment(inputs.cwd, runtime, inputs.model));

    for file in &runtime.instructions {
        sections.push(format!(
            "# Instructions from {}\n\nThese come from the user and take precedence over the defaults above.\n\n{}",
            file.path.display(),
            file.text
        ));
    }
    let rules = &runtime.context.instructions.rules;
    if !rules.is_empty() {
        let list: Vec<String> = rules
            .iter()
            .map(|r| {
                let applies = if r.globs.is_empty() {
                    String::new()
                } else {
                    format!(" (for {})", r.globs.join(", "))
                };
                format!("- {}{applies}: {}", r.path.display(), r.description)
            })
            .collect();
        sections.push(format!(
            "# Rules for some files\n\nThe project has rules that apply only to some files or tasks. Read a rule \
             before working on what it covers, and follow it like the instructions above.\n\n{}",
            list.join("\n")
        ));
    }

    if let Some(knowledge) = &runtime.context.knowledge {
        sections.push(format!(
            "# Project knowledge\n\nWhat earlier sessions learned about this project, from {} (each entry is a \
             project memory).\n\n{knowledge}",
            crate::context::knowledge::path(&runtime.project_root).display()
        ));
    }

    if runtime.context.index.is_some() {
        sections.push(
            "# Project index\n\n`.malac/index.md` maps this repository: its tree with file counts, languages, \
             manifests and entry points, the top-level symbols of each source file, doc headings, and notes. It is \
             regenerated as files change. Before searching broadly, look there with `project_index` (or `query` it \
             for a name) and go straight to the files it points at. When you learn something that would have saved \
             you a search, add a `note`."
                .to_owned(),
        );
    }

    if !runtime.context.docs.is_empty() {
        sections.push(format!(
            "# Project docs\n\nThe project's documentation, not loaded. Read the ones that bear on the task.\n\n{}",
            crate::context::docs::render(&runtime.context.docs)
        ));
    }

    if runtime.memory.enabled() {
        let project_in_knowledge = runtime.context.knowledge.is_some();
        let scope = project_in_knowledge.then_some(Scope::Global);
        let index = runtime.memory.prompt_index(scope);
        let mut memory = String::from(
            "# Memory\n\nYou have persistent memory across sessions. Save durable facts with `memory_save`: \
             the user's preferences and corrections with scope `global`; what you learn about this project with \
             scope `project` and a `kind` (`command` for how to build, test and run it, `convention`, `gotcha`, \
             `decision` with its reason, or `note`). Project memories are committed in `.malac/` and listed in \
             `.malac/MALAC.md`, so they reach the next session, other machines and teammates. Instructions for \
             agents belong in AGENTS.md, which the user owns: suggest changes to it rather than writing it \
             unasked. Update or forget memories that turn out wrong. Read a memory's file when its description \
             looks relevant.",
        );
        if !index.is_empty() {
            memory.push_str(if project_in_knowledge {
                "\n\nSaved global memories (project ones are under Project knowledge):\n"
            } else {
                "\n\nSaved memories:\n"
            });
            memory.push_str(&index);
        }
        sections.push(memory);
    }

    let skills: Vec<String> = runtime
        .skills
        .iter()
        .filter(|s| s.model_invocable)
        .map(|s| format!("- {}: {}", s.name, s.description))
        .collect();
    if !skills.is_empty() {
        sections.push(format!(
            "# Skills\n\nSkills are instruction packs for specific tasks. When one fits the task, load it with \
             `skill` and follow it; it may come with scripts to run and references to read.\n\n{}",
            skills.join("\n")
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

    if inputs.depth < runtime.config.agents.max_depth {
        if let Some(models) = models_section(&runtime.config, inputs.model) {
            sections.push(models);
        }
    }

    for (server, text) in runtime.mcp_instructions() {
        sections.push(format!(
            "# Instructions from the `{server}` tool server\n\n{text}"
        ));
    }
    sections.join("\n\n")
}

/// Most models listed in the prompt.
const MAX_LISTED_MODELS: usize = 40;

/// The configured models and what they are for, so subagents get a model
/// that fits their task. `None` when there is nothing to choose from.
fn models_section(config: &config::Config, current: &str) -> Option<String> {
    let mut specs: Vec<String> = config
        .providers
        .iter()
        .flat_map(|(name, p)| p.models.iter().map(move |m| format!("{name}/{m}")))
        .collect();
    for (spec, model) in &config.models {
        if model.description.is_some() && !specs.contains(spec) {
            specs.push(spec.clone());
        }
    }
    if specs.len() < 2 {
        return None;
    }
    let lines: Vec<String> = specs
        .iter()
        .take(MAX_LISTED_MODELS)
        .map(|spec| {
            let yours = if spec == current { " (yours)" } else { "" };
            match &config.model_config(spec).description {
                Some(description) => format!("- {spec}{yours}: {description}"),
                None => format!("- {spec}{yours}"),
            }
        })
        .collect();
    Some(format!(
        "# Available models\n\n`agent_spawn` takes a `model`; without one a subagent uses yours ({current}). \
         Match the model to the task: a fast, cheap one for searches, lookups, summaries and mechanical \
         edits; the strongest one for design, hard debugging and reviews; one with vision for screenshots.\n\n{}",
        lines.join("\n")
    ))
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

fn environment(cwd: &Path, runtime: &Runtime, model: &str) -> String {
    let project_root = &runtime.project_root;
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
    lines.extend(runtime.context.layout.prompt_line());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ModelConfig, ProviderConfig};

    #[test]
    fn lists_models_with_descriptions() {
        let mut config = Config::default();
        config.providers.insert(
            "lora".into(),
            ProviderConfig {
                models: vec!["smart".into(), "fast".into()],
                ..Default::default()
            },
        );
        config.models.insert(
            "lora/fast".into(),
            ModelConfig {
                description: Some("fast and cheap".into()),
                ..Default::default()
            },
        );
        config.models.insert(
            "other/vision".into(),
            ModelConfig {
                description: Some("sees images".into()),
                ..Default::default()
            },
        );
        let section = models_section(&config, "lora/smart").unwrap();
        assert!(section.contains("- lora/smart (yours)\n"));
        assert!(section.contains("- lora/fast: fast and cheap"));
        assert!(section.contains("- other/vision: sees images"));
    }

    #[test]
    fn skips_the_section_without_a_choice() {
        let mut config = Config::default();
        config.providers.insert(
            "lora".into(),
            ProviderConfig {
                models: vec!["smart".into()],
                ..Default::default()
            },
        );
        assert!(models_section(&config, "lora/smart").is_none());
    }
}
