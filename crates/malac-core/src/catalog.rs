//! Discovers agent types and skills on disk and in plugins.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::frontmatter;
use crate::plugins::Plugin;
use crate::tools;

/// Files listed for a skill at most.
const MAX_SKILL_FILES: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct AgentType {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    /// Tool names this agent may use; `None` means all.
    pub tools: Option<Vec<String>>,
    #[serde(skip)]
    pub instructions: String,
    /// `user`, `project` or `plugin <id>`.
    pub source: String,
}

/// `*.md` files in `~/.malac/agents`, `<project>/.malac/agents` and the
/// enabled plugins' `agents/`; project files win, plugin agents are named
/// `<plugin>:<agent>`.
pub fn agent_types(home: &Path, project_root: &Path, plugins: &[Plugin]) -> Vec<AgentType> {
    let mut types: Vec<AgentType> = Vec::new();
    for plugin in plugins {
        for path in &plugin.components.agents {
            if let Some(mut agent) = read_agent(path, &format!("plugin {}", plugin.id)) {
                agent.name = format!("{}:{}", plugin.name, agent.name);
                agent.instructions = plugin.substitute(&agent.instructions);
                types.push(agent);
            }
        }
    }
    for (dir, source) in [
        (home.join("agents"), "user"),
        (project_root.join(".malac").join("agents"), "project"),
    ] {
        for path in markdown_files(&dir) {
            if let Some(agent) = read_agent(&path, source) {
                types.retain(|t| t.name != agent.name);
                types.push(agent);
            }
        }
    }
    types
}

