//! What malac knows about the project it works in: instruction files,
//! the docs map, the structural index, the knowledge it keeps in
//! `.malac/MALAC.md`, and where the working directory sits in its
//! repository.

pub mod docs;
pub mod index;
pub mod instructions;
pub mod knowledge;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::catalog::{AgentType, Skill};
use crate::commands::CommandInfo;
use crate::config::ContextConfig;
use crate::plugins::PluginInfo;

/// Docs listed in the prompt at most.
const MAX_DOCS: usize = 40;
/// Characters of MALAC.md in the prompt at most.
const MAX_KNOWLEDGE_CHARS: usize = 12_000;
/// An index older than this is refreshed in the background when a session starts.
const INDEX_MAX_AGE: Duration = Duration::from_secs(300);
/// Files whose presence makes a directory a project of its own.
const PROJECT_MARKERS: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
    "requirements.txt",
    "setup.py",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "Gemfile",
    "composer.json",
    "Package.swift",
    "mix.exs",
    "deno.json",
    "pubspec.yaml",
    "CMakeLists.txt",
    "Dockerfile",
    "go.work",
];
/// Directories that are never a part of the project worth suggesting.
const NOT_PARTS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    "docs",
    "tmp",
];

/// Where the working directory sits in its repository.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// The project root: the repository's top directory, or the working
    /// directory outside a repository.
    pub root: PathBuf,
    /// The working directory below the root, when it is not the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdir: Option<PathBuf>,
    /// Other parts of the repository that are projects of their own, such
    /// as `backend` next to `frontend`, relative to the root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub siblings: Vec<String>,
    /// Set when the working directory is one part of a larger repository:
    /// opening this directory instead lets the agent work across all of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggest_root: Option<PathBuf>,
}

fn is_project(dir: &Path) -> bool {
    PROJECT_MARKERS.iter().any(|m| dir.join(m).exists())
        || std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(".csproj"))
}

/// Finds the other project-like directories next to each directory between
/// the root and `cwd`.
pub fn layout(root: &Path, cwd: &Path) -> Layout {
    let mut layout = Layout {
        root: root.to_path_buf(),
        ..Default::default()
    };
    let Ok(rel) = cwd.strip_prefix(root) else {
        return layout;
    };
    if rel.as_os_str().is_empty() {
        return layout;
    }
    layout.subdir = Some(rel.to_path_buf());
    let mut siblings = Vec::new();
    let mut dir = cwd;
    while let Some(parent) = dir.parent().filter(|p| p.starts_with(root) && *p != dir) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(parent)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p != dir)
            .filter(|p| {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                !name.starts_with('.') && !NOT_PARTS.contains(&name.as_str())
            })
            .filter(|p| is_project(p))
            .collect();
        entries.sort();
        siblings.extend(
            entries
                .iter()
                .filter_map(|p| p.strip_prefix(root).ok())
                .map(|p| p.display().to_string()),
        );
        if parent == root {
            break;
        }
        dir = parent;
    }
    siblings.truncate(12);
    if !siblings.is_empty() {
        layout.suggest_root = Some(root.to_path_buf());
    }
    layout.siblings = siblings;
    layout
}

impl Layout {
    /// A line for the prompt's environment section, when the working
    /// directory is part of a larger repository.
    pub fn prompt_line(&self) -> Option<String> {
        let subdir = self.subdir.as_ref()?;
        if self.siblings.is_empty() {
            return Some(format!(
                "- Working in `{}/` of the repository at {}",
                subdir.display(),
                self.root.display()
            ));
        }
        let siblings: Vec<String> = self.siblings.iter().map(|s| format!("`{s}/`")).collect();
        Some(format!(
            "- Working in `{}/` of the repository at {}, which also holds {}. Read and change those parts \
             when the task reaches them (an API both sides use, a shared schema). If the task is really about \
             the whole stack, suggest the user open the repository root instead.",
            subdir.display(),
            self.root.display(),
            siblings.join(", ")
        ))
    }
}

/// What front ends show with a tab.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    #[serde(flatten)]
    pub layout: Layout,
    /// Custom slash commands, for completion and help.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<CommandInfo>,
}

