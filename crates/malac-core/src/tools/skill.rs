//! Loads a skill: its full instructions and the files it ships.

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolKind, ToolOutput, str_arg};
use crate::catalog::Skill;

pub struct Load;

#[async_trait]
impl Tool for Load {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> String {
        "Load a skill listed under Skills in your instructions when its description fits the task: returns its \
         full instructions and the files it ships (scripts/, references/, assets/). Follow the instructions. \
         Read references with `read` when they are needed. Run scripts with `bash`; from now on your shell \
         commands have SKILL_DIR set to the skill's directory."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "The skill's name as listed, e.g. `ponytail:ponytail-review`" },
                "arguments": { "type": "string", "description": "Arguments for the skill's $ARGUMENTS, if it takes any" }
            },
            "required": ["name"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    fn summary(&self, input: &Value) -> String {
        input["name"].as_str().unwrap_or_default().to_owned()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        load(&input, ctx).into()
    }
}

/// A skill by its listed name, or by the part after `plugin:` when that is unique.
pub fn find<'a>(skills: &'a [Skill], name: &str) -> Result<&'a Skill> {
    let name = name.trim().trim_start_matches('/');
    if let Some(skill) = skills.iter().find(|s| s.name == name) {
        return Ok(skill);
    }
    let matches: Vec<&Skill> = skills
        .iter()
        .filter(|s| s.name.rsplit(':').next() == Some(name))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => bail!(
            "no skill named `{name}`; available: {}",
            skills
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        many => bail!(
            "`{name}` is ambiguous: {}",
            many.iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn load(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let skill = find(&ctx.runtime.skills, str_arg(input, "name")?)?;
    let mut instructions = skill.instructions()?;
    if let Some(args) = input["arguments"].as_str() {
        instructions = instructions.replace("$ARGUMENTS", args);
    }
    let dir = skill.dir();
    ctx.env.set("SKILL_DIR", dir.display().to_string());
    if let Some(plugin) = &skill.plugin {
        ctx.env
            .set("CLAUDE_PLUGIN_ROOT", plugin.root.display().to_string());
        ctx.env
            .set("CLAUDE_PLUGIN_DATA", plugin.data.display().to_string());
    }
    let files = skill.files();
    let listing = if files.is_empty() {
        "(only SKILL.md)".to_owned()
    } else {
        files
            .iter()
            .map(|f| format!("- {}", f.display()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(ToolOutput::text(format!(
        "# Skill `{}` ({})\n\nDirectory (SKILL_DIR): {}\nFiles:\n{listing}\n\n{instructions}",
        skill.name,
        skill.source,
        dir.display()
    )))
}