fn read_agent(path: &Path, source: &str) -> Option<AgentType> {
    let text = std::fs::read_to_string(path).ok()?;
    let (front, body) = frontmatter::parse(&text);
    Some(AgentType {
        name: front.string("name").unwrap_or_else(|| stem(path)),
        description: front.string("description").unwrap_or_default(),
        // Claude Code agents name models like `sonnet`; only `provider/model` means something here.
        model: front.string("model").filter(|m| m.contains('/')),
        tools: front.list("tools", false).map(|names| {
            names
                .iter()
                .map(|name| tools::from_claude_name(name).unwrap_or(name).to_owned())
                .collect()
        }),
        instructions: body.trim().to_owned(),
        source: source.to_owned(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The `SKILL.md` file.
    pub path: PathBuf,
    /// `user`, `project` or `plugin <id>`.
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// Listed in the system prompt for the model to load when it fits.
    /// Claude Code's `disable-model-invocation` turns it off.
    pub model_invocable: bool,
    /// Runnable as `/<name>`.
    pub user_invocable: bool,
    /// The plugin it comes from, for `${CLAUDE_PLUGIN_ROOT}` and friends.
    #[serde(skip)]
    pub plugin: Option<PluginPaths>,
}

#[derive(Debug, Clone)]
pub struct PluginPaths {
    pub root: PathBuf,
    pub data: PathBuf,
}

impl Skill {
    pub fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(&self.path)
    }

    /// Files besides `SKILL.md` (scripts, references, assets), relative to
    /// the skill directory.
    pub fn files(&self) -> Vec<PathBuf> {
        let dir = self.dir();
        let mut files: Vec<PathBuf> = ignore::WalkBuilder::new(dir)
            .max_depth(Some(4))
            .require_git(false)
            .build()
            .flatten()
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .filter_map(|e| e.path().strip_prefix(dir).ok().map(Path::to_path_buf))
            .filter(|p| p != Path::new("SKILL.md"))
            .take(MAX_SKILL_FILES)
            .collect();
        files.sort();
        files
    }

    /// Whether it ships scripts the agent may run.
    pub fn has_scripts(&self) -> bool {
        self.dir().join("scripts").is_dir()
    }

    /// `SKILL.md` without its frontmatter, with the skill's directory (and
    /// plugin paths) written in for the variables that name them.
    pub fn instructions(&self) -> std::io::Result<String> {
        let text = std::fs::read_to_string(&self.path)?;
        let (_, body) = frontmatter::split(&text);
        Ok(self.substitute(body.trim()))
    }

    pub fn substitute(&self, text: &str) -> String {
        let dir = self.dir().display().to_string();
        let mut text = text
            .replace("${CLAUDE_SKILL_DIR}", &dir)
            .replace("${SKILL_DIR}", &dir)
            .replace("$SKILL_DIR", &dir);
        if let Some(plugin) = &self.plugin {
            text = substitute_plugin(&text, &plugin.root, &plugin.data);
        }
        text
    }
}

/// Writes a plugin's root and data directories in for `${CLAUDE_PLUGIN_ROOT}`,
/// `${PLUGIN_ROOT}` and their `_DATA` counterparts.
pub fn substitute_plugin(text: &str, root: &Path, data: &Path) -> String {
    let root = root.display().to_string();
    let data = data.display().to_string();
    text.replace("${CLAUDE_PLUGIN_ROOT}", &root)
        .replace("${PLUGIN_ROOT}", &root)
        .replace("${CLAUDE_PLUGIN_DATA}", &data)
        .replace("${PLUGIN_DATA}", &data)
}

/// Skills from the enabled plugins (named `<plugin>:<skill>`), then
/// `<dir>/<skill>/SKILL.md` in the user's and the project's skill folders
/// (malac's, Claude Code's and `.agents/skills`); later ones win.
pub fn skills(
    home: &Path,
    user_home: Option<&Path>,
    project_root: &Path,
    plugins: &[Plugin],
) -> (Vec<Skill>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut skills: Vec<Skill> = Vec::new();
    for plugin in plugins {
        for path in &plugin.components.skills {
            let source = format!("plugin {}", plugin.id);
            let Some(mut skill) = read_skill(path, &source) else {
                continue;
            };
            if let Err(problem) = check_skill(&skill) {
                warnings.push(format!(
                    "plugin {}: skipped skill {}: {problem}",
                    plugin.id,
                    path.display()
                ));
                continue;
            }
            skill.name = format!("{}:{}", plugin.name, skill.name);
            skill.plugin = Some(PluginPaths {
                root: plugin.root.clone(),
                data: plugin.data.clone(),
            });
            skills.push(skill);
        }
    }
    let mut roots = vec![(home.join("skills"), "user")];
    if let Some(user_home) = user_home {
        roots.push((user_home.join(".claude").join("skills"), "user"));
        roots.push((user_home.join(".agents").join("skills"), "user"));
    }
    roots.extend([
        (project_root.join(".agents").join("skills"), "project"),
        (project_root.join(".claude").join("skills"), "project"),
        (project_root.join(".malac").join("skills"), "project"),
    ]);
    for (root, source) in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            if let Some(skill) = read_skill(&dir.join("SKILL.md"), source) {
                skills.retain(|s| s.name != skill.name);
                skills.push(skill);
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    (skills, warnings)
}

fn read_skill(path: &Path, source: &str) -> Option<Skill> {
    let text = std::fs::read_to_string(path).ok()?;
    let (front, _) = frontmatter::parse(&text);
    let dir_name = path
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(Skill {
        name: front.string("name").unwrap_or(dir_name),
        description: front.string("description").unwrap_or_default(),
        path: path.to_path_buf(),
        source: source.to_owned(),
        argument_hint: front.string("argument-hint"),
        model_invocable: front.bool("disable-model-invocation") != Some(true),
        user_invocable: front.bool("user-invocable") != Some(false),
        plugin: None,
    })
}

/// The Agent Skills rules a plugin's skill must meet: a description and a
/// lowercase name of letters, digits and single hyphens.
fn check_skill(skill: &Skill) -> Result<(), String> {
    if skill.description.trim().is_empty() {
        return Err("no description".into());
    }
    if !valid_skill_name(&skill.name) {
        return Err(format!("invalid name `{}`", skill.name));
    }
    Ok(())
}

pub fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    files
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_agents() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let agents = project.path().join(".malac/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews diffs\ntools: Read, grep, Bash\nmodel: sonnet\n---\nBe strict.",
        )
        .unwrap();

        let types = agent_types(home.path(), project.path(), &[]);
        assert_eq!(types.len(), 1);
        assert_eq!(
            types[0].tools.as_deref(),
            Some(&["read".to_owned(), "grep".to_owned(), "bash".to_owned()][..])
        );
        assert_eq!(types[0].model, None);
        assert_eq!(types[0].instructions, "Be strict.");
    }

    #[test]
    fn reads_skills_with_their_files() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let dir = project.path().join(".agents/skills/release");
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::create_dir_all(dir.join("references")).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: release\ndescription: >\n  Cut a release.\n  Use when shipping.\nargument-hint: \"[version]\"\ndisable-model-invocation: true\n---\nRun `$SKILL_DIR/scripts/bump.sh` then read ${SKILL_DIR}/references/notes.md.\n",
        )
        .unwrap();
        std::fs::write(dir.join("scripts/bump.sh"), "#!/bin/sh\necho bumped\n").unwrap();
        std::fs::write(dir.join("references/notes.md"), "notes").unwrap();

        let (skills, warnings) = skills(home.path(), None, project.path(), &[]);
        assert!(warnings.is_empty());
        let skill = skills.iter().find(|s| s.name == "release").unwrap();
        assert_eq!(skill.description, "Cut a release. Use when shipping.");
        assert!(!skill.model_invocable);
        assert!(skill.has_scripts());
        assert_eq!(
            skill.files(),
            vec![
                PathBuf::from("references/notes.md"),
                PathBuf::from("scripts/bump.sh")
            ]
        );
        let text = skill.instructions().unwrap();
        let dir = dir.display().to_string();
        assert_eq!(
            text,
            format!("Run `{dir}/scripts/bump.sh` then read {dir}/references/notes.md.")
        );
    }

    #[test]
    fn validates_skill_names() {
        assert!(valid_skill_name("pdf-processing"));
        assert!(!valid_skill_name("PDF"));
        assert!(!valid_skill_name("-pdf"));
        assert!(!valid_skill_name("pdf--x"));
    }
}