/// What a session in a directory starts with.
#[derive(Debug, Clone, Default)]
pub struct ProjectContext {
    pub layout: Layout,
    pub instructions: instructions::Discovered,
    pub docs: Vec<docs::Doc>,
    /// The structural index, when the directory is a project.
    pub index: Option<index::Index>,
    /// `.malac/MALAC.md`.
    pub knowledge: Option<String>,
}

impl ProjectContext {
    /// Reads everything from disk and starts a background refresh of a
    /// stale index.
    pub fn load(
        config: &ContextConfig,
        malac_home: &Path,
        project_root: &Path,
        cwd: &Path,
    ) -> Self {
        let user_home = dirs::home_dir();
        let instructions = instructions::discover(&instructions::Sources {
            malac_home,
            user_home: user_home.as_deref(),
            project_root,
            cwd,
            extra: &config.instruction_files,
            max_chars: config.max_instruction_chars,
        });
        // An index of a home directory or another non-project would cost
        // more than it helps.
        let is_project = project_root.join(".git").exists() || project_root.join(".malac").is_dir();
        let index = (config.index && is_project)
            .then(|| index::Index::new(project_root, config.index_max_files));
        if let Some(index) = &index {
            if index.is_stale(INDEX_MAX_AGE) {
                index.refresh_in_background();
            }
        }
        Self {
            layout: layout(project_root, cwd),
            instructions,
            docs: if config.docs && is_project {
                docs::discover(project_root, cwd, MAX_DOCS)
            } else {
                Vec::new()
            },
            index,
            knowledge: config
                .knowledge
                .then(|| knowledge::read(project_root))
                .flatten()
                .map(|text| knowledge::for_prompt(&text))
                .filter(|text| !text.is_empty())
                .map(|text| crate::tools::truncate_middle(&text, MAX_KNOWLEDGE_CHARS)),
        }
    }
}

/// Everything a session sees about its project, for `/context`, the
/// settings API and `malac context`.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub layout: Layout,
    pub instructions: instructions::Discovered,
    pub docs: Vec<docs::Doc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<index::Status>,
    pub knowledge: KnowledgeStatus,
    pub skills: Vec<Skill>,
    pub commands: Vec<CommandInfo>,
    pub agent_types: Vec<AgentType>,
    pub plugins: Vec<PluginInfo>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeStatus {
    pub path: PathBuf,
    pub exists: bool,
    pub chars: usize,
}

impl KnowledgeStatus {
    pub fn of(project_root: &Path) -> Self {
        let path = knowledge::path(project_root);
        let chars = std::fs::read_to_string(&path)
            .map(|t| t.chars().count())
            .ok();
        Self {
            exists: chars.is_some(),
            chars: chars.unwrap_or_default(),
            path,
        }
    }
}

impl Report {
    /// The report as Markdown.
    pub fn render(&self) -> String {
        let mut out = vec!["# Context".to_owned()];
        let mut section = |title: &str, lines: Vec<String>, empty: &str| {
            out.push(format!("\n## {title}\n"));
            if lines.is_empty() {
                out.push(empty.to_owned());
            } else {
                out.extend(lines);
            }
        };

        let mut layout = vec![format!("- Project root: {}", self.layout.root.display())];
        if let Some(subdir) = &self.layout.subdir {
            layout.push(format!("- Working directory: `{}/`", subdir.display()));
        }
        if !self.layout.siblings.is_empty() {
            let parts: Vec<String> = self
                .layout
                .siblings
                .iter()
                .map(|s| format!("`{s}/`"))
                .collect();
            layout.push(format!(
                "- Other parts of the repository: {}",
                parts.join(", ")
            ));
        }
        if let Some(root) = &self.layout.suggest_root {
            layout.push(format!(
                "- Suggestion: open {} to work across the whole repository",
                root.display()
            ));
        }
        section("Project", layout, "");

        let mut lines: Vec<String> = self
            .instructions
            .loaded
            .iter()
            .map(|i| {
                let cut = if i.truncated { ", cut to fit" } else { "" };
                format!(
                    "- {} ({}, {} chars{cut})",
                    i.path.display(),
                    i.kind,
                    i.chars
                )
            })
            .collect();
        lines.extend(self.instructions.rules.iter().map(|r| {
            let applies = if r.globs.is_empty() {
                "when relevant".to_owned()
            } else {
                r.globs.join(", ")
            };
            format!("- {} ({}, listed for {applies})", r.path.display(), r.kind)
        }));
        lines.extend(
            self.instructions
                .skipped
                .iter()
                .map(|s| format!("- {} (skipped: {})", s.path.display(), s.reason)),
        );
        section(
            "Instruction files",
            lines,
            "None found. AGENTS.md is the standard place for them.",
        );

        section(
            "Docs map",
            self.docs
                .iter()
                .map(|d| format!("- {} — {}", d.path.display(), d.title))
                .collect(),
            "None.",
        );

        let index = match &self.index {
            Some(status) if status.exists => vec![format!(
                "- {} ({} files, updated {})",
                status.path.display(),
                status.files,
                status
                    .updated
                    .map(|t| t
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string())
                    .unwrap_or_default()
            )],
            Some(status) => vec![format!(
                "- {} (being built; `malac index` builds it now)",
                status.path.display()
            )],
            None => vec!["- Off: not a repository, or `[context] index = false`".to_owned()],
        };
        section("Project index", index, "");

        let knowledge = if self.knowledge.exists {
            format!(
                "- {} ({} chars)",
                self.knowledge.path.display(),
                self.knowledge.chars
            )
        } else {
            format!(
                "- {} (not yet; created when malac saves project knowledge)",
                self.knowledge.path.display()
            )
        };
        section("Project knowledge", vec![knowledge], "");

        section(
            "Skills",
            self.skills
                .iter()
                .map(|s| {
                    let manual = if s.model_invocable {
                        ""
                    } else {
                        ", run with /name only"
                    };
                    format!("- {} ({}{manual}): {}", s.name, s.source, s.description)
                })
                .collect(),
            "None.",
        );
        section(
            "Commands",
            self.commands
                .iter()
                .map(|c| {
                    let hint = c
                        .argument_hint
                        .as_deref()
                        .map(|h| format!(" {h}"))
                        .unwrap_or_default();
                    format!("- /{}{hint} ({}): {}", c.name, c.source, c.description)
                })
                .collect(),
            "None.",
        );
        section(
            "Agent types",
            self.agent_types
                .iter()
                .map(|a| format!("- {} ({}): {}", a.name, a.source, a.description))
                .collect(),
            "None.",
        );
        section(
            "Plugins",
            self.plugins
                .iter()
                .map(|p| {
                    let state = match (p.enabled, &p.error) {
                        (_, Some(err)) => format!("error: {err}"),
                        (true, None) => "enabled".to_owned(),
                        (false, None) => "disabled".to_owned(),
                    };
                    let pending = if p.pending.is_empty() {
                        String::new()
                    } else {
                        format!(", {} awaiting approval", p.pending.join(" and "))
                    };
                    let parts: Vec<String> = [
                        ("skills", &p.skills),
                        ("commands", &p.commands),
                        ("agents", &p.agents),
                        ("MCP servers", &p.mcp),
                        ("hooks", &p.hooks),
                    ]
                    .iter()
                    .filter(|(_, names)| !names.is_empty())
                    .map(|(label, names)| format!("{label} {}", names.join(", ")))
                    .collect();
                    format!(
                        "- {} {} ({state}{pending}): {}",
                        p.id,
                        p.version.as_deref().unwrap_or(""),
                        parts.join("; ")
                    )
                })
                .collect(),
            "None.",
        );
        if !self.warnings.is_empty() {
            section(
                "Warnings",
                self.warnings.iter().map(|w| format!("- {w}")).collect(),
                "",
            );
        }
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_the_root_from_one_part_of_a_monorepo() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        for (dir, marker) in [
            ("frontend", "package.json"),
            ("backend", "go.mod"),
            ("infra", "Dockerfile"),
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join(marker), "").unwrap();
        }
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join("frontend/src")).unwrap();

        let here = layout(root, &root.join("frontend/src"));
        assert_eq!(here.subdir.as_deref(), Some(Path::new("frontend/src")));
        assert_eq!(here.siblings, vec!["backend", "infra"]);
        assert_eq!(here.suggest_root.as_deref(), Some(root));
        assert!(here.prompt_line().unwrap().contains("`backend/`, `infra/`"));

        let top = layout(root, root);
        assert!(top.subdir.is_none() && top.suggest_root.is_none());
        assert!(top.prompt_line().is_none());
    }
}
